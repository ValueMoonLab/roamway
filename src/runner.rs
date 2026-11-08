use crate::types::*;
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::Write;
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
    let (run, log_path) = {
        let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let idx = match st.state.runs.iter().position(|r| r.id == id) {
            Some(i) => i,
            None => return,
        };
        // 被 stop 抢先改掉的任务绝不复活：状态必须是 Queued 且无取消标记
        if !run_is_startable(st.state.runs[idx].status, &cancel) {
            drop(st);
            remove_cancel(&app, &id);
            return;
        }
        st.state.runs[idx].status = RunStatus::Running;
        st.state.runs[idx].started_at = Some(now());
        let r = st.state.runs[idx].clone();
        let lp = st.log_path(&id);
        st.save();
        (r, lp)
    };

    // spawn 前再查一次取消标志
    if cancel.load(Ordering::Relaxed) {
        finalize(&app, &id, RunStatus::Killed, None, None);
        return;
    }

    {
        let mut f = match File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                finalize(&app, &id, RunStatus::Failed, None, Some(format!("log: {}", e)));
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

    let (status, code, err) = if run.agent.starts_with("api:") {
        run_api(&run, &log_path, &cancel)
    } else {
        run_cli(&app, &run, &log_path, &cancel)
    };

    if let Ok(mut f) = OpenOptions::new().append(true).open(&log_path) {
        let _ = writeln!(f, "\n# end {} status={:?} exit={:?}", iso(now()), status, code);
    }
    finalize(&app, &id, status, code, err);
}

fn env_list(key: &str) -> Vec<String> {
    std::env::var(key)
        .map(|v| v.split_whitespace().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

fn build_command(run: &Run) -> Result<Command, String> {
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
        "claude" => ("claude".into(), vec!["-p".into(), run.prompt.clone()]),
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
) -> (RunStatus, Option<i32>, Option<String>) {
    let mut cmd = match build_command(run) {
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
) -> (RunStatus, Option<i32>, Option<String>) {
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
            )
        }
    };
    let model = run
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| default_model.to_string());
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": run.prompt}],
        "stream": false
    });
    let resp = ureq::post(&url)
        .set("authorization", &format!("Bearer {}", key))
        .set("content-type", "application/json")
        .timeout(Duration::from_secs(run.timeout_s.max(30)))
        .send_json(body);

    let mut f = match OpenOptions::new().append(true).create(true).open(log_path) {
        Ok(f) => f,
        Err(e) => return (RunStatus::Failed, None, Some(e.to_string())),
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
                    return (RunStatus::Failed, Some(0), Some("empty completion".into()));
                }
                (RunStatus::Done, Some(0), None)
            }
            Err(e) => (RunStatus::Failed, None, Some(e.to_string())),
        },
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let _ = writeln!(f, "HTTP {}: {}", code, body);
            (
                RunStatus::Failed,
                Some(code as i32),
                Some(format!("http {}", code)),
            )
        }
        Err(e) => {
            let _ = writeln!(f, "request error: {}", e);
            (RunStatus::Failed, None, Some(e.to_string()))
        }
    }
}

fn finalize(app: &Arc<App>, id: &str, status: RunStatus, code: Option<i32>, err: Option<String>) {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let idx = match st.state.runs.iter().position(|r| r.id == id) {
        Some(i) => i,
        None => return,
    };
    let job_id = {
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
            if err.is_some() {
                r.error = err;
            }
        }
        r.job_id.clone()
    };
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
        }
    }

    #[test]
    fn unknown_agent_rejected_by_builder() {
        assert!(build_command(&sample_run("rm -rf /")).is_err());
        assert!(build_command(&sample_run("shell")).is_ok());
        assert!(build_command(&sample_run("codex")).is_ok());
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
