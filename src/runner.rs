use crate::agents;
use crate::types::*;
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
        st.state.runs[idx].status = RunStatus::Running;
        st.state.runs[idx].started_at = Some(now());
        let r = st.state.runs[idx].clone();
        let ctx = match r.agent_id.as_deref() {
            Some(aid) => st
                .state
                .agents
                .iter()
                .find(|a| a.id == aid)
                .map(|a| (a.kind.clone(), a.session_id.clone(), a.history.clone()))
                .unwrap_or_else(|| (r.agent.clone(), String::new(), Vec::new())),
            None => (r.agent.clone(), String::new(), Vec::new()),
        };
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
        let mut f = match File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                finalize(&app, &id, RunStatus::Failed, None, Some(format!("log: {}", e)), None);
                return;
            }
        };
        let _ = writeln!(
            f,
            "# {} run={} agent={} model={:?} dir={}",
            iso(now()),
            id,
            run.agent,
            run.model,
            run.workspace
        );
        let _ = f.flush();
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
        let (status, code, err, text, usage) = run_api(&run, &log_path, &cancel, &history);
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
        let (status, code, err) = run_cli(&app, &run, &log_path, &cancel, session_for_run, claude_flag);
        (status, code, err, String::new(), None)
    };
    let (status, code, err, assistant, mut usage) = outcome;
    if usage.is_none() && status == RunStatus::Done && !run.agent.starts_with("api:") {
        usage = Some(RunUsage::Note("unavailable".into()));
    }

    if let Ok(mut f) = OpenOptions::new().append(true).open(&log_path) {
        let _ = writeln!(f, "\n# end {} status={:?} exit={:?}", iso(now()), status, code);
    }

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
    if status == RunStatus::Done {
        if let Some(note) = resume_note.take() {
            mark_agent_error(&app, &run.agent_id, &note);
        }
    }

    finalize(&app, &id, status, code, err, usage);

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

fn update_agent_run_status(app: &Arc<App>, agent_id: &Option<String>, run_id: &str, status: &RunStatus, err: Option<&str>) {
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

fn build_command_with_session(
    run: &Run,
    session_id: Option<&str>,
    claude_resume_flag: Option<&str>,
) -> Result<Command, String> {
    let (prog, args): (String, Vec<String>) = match run.agent.as_str() {
        "shell" => {
            if cfg!(windows) {
                ("cmd".into(), vec!["/C".into(), run.prompt.clone()])
            } else {
                ("sh".into(), vec!["-c".into(), run.prompt.clone()])
            }
        }
        "codex" => {
            let mut a: Vec<String> = vec![
                "exec".into(),
                "--skip-git-repo-check".into(),
                "-s".into(),
                "workspace-write".into(),
            ];
            if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
                a.push("resume".into());
                a.push(sid.to_string());
            }
            if !run.workspace.is_empty() {
                a.push("-C".into());
                a.push(run.workspace.clone());
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
            let mut a: Vec<String> = vec!["-z".into(), run.prompt.clone()];
            if let Some(m) = run.model.as_ref().filter(|m| !m.is_empty()) {
                a.push("-m".into());
                a.push(m.clone());
            }
            a.extend(env_list("ROAMWAY_HERMES_ARGS"));
            ("hermes".into(), a)
        }
        "claude" => {
            let mut a: Vec<String> = vec!["-p".into()];
            if let (Some(flag), Some(sid)) = (claude_resume_flag, session_id.filter(|s| !s.is_empty())) {
                a.push(flag.to_string());
                a.push(sid.to_string());
            }
            a.push(run.prompt.clone());
            ("claude".into(), a)
        }
        other => return Err(format!("unknown agent: {}", other)),
    };

    let mut c = Command::new(prog);
    c.args(args);
    if !run.workspace.is_empty() && Path::new(&run.workspace).is_dir() {
        c.current_dir(&run.workspace);
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
    let out = match File::create(log_path) {
        Ok(f) => f,
        Err(e) => return (RunStatus::Failed, None, Some(format!("log: {}", e))),
    };
    let out2 = match out.try_clone() {
        Ok(f) => f,
        Err(e) => return (RunStatus::Failed, None, Some(format!("log: {}", e))),
    };
    cmd.stdout(Stdio::from(out))
        .stderr(Stdio::from(out2))
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
}

fn run_api(
    run: &Run,
    log_path: &Path,
    _cancel: &Arc<AtomicBool>,
    history: &[agents::Msg],
 ) -> (RunStatus, Option<i32>, Option<String>, String, Option<RunUsage>) {
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
    if !history.iter().any(|m| m.role == "user" && m.content == run.prompt) {
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

    let mut f = match OpenOptions::new().append(true).create(true).open(log_path) {
        Ok(f) => f,
        Err(e) => return (RunStatus::Failed, None, Some(e.to_string()), String::new(), None),
    };
    match resp {
        Ok(r) => match r.into_json::<serde_json::Value>() {
            Ok(v) => {
                let text = v["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let _ = writeln!(f, "{}", text);
                let _ = f.flush();
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
            Err(e) => (RunStatus::Failed, None, Some(e.to_string()), String::new(), None),
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let _ = writeln!(f, "HTTP {}: {}", code, body);
            (
                RunStatus::Failed,
                Some(code as i32),
                Some(format!("http {}", code)),
                String::new(),
                None,
            )
        }
        Err(e) => {
            let _ = writeln!(f, "request error: {}", e);
            (RunStatus::Failed, None, Some(e.to_string()), String::new(), None)
        }
    }
}
fn finalize(app: &Arc<App>, id: &str, status: RunStatus, code: Option<i32>, err: Option<String>, usage: Option<RunUsage>) {
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
        let bad = matches!(status, RunStatus::Failed | RunStatus::Timeout);
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
                crate::jobs::apply_failure(j, t, tz, &format!("{:?}", status));
            } else {
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
            timeout_s: 10,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
            usage: None,
        }
    }

    #[test]
    fn unknown_agent_rejected_by_builder() {
        assert!(build_command_with_session(&sample_run("rm -rf /"), None, None).is_err());
        assert!(build_command_with_session(&sample_run("shell"), None, None).is_ok());
        assert!(build_command_with_session(&sample_run("codex"), None, None).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn start_times_near_tolerance() {
        assert!(start_times_near(1_700_000_000, 1_700_000_010));
        assert!(start_times_near(1_700_000_000, 1_700_000_000));
        assert!(!start_times_near(1_700_000_000, 1_700_000_100));
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
}
