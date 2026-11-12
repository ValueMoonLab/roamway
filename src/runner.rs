use crate::agents;
use crate::types::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub fn start(app: Arc<App>, run_id: String) {
    thread::spawn(move || execute(app, run_id));
}

/// stop 后的守卫：只有状态仍为 Queued 且没有取消标记时才允许开始执行
fn run_is_startable(status: RunStatus, cancel: &AtomicBool) -> bool {
    status == RunStatus::Queued && !cancel.load(Ordering::Relaxed)
}

fn remove_cancel(app: &Arc<App>, id: &str) {
    if let Ok(mut c) = app.cancels.lock() {
        c.remove(id);
    }
}

fn execute(app: Arc<App>, id: String) {
    let cancel = {
        let mut c = app.cancels.lock().unwrap_or_else(|e| e.into_inner());
        c.entry(id.clone())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    };
    let (run, log_path, agent_ctx) = {
        let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let idx = match st.state.runs.iter().position(|r| r.id == id) {
            Some(i) => i,
            None => return,
        };
        if !run_is_startable(st.state.runs[idx].status, &cancel) {
            drop(st);
            remove_cancel(&app, &id);
            return;
        }
        let run_ref = &st.state.runs[idx];
        let ctx = match run_ref.agent_id.as_deref() {
            Some(aid) => st
                .state
                .agents
                .iter()
                .find(|a| a.id == aid)
                .map(|a| (a.kind.clone(), a.session_id.clone(), a.history.clone()))
                .unwrap_or_else(|| (run_ref.agent.clone(), String::new(), Vec::new())),
            None => (run_ref.agent.clone(), String::new(), Vec::new()),
        };
        st.state.runs[idx].status = RunStatus::Running;
        st.state.runs[idx].started_at = Some(now());
        st.state.runs[idx].resumed = agents::should_resume(&ctx.0, &ctx.1);
        let r = st.state.runs[idx].clone();
        let lp = st.log_path(&id);
        st.save();
        (r, lp, ctx)
    };

    if cancel.load(Ordering::Relaxed) {
        finalize(&app, &id, RunStatus::Killed, None, None, None);
        return;
    }

    crate::plugins::fire(
        &app,
        "agent.created",
        json!({ "run_id": id.clone(), "agent": run.agent.clone(), "status": "running" }),
    );

    {
        if let Err(e) = File::create(&log_path) {
            finalize(
                &app,
                &id,
                RunStatus::Failed,
                None,
                Some(format!("log: {}", e)),
                None,
            );
            return;
        }
        let option_keys = run
            .options
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let header = format!(
            "# {} run={} agent={} model={:?} dir={}\n[runopts] agent={} keys={} images={}\n",
            iso(now()),
            id,
            run.agent,
            run.model,
            run.workspace,
            run.agent,
            option_keys,
            run.images.len()
        );
        if let Err(e) = append_capped(&log_path, header.as_bytes(), app.max_log_bytes) {
            finalize(
                &app,
                &id,
                RunStatus::Failed,
                None,
                Some(format!("log: {}", e)),
                None,
            );
            return;
        }
    }

    let (agent_kind, mut session, history) = agent_ctx;
    let mut resume_note: Option<String> = None;
    let mut claude_flag: Option<&'static str> = None;
    if run.agent_id.is_some() && agent_kind == "claude" {
        match probe_claude_flags() {
            Some(flags) => {
                if session.is_empty() {
                    if let Some(sf) = flags.session_flag {
                        session = agents::gen_session_id();
                        record_agent_session(&app, &run.agent_id, &session);
                        claude_flag = Some(sf);
                    } else {
                        resume_note = Some("resume unsupported".into());
                    }
                } else {
                    claude_flag = flags.resume_flag.or(flags.session_flag);
                }
            }
            None => resume_note = Some("resume unsupported".into()),
        }
    }

    let outcome = if run.agent.starts_with("api:") {
        let (status, code, err, text, usage) =
            run_api(&run, &log_path, &cancel, &history, app.max_log_bytes);
        (status, code, err, text, usage)
    } else {
        let session_for_run = if matches!(agent_kind.as_str(), "codex" | "claude") {
            if session.is_empty() {
                None
            } else {
                Some(session.as_str())
            }
        } else {
            None
        };
        let (status, code, err) =
            run_cli(&app, &run, &log_path, &cancel, session_for_run, claude_flag);
        (status, code, err, String::new(), None)
    };
    let (status, code, err, assistant, mut usage) = outcome;
    if usage.is_none() && status == RunStatus::Done && !run.agent.starts_with("api:") {
        usage = Some(RunUsage::Note("unavailable".into()));
    }

    let end = format!(
        "\n# end {} status={:?} exit={:?}\n",
        iso(now()),
        status,
        code
    );
    let _ = append_capped(&log_path, end.as_bytes(), app.max_log_bytes);

    if status == RunStatus::Done && agent_kind == "codex" {
        if session.is_empty() {
            match extract_codex_session(&log_path) {
                Some(sid) => {
                    record_agent_session(&app, &run.agent_id, &sid);
                }
                None => {
                    resume_note = Some("resume unsupported".into());
                }
            }
        } else if let Some(sid) = extract_codex_session(&log_path) {
            record_agent_session(&app, &run.agent_id, &sid);
        }
    }
    if !assistant.is_empty() {
        push_assistant(&app, &run.agent_id, &assistant);
    }
    update_agent_run_status(&app, &run.agent_id, &id, &status, err.as_deref());
    finalize(&app, &id, status, code, err, usage);

    if let Some(note) = resume_note.take() {
        eprintln!(
            "[resume] unsupported kind={} agent={}",
            agent_kind,
            run.agent_id.as_deref().unwrap_or(&run.agent)
        );
        apply_resume_note_to_run(&app, &id, &note);
        mark_agent_error(&app, &run.agent_id, &note);
    }

    if status == RunStatus::Done {
        crate::plugins::fire(
            &app,
            "agent.finished",
            json!({ "run_id": id.clone(), "agent": run.agent.clone(), "status": "done" }),
        );
    } else if matches!(status, RunStatus::Failed | RunStatus::Timeout) {
        crate::plugins::fire(
            &app,
            "run.failed",
            json!({ "run_id": id.clone(), "agent": run.agent.clone(), "status": format!("{:?}", status).to_lowercase() }),
        );
    }
}

fn update_agent_run_status(
    app: &Arc<App>,
    agent_id: &Option<String>,
    run_id: &str,
    status: &RunStatus,
    err: Option<&str>,
) {
    let aid = match agent_id.as_deref() {
        Some(a) => a,
        None => return,
    };
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = st.state.agents.iter_mut().find(|a| a.id == aid) {
        let ok = *status == RunStatus::Done;
        agents::apply_run_status_to_agent(a, run_id, ok, err);
    }
    st.save();
}

fn record_agent_session(app: &Arc<App>, agent_id: &Option<String>, sid: &str) {
    let aid = match agent_id.as_deref() {
        Some(a) => a,
        None => return,
    };
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = st.state.agents.iter_mut().find(|a| a.id == aid) {
        agents::record_session_id(a, sid);
    }
    st.save();
}

/// 把 resume_note 落到 run：Done 时同时写 error，不改 status / exit_code
fn apply_resume_note(run: &mut Run, note: &str) {
    run.resume_note = Some(note.to_string());
    if run.status == RunStatus::Done {
        run.error = Some(note.to_string());
    }
}

fn apply_resume_note_to_run(app: &Arc<App>, run_id: &str, note: &str) {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = st.state.runs.iter_mut().find(|r| r.id == run_id) {
        apply_resume_note(r, note);
    }
    st.save();
}

fn mark_agent_error(app: &Arc<App>, agent_id: &Option<String>, err: &str) {
    let aid = match agent_id.as_deref() {
        Some(a) => a,
        None => return,
    };
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = st.state.agents.iter_mut().find(|a| a.id == aid) {
        a.last_error = err.to_string();
        a.updated_at = now();
    }
    st.save();
}

fn push_assistant(app: &Arc<App>, agent_id: &Option<String>, text: &str) {
    let aid = match agent_id.as_deref() {
        Some(a) => a,
        None => return,
    };
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = st.state.agents.iter_mut().find(|a| a.id == aid) {
        agents::push_msg(a, "assistant", text);
    }
    st.save();
}

struct ClaudeFlags {
    session_flag: Option<&'static str>,
    resume_flag: Option<&'static str>,
}

fn probe_claude_flags() -> Option<ClaudeFlags> {
    let out = Command::new("claude").args(["--help"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    if !text.contains("--session-id") && !text.contains("--resume") {
        return None;
    }
    Some(ClaudeFlags {
        session_flag: if text.contains("--session-id") {
            Some("--session-id")
        } else {
            None
        },
        resume_flag: if text.contains("--resume") {
            Some("--resume")
        } else {
            None
        },
    })
}

fn extract_codex_session(log_path: &Path) -> Option<String> {
    let mut s = String::new();
    File::open(log_path).ok()?.read_to_string(&mut s).ok()?;
    for line in s.lines() {
        let l = line.trim();
        let lower = l.to_lowercase();
        for key in ["session id:", "session:"] {
            if lower.starts_with(key) {
                let sid = l[key.len()..].trim();
                if !sid.is_empty() {
                    return Some(sid.to_string());
                }
            }
        }
    }
    None
}
fn env_list(key: &str) -> Vec<String> {
    std::env::var(key)
        .map(|v| v.split_whitespace().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

fn first_option_key(opts: &BTreeMap<String, String>) -> Option<&str> {
    opts.keys().next().map(String::as_str)
}

fn reject_any_option(
    kind: &str,
    opts: &BTreeMap<String, String>,
    images: &[String],
) -> Result<(), String> {
    if let Some(key) = first_option_key(opts) {
        return Err(format!("run --{}: not supported for agent {}", key, kind));
    }
    if !images.is_empty() {
        return Err(format!("run --image: not supported for agent {}", kind));
    }
    Ok(())
}

fn claude_thinking_tokens(level: &str) -> Option<&'static str> {
    match level {
        "low" => Some("2048"),
        "medium" => Some("8192"),
        "high" => Some("24576"),
        _ => None,
    }
}

/// 把 per-run options/images 映射成 argv 片段，顺序固定：
/// thinking → mode → provider → images → output_schema。
/// provider 没有对应能力时显式报错，不静默丢弃。
pub fn provider_args(
    kind: &str,
    opts: &BTreeMap<String, String>,
    images: &[String],
    mode: Option<&str>,
) -> Result<Vec<String>, String> {
    match kind {
        "codex" => {
            let mut args: Vec<String> = Vec::new();
            if let Some(level) = opts.get("thinking") {
                args.push("-c".into());
                args.push(format!("model_reasoning_effort={}", level));
            }
            if let Some(m) = mode {
                args.push("-s".into());
                args.push(m.to_string());
            }
            if let Some(provider) = opts.get("provider") {
                match provider.as_str() {
                    "ollama" | "lmstudio" => {
                        args.push("--oss".into());
                        args.push("--local-provider".into());
                        args.push(provider.clone());
                    }
                    "oss" => args.push("--oss".into()),
                    other => {
                        args.push("-c".into());
                        args.push(format!("model_provider={}", other));
                    }
                }
            }
            for image in images {
                args.push("-i".into());
                args.push(image.clone());
            }
            if let Some(schema) = opts.get("output_schema") {
                args.push("--output-schema".into());
                args.push(schema.clone());
            }
            Ok(args)
        }
        "claude" => {
            if opts.contains_key("provider") {
                return Err("run --provider: not supported for agent claude".into());
            }
            if !images.is_empty() {
                return Err("run --image: not supported for agent claude".into());
            }
            if opts.contains_key("output_schema") {
                return Err("run --output-schema: not supported for agent claude".into());
            }
            let mut args: Vec<String> = Vec::new();
            let m = mode.or_else(|| opts.get("mode").map(String::as_str));
            if let Some(m) = m {
                args.push("--permission-mode".into());
                args.push(m.to_string());
            }
            Ok(args)
        }
        "shell" | "hermes" => {
            reject_any_option(kind, opts, images)?;
            Ok(Vec::new())
        }
        other if other.starts_with("api:") => {
            reject_any_option(other, opts, images)?;
            Ok(Vec::new())
        }
        other => Err(format!("unknown agent: {}", other)),
    }
}

fn build_command_with_session(
    run: &Run,
    session_id: Option<&str>,
    claude_resume_flag: Option<&str>,
) -> Result<Command, String> {
    let effective = run
        .cwd
        .as_deref()
        .filter(|d| !d.is_empty())
        .unwrap_or(&run.workspace);
    let (prog, args): (String, Vec<String>) = match run.agent.as_str() {
        "shell" => {
            reject_any_option("shell", &run.options, &run.images)?;
            if cfg!(windows) {
                ("cmd".into(), vec!["/C".into(), run.prompt.clone()])
            } else {
                ("sh".into(), vec!["-c".into(), run.prompt.clone()])
            }
        }
        "codex" => {
            let mode = run.options.get("mode").map(String::as_str);
            let extra = provider_args("codex", &run.options, &run.images, mode)?;
            let mut a: Vec<String> = vec!["exec".into(), "--skip-git-repo-check".into()];
            if run.options.contains_key("mode") {
                // provider_args 已输出 -s <mode>
            } else {
                a.push("-s".into());
                a.push("workspace-write".into());
            }
            a.extend(extra);
            if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
                a.push("resume".into());
                a.push(sid.to_string());
            }
            if !effective.is_empty() {
                a.push("-C".into());
                a.push(effective.to_string());
            }
            if let Some(m) = run.model.as_ref().filter(|m| !m.is_empty()) {
                a.push("--model".into());
                a.push(m.clone());
            }
            a.extend(env_list("ROAMWAY_CODEX_ARGS"));
            a.push(run.prompt.clone());
            ("codex".into(), a)
        }
        "hermes" => {
            reject_any_option("hermes", &run.options, &run.images)?;
            let mut a: Vec<String> = vec!["-z".into(), run.prompt.clone()];
            if let Some(m) = run.model.as_ref().filter(|m| !m.is_empty()) {
                a.push("-m".into());
                a.push(m.clone());
            }
            a.extend(env_list("ROAMWAY_HERMES_ARGS"));
            ("hermes".into(), a)
        }
        "claude" => {
            let extra = provider_args(
                "claude",
                &run.options,
                &run.images,
                run.options.get("mode").map(String::as_str),
            )?;
            let mut a: Vec<String> = vec!["-p".into()];
            if let (Some(flag), Some(sid)) =
                (claude_resume_flag, session_id.filter(|s| !s.is_empty()))
            {
                a.push(flag.to_string());
                a.push(sid.to_string());
            }
            a.extend(extra);
            a.push(run.prompt.clone());
            ("claude".into(), a)
        }
        other => return Err(format!("unknown agent: {}", other)),
    };

    let mut c = Command::new(prog);
    if run.agent == "claude" {
        if let Some(level) = run.options.get("thinking") {
            if let Some(tokens) = claude_thinking_tokens(level) {
                c.env("MAX_THINKING_TOKENS", tokens);
            }
        }
    }
    c.args(args);
    if !effective.is_empty() && Path::new(effective).is_dir() {
        c.current_dir(effective);
    }
    configure_process(&mut c);
    Ok(c)
}
#[cfg(unix)]
fn configure_process(c: &mut Command) {
    use std::os::unix::process::CommandExt;
    c.process_group(0);
}

#[cfg(windows)]
fn configure_process(c: &mut Command) {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    use std::os::windows::process::CommandExt;
    c.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

/// 终止子进程及其整个进程组/进程树，先 SIGTERM（Windows 无此步），超时后 SIGKILL。
pub fn kill_tree(child: &mut Child) {
    kill_process_group(child.id());
    let _ = child.wait();
}

#[cfg(unix)]
pub fn kill_process_group(pid: u32) {
    let pid = pid as i32;
    if pid <= 1 {
        return;
    }
    unsafe {
        let _ = libc::kill(-pid, libc::SIGTERM);
        for _ in 0..20 {
            if libc::kill(-pid, 0) != 0 {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = libc::kill(-pid, libc::SIGKILL);
    }
}

#[cfg(windows)]
pub fn kill_process_group(pid: u32) {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    use std::os::windows::process::CommandExt;
    let pid_s = pid.to_string();
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", pid_s.as_str()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// 判断 pid 是否仍属于我们记录的 run：先看进程是否存在，Linux 上再对比
/// /proc 里的启动时间，降低 PID 复用导致的误杀。
#[cfg(not(unix))]
pub fn pid_is_our_run(_pid: u32, _started_at: Option<i64>) -> bool {
    false
}

#[cfg(unix)]
pub fn pid_is_our_run(pid: u32, _started_at: Option<i64>) -> bool {
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(ts) = _started_at {
            if let Some(proc_ts) = proc_start_epoch(pid) {
                return start_times_near(proc_ts, ts);
            }
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn start_times_near(proc_start: i64, recorded: i64) -> bool {
    (proc_start - recorded).abs() <= 30
}

#[cfg(target_os = "linux")]
fn proc_start_epoch(pid: u32) -> Option<i64> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // starttime 是 field 22；comm（field 2）可能带空格/括号，先按 ')' 切开
    let start = stat
        .rsplit(')')
        .next()?
        .split_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()? as i64;
    let btime = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find(|l| l.starts_with("btime "))?
        .split_whitespace()
        .nth(1)?
        .parse::<i64>()
        .ok()?;
    Some(btime + start / 100)
}

/// 把单 run 日志控制在 `limit` 字节以内：`limit == 0` 表示不限。
/// 写入前检查文件末尾是否已有 truncate 标记，已标记则直接返回；
/// 已存在内容不重写、不截断，只按 UTF-8 边界截断新 chunk 后停止后续写入。
pub fn append_capped(path: &Path, chunk: &[u8], limit: usize) -> std::io::Result<()> {
    let mut f = OpenOptions::new().append(true).create(true).open(path)?;
    if limit == 0 {
        f.write_all(chunk)?;
        return Ok(());
    }
    let marker = format!(
        "\n[roamway: log truncated; max_log_bytes={} reached]\n",
        limit
    );
    let len = f.metadata()?.len();
    if len >= marker.len() as u64 && log_ends_with_marker(path, marker.as_bytes())? {
        return Ok(());
    }
    if chunk.is_empty() || len.saturating_add(chunk.len() as u64) <= limit as u64 {
        f.write_all(chunk)?;
        return Ok(());
    }
    let room = (limit as u64).saturating_sub(len);
    let mut end = (room as usize).min(chunk.len());
    while end > 0 && end < chunk.len() && (chunk[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    f.write_all(&chunk[..end])?;
    f.write_all(marker.as_bytes())?;
    f.flush()?;
    Ok(())
}

/// 只读文件末尾 200 字节，检查是否已以固定标记收尾。
fn log_ends_with_marker(path: &Path, marker: &[u8]) -> std::io::Result<bool> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    if len < marker.len() as u64 {
        return Ok(false);
    }
    f.seek(SeekFrom::Start(len.saturating_sub(200)))?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail)?;
    Ok(tail.ends_with(marker))
}

/// 把子进程的一路输出持续追加到日志，写完由外部锁串行化，避免 stdout/stderr 交错。
fn pump_to_log<R: Read>(mut src: R, log_path: PathBuf, limit: usize, lock: Arc<Mutex<()>>) {
    let mut buf = [0u8; 8192];
    loop {
        match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Ok(_g) = lock.lock() {
                    let _ = append_capped(&log_path, &buf[..n], limit);
                }
            }
            Err(_) => break,
        }
    }
}

fn run_cli(
    app: &Arc<App>,
    run: &Run,
    log_path: &Path,
    cancel: &Arc<AtomicBool>,
    session_id: Option<&str>,
    claude_resume_flag: Option<&str>,
) -> (RunStatus, Option<i32>, Option<String>) {
    let mut cmd = match build_command_with_session(run, session_id, claude_resume_flag) {
        Ok(c) => c,
        Err(e) => return (RunStatus::Failed, None, Some(e)),
    };
    for (key, value) in &run.env {
        cmd.env(key, value);
    }
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    if cancel.load(Ordering::Relaxed) {
        return (RunStatus::Killed, None, None);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (RunStatus::Failed, None, Some(format!("spawn: {}", e))),
    };
    {
        let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = st.state.runs.iter_mut().find(|r| r.id == run.id) {
            r.pid = Some(child.id());
        }
        st.save();
    }
    if cancel.load(Ordering::Relaxed) {
        kill_tree(&mut child);
        return (RunStatus::Killed, None, None);
    }

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            kill_tree(&mut child);
            return (RunStatus::Failed, None, Some("no stdout pipe".to_string()));
        }
    };
    let stderr = match child.stderr.take() {
        Some(s) => s,
        None => {
            kill_tree(&mut child);
            return (RunStatus::Failed, None, Some("no stderr pipe".to_string()));
        }
    };
    let lock = Arc::new(Mutex::new(()));
    let out_log = log_path.to_path_buf();
    let err_log = log_path.to_path_buf();
    let limit = app.max_log_bytes;
    let out_lock = Arc::clone(&lock);
    let out_thread = thread::spawn(move || pump_to_log(stdout, out_log, limit, out_lock));
    let err_thread = thread::spawn(move || pump_to_log(stderr, err_log, limit, lock));

    let outcome = (|| {
        let started = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                kill_tree(&mut child);
                return (RunStatus::Killed, None, None);
            }
            if run.timeout_s > 0 && started.elapsed().as_secs() >= run.timeout_s {
                kill_tree(&mut child);
                return (
                    RunStatus::Timeout,
                    None,
                    Some(format!("timeout {}s", run.timeout_s)),
                );
            }
            match child.try_wait() {
                Ok(Some(st)) => {
                    let code = st.code();
                    if st.success() {
                        return (RunStatus::Done, code, None);
                    }
                    return (RunStatus::Failed, code, Some(format!("exit {:?}", code)));
                }
                Ok(None) => thread::sleep(Duration::from_millis(300)),
                Err(e) => {
                    kill_tree(&mut child);
                    return (RunStatus::Failed, None, Some(e.to_string()));
                }
            }
        }
    })();
    let _ = out_thread.join();
    let _ = err_thread.join();
    outcome
}

fn run_api(
    run: &Run,
    log_path: &Path,
    _cancel: &Arc<AtomicBool>,
    history: &[agents::Msg],
    max_log_bytes: usize,
) -> (
    RunStatus,
    Option<i32>,
    Option<String>,
    String,
    Option<RunUsage>,
) {
    let (default_url, key_env, url_env, default_model) = match run.agent.as_str() {
        "api:deepseek" => (
            "https://api.deepseek.com/chat/completions",
            "ROAMWAY_DEEPSEEK_KEY",
            "ROAMWAY_DEEPSEEK_URL",
            "deepseek-chat",
        ),
        "api:qwen" => (
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            "ROAMWAY_QWEN_KEY",
            "ROAMWAY_QWEN_URL",
            "qwen-plus",
        ),
        other => {
            return (
                RunStatus::Failed,
                None,
                Some(format!("unknown api agent {}", other)),
                String::new(),
                None,
            )
        }
    };
    let url = std::env::var(url_env)
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| default_url.to_string());
    let key = match std::env::var(key_env) {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            return (
                RunStatus::Failed,
                None,
                Some(format!("missing env {}", key_env)),
                String::new(),
                None,
            )
        }
    };
    let model = run
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| default_model.to_string());
    let mut messages: Vec<serde_json::Value> = history
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    if !history
        .iter()
        .any(|m| m.role == "user" && m.content == run.prompt)
    {
        messages.push(json!({ "role": "user", "content": run.prompt }));
    }
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": false
    });
    let resp = ureq::post(&url)
        .set("authorization", &format!("Bearer {}", key))
        .set("content-type", "application/json")
        .timeout(Duration::from_secs(run.timeout_s.max(30)))
        .send_json(body);

    match resp {
        Ok(r) => match r.into_json::<serde_json::Value>() {
            Ok(v) => {
                let text = v["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let _ = append_capped(log_path, format!("{}\n", text).as_bytes(), max_log_bytes);
                if text.is_empty() {
                    return (
                        RunStatus::Failed,
                        Some(0),
                        Some("empty completion".into()),
                        String::new(),
                        None,
                    );
                }
                let usage = Some(RunUsage::Data(Usage {
                    prompt_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                    completion_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
                    total_tokens: v["usage"]["total_tokens"].as_u64().unwrap_or(0),
                    model: model.clone(),
                    provider: run.agent.clone(),
                }));
                (RunStatus::Done, Some(0), None, text, usage)
            }
            Err(e) => (
                RunStatus::Failed,
                None,
                Some(e.to_string()),
                String::new(),
                None,
            ),
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let _ = append_capped(
                log_path,
                format!("HTTP {}: {}\n", code, body).as_bytes(),
                max_log_bytes,
            );
            (
                RunStatus::Failed,
                Some(code as i32),
                Some(format!("http {}", code)),
                String::new(),
                None,
            )
        }
        Err(e) => {
            let _ = append_capped(
                log_path,
                format!("request error: {}\n", e).as_bytes(),
                max_log_bytes,
            );
            (
                RunStatus::Failed,
                None,
                Some(e.to_string()),
                String::new(),
                None,
            )
        }
    }
}
fn finalize(
    app: &Arc<App>,
    id: &str,
    status: RunStatus,
    code: Option<i32>,
    err: Option<String>,
    usage: Option<RunUsage>,
) {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let idx = match st.state.runs.iter().position(|r| r.id == id) {
        Some(i) => i,
        None => return,
    };
    let result_err = err.clone();
    let (job_id, result_status) = {
        let r = &mut st.state.runs[idx];
        if r.status == RunStatus::Killed {
            // stop 已判 killed：自然退出/超时都不允许覆盖
            if r.ended_at.is_none() {
                r.ended_at = Some(now());
            }
        } else {
            r.status = status;
            r.exit_code = code;
            r.ended_at = Some(now());
            if usage.is_some() {
                r.usage = usage;
            }
            if err.is_some() {
                r.error = err;
            }
        }
        (r.job_id.clone(), r.status)
    };
    let schedule_id = job_id
        .as_deref()
        .filter(|j| j.starts_with("sch_"))
        .map(str::to_string);
    if let Some(jid) = job_id {
        let bad = matches!(result_status, RunStatus::Failed | RunStatus::Timeout);
        let t = now();
        let tz = st
            .state
            .jobs
            .iter()
            .find(|j| j.id == jid)
            .map(|j| j.tz_offset_hours)
            .unwrap_or(app.tz_offset_hours);
        if let Some(j) = st.state.jobs.iter_mut().find(|j| j.id == jid) {
            if bad {
                crate::jobs::apply_failure(j, t, tz, &format!("{:?}", result_status));
            } else if result_status != RunStatus::Killed {
                // Killed（用户 stop）既不记失败也不清历史计数，job 状态保持原样。
                j.attempts = 0;
                j.last_error = None;
            }
        }
    }
    st.save();
    drop(st);
    if let Some(sid) = schedule_id {
        crate::schedules::record_result(app, &sid, result_status, result_err);
    }
    remove_cancel(app, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_run(agent: &str) -> Run {
        Run {
            id: "run_000001".into(),
            title: String::new(),
            agent: agent.to_string(),
            model: None,
            workspace: String::new(),
            prompt: "echo hi".into(),
            status: RunStatus::Queued,
            created_at: 0,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            resumed: false,
            resume_note: None,
            timeout_s: 10,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
            trigger_id: None,
            usage: None,
            labels: std::collections::BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: std::collections::BTreeMap::new(),
            images: Vec::new(),
            worktree: None,
        }
    }

    #[test]
    fn unknown_agent_rejected_by_builder() {
        assert!(build_command_with_session(&sample_run("rm -rf /"), None, None).is_err());
        assert!(build_command_with_session(&sample_run("shell"), None, None).is_ok());
        assert!(build_command_with_session(&sample_run("codex"), None, None).is_ok());
    }

    #[test]
    fn finalize_killed_run_skips_failure_propagation() {
        let dir = std::env::temp_dir().join(format!("roamway-killed-finalize-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut run = sample_run("shell");
        run.status = RunStatus::Killed;
        run.job_id = Some("job_000001".into());
        let job = Job {
            id: "job_000001".into(),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: "echo hi".into(),
            every_secs: Some(60),
            daily: None,
            tz_offset_hours: 8,
            timeout_s: 60,
            max_retries: 2,
            enabled: true,
            allow_overlap: false,
            next_at: 0,
            last_run: None,
            last_error: None,
            attempts: 0,
            depends_on: vec![],
            last_skip: None,
        };
        let store = Store {
            dir: dir.clone(),
            state: State {
                runs: vec![run],
                jobs: vec![job],
                agents: vec![],
                seq: 1,
            },
            loaded_ok: true,
            store_health: StoreHealth::Ok,
            preserve_backup: false,
            last_write_hash: 0,
            writes_total: 0,
            writes_skipped: 0,
        };
        let app = Arc::new(App {
            store: Mutex::new(store),
            schedules: Mutex::new(Default::default()),
            terms: Mutex::new(Default::default()),
            term_rt: Mutex::new(Default::default()),
            token: None,
            started: now(),
            cancels: Mutex::new(Default::default()),
            tz_offset_hours: 8,
            max_concurrent: 4,
            max_conn: 64,
            keep_runs: 500,
            keep_days: 0,
            max_log_bytes: 4_194_304,
            conns: std::sync::atomic::AtomicUsize::new(0),
            req_total: std::sync::atomic::AtomicU64::new(0),
            req_failed: std::sync::atomic::AtomicU64::new(0),
            req_us: std::sync::atomic::AtomicU64::new(0),
            req_max_us: std::sync::atomic::AtomicU64::new(0),
            req_4xx: std::sync::atomic::AtomicU64::new(0),
            req_5xx: std::sync::atomic::AtomicU64::new(0),
            tokens: Mutex::new(Default::default()),
            approvals: Mutex::new(Default::default()),
            passwords: Mutex::new(Default::default()),
            pairings: Mutex::new(Default::default()),
            hub_rt: Mutex::new(Default::default()),
            allow_hosts: vec![],
            allow_origins: vec![],
            require_approval: vec![],
            bind_host: "127.0.0.1".into(),
            unix_socket: None,
            tls_cert: None,
            tls_key: None,
        });
        // 传入 status=Failed，但 run 已被 stop 判 Killed：不得触发失败重试。
        finalize(
            &app,
            "run_000001",
            RunStatus::Failed,
            Some(1),
            Some("boom".into()),
            None,
        );
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            st.state.jobs[0].attempts, 0,
            "Killed 的 run 不应触发失败重试"
        );
        assert_eq!(st.state.jobs[0].last_error, None);
        assert_eq!(st.state.runs[0].status, RunStatus::Killed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn start_times_near_tolerance() {
        assert!(start_times_near(1_700_000_000, 1_700_000_010));
        assert!(start_times_near(1_700_000_000, 1_700_000_000));
        assert!(!start_times_near(1_700_000_000, 1_700_000_100));
    }

    #[test]
    fn apply_resume_note_marks_done_run_error_without_changing_status() {
        let mut run = sample_run("shell");
        run.status = RunStatus::Done;
        run.exit_code = Some(0);
        apply_resume_note(&mut run, "resume unsupported");
        assert_eq!(run.status, RunStatus::Done);
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(run.error.as_deref(), Some("resume unsupported"));
        assert_eq!(run.resume_note.as_deref(), Some("resume unsupported"));
    }

    #[test]
    fn apply_resume_note_on_non_done_run_keeps_error_untouched() {
        let mut run = sample_run("shell");
        run.status = RunStatus::Failed;
        apply_resume_note(&mut run, "resume unsupported");
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.error, None);
        assert_eq!(run.resume_note.as_deref(), Some("resume unsupported"));
    }

    #[test]
    fn stop_guard_prevents_starting() {
        let cancel = AtomicBool::new(false);
        assert!(run_is_startable(RunStatus::Queued, &cancel));
        assert!(!run_is_startable(RunStatus::Killed, &cancel));
        assert!(!run_is_startable(RunStatus::Running, &cancel));
        cancel.store(true, Ordering::Relaxed);
        assert!(!run_is_startable(RunStatus::Queued, &cancel));
    }

    #[test]
    fn provider_args_codex_maps_thinking_mode_provider_image_schema() {
        let mut opts = BTreeMap::new();
        opts.insert("thinking".to_string(), "high".to_string());
        opts.insert("provider".to_string(), "ollama".to_string());
        opts.insert("output_schema".to_string(), "/tmp/s.json".to_string());
        let images = vec!["/tmp/a.png".to_string(), "/tmp/b.png".to_string()];
        let args = provider_args("codex", &opts, &images, Some("read-only")).unwrap();
        assert_eq!(
            args,
            vec![
                "-c",
                "model_reasoning_effort=high",
                "-s",
                "read-only",
                "--oss",
                "--local-provider",
                "ollama",
                "-i",
                "/tmp/a.png",
                "-i",
                "/tmp/b.png",
                "--output-schema",
                "/tmp/s.json",
            ]
        );
    }

    #[test]
    fn provider_args_empty_is_identity() {
        let empty = BTreeMap::new();
        assert_eq!(
            provider_args("codex", &empty, &[], None).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            provider_args("claude", &empty, &[], None).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            provider_args("shell", &empty, &[], None).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn provider_args_claude_rejects_provider_and_image() {
        let mut opts = BTreeMap::new();
        opts.insert("provider".to_string(), "ollama".to_string());
        assert_eq!(
            provider_args("claude", &opts, &[], None).unwrap_err(),
            "run --provider: not supported for agent claude"
        );

        let empty = BTreeMap::new();
        assert_eq!(
            provider_args("claude", &empty, &["/tmp/a.png".to_string()], None).unwrap_err(),
            "run --image: not supported for agent claude"
        );
    }

    #[test]
    fn provider_args_shell_rejects_any_option() {
        let mut opts = BTreeMap::new();
        opts.insert("thinking".to_string(), "high".to_string());
        assert_eq!(
            provider_args("shell", &opts, &[], None).unwrap_err(),
            "run --thinking: not supported for agent shell"
        );
    }

    #[test]
    fn append_capped_stops_at_limit_and_marks_once() {
        let path = std::env::temp_dir().join(format!(
            "roamway_p22_append_capped_{}.log",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let limit = 24usize;
        let marker = format!(
            "\n[roamway: log truncated; max_log_bytes={} reached]\n",
            limit
        );
        // 先写一个小于 limit 的 chunk，后续两次追加会触发封顶与"标记只写一次"。
        assert!(append_capped(&path, b"hello \xe4\xbd\xa0", limit).is_ok());
        assert!(append_capped(&path, "世界一二三四五六七八九".as_bytes(), limit).is_ok());
        assert!(append_capped(&path, b"again", limit).is_ok());
        let data = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let text = String::from_utf8_lossy(&data);
        assert!(
            text.ends_with(marker.as_str()),
            "log must end with truncate marker"
        );
        assert_eq!(
            text.matches(marker.trim()).count(),
            1,
            "marker must appear exactly once"
        );
        assert!(
            data.len() <= limit + marker.len(),
            "log must stay within limit + marker"
        );
    }

    #[test]
    fn append_capped_zero_means_unlimited() {
        let path = std::env::temp_dir().join(format!(
            "roamway_p22_append_unlimited_{}.log",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let chunk = b"0123456789".repeat(64);
        assert!(append_capped(&path, &chunk, 0).is_ok());
        let data = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(data, chunk);
        assert!(!String::from_utf8_lossy(&data).contains("log truncated"));
    }
}
