use crate::jobs::{backoff, next_after};
use crate::types::*;
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub fn start(app: Arc<App>, run_id: String) {
    thread::spawn(move || execute(app, run_id));
}

fn execute(app: Arc<App>, id: String) {
    let (run, log_path) = {
        let mut st = app.store.lock().unwrap();
        let idx = match st.state.runs.iter().position(|r| r.id == id) {
            Some(i) => i,
            None => return,
        };
        st.state.runs[idx].status = RunStatus::Running;
        st.state.runs[idx].started_at = Some(now());
        let r = st.state.runs[idx].clone();
        let lp = st.log_path(&id);
        st.save();
        (r, lp)
    };

    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut c) = app.cancels.lock() {
        c.insert(id.clone(), cancel.clone());
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

fn build_command(run: &Run) -> Command {
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
        other => (other.to_string(), vec![run.prompt.clone()]),
    };

    let mut c = Command::new(prog);
    c.args(args);
    if !run.workspace.is_empty() && Path::new(&run.workspace).is_dir() {
        c.current_dir(&run.workspace);
    }
    c
}

fn run_cli(
    app: &Arc<App>,
    run: &Run,
    log_path: &Path,
    cancel: &Arc<AtomicBool>,
) -> (RunStatus, Option<i32>, Option<String>) {
    let mut cmd = build_command(run);
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

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (RunStatus::Failed, None, Some(format!("spawn: {}", e))),
    };
    {
        let mut st = app.store.lock().unwrap();
        if let Some(r) = st.state.runs.iter_mut().find(|r| r.id == run.id) {
            r.pid = Some(child.id());
        }
        st.save();
    }

    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return (RunStatus::Killed, None, None);
        }
        if run.timeout_s > 0 && started.elapsed().as_secs() >= run.timeout_s {
            let _ = child.kill();
            let _ = child.wait();
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
                let _ = child.kill();
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
    let mut st = app.store.lock().unwrap();
    let idx = match st.state.runs.iter().position(|r| r.id == id) {
        Some(i) => i,
        None => return,
    };
    {
        let r = &mut st.state.runs[idx];
        r.status = status;
        r.exit_code = code;
        r.ended_at = Some(now());
        if err.is_some() {
            r.error = err;
        }
    }
    let job_id = st.state.runs[idx].job_id.clone();
    if let Some(jid) = job_id {
        let bad = matches!(status, RunStatus::Failed | RunStatus::Timeout);
        let t = now();
        let tz = st.state.jobs.iter().find(|j| j.id == jid).map(|j| j.tz_offset_hours).unwrap_or(app.tz_offset_hours);
        if let Some(j) = st.state.jobs.iter_mut().find(|j| j.id == jid) {
            if bad {
                j.attempts += 1;
                j.last_error = Some(format!("{:?}", status));
                if j.attempts <= j.max_retries {
                    j.next_at = t + backoff(j.attempts);
                } else {
                    if j.max_retries > 0 {
                        j.enabled = false;
                    }
                    j.attempts = 0;
                    j.next_at = next_after(j, t, tz);
                }
            } else {
                j.attempts = 0;
                j.last_error = None;
            }
        }
    }
    st.save();
    drop(st);
    if let Ok(mut c) = app.cancels.lock() {
        c.remove(id);
    }
}
