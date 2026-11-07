use crate::jobs;
use crate::runner;
use crate::types::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub fn serve(app: Arc<App>, bind: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind)?;
    println!("roamway listening on http://{}", bind);
    for s in listener.incoming() {
        if let Ok(stream) = s {
            let a = app.clone();
            thread::spawn(move || {
                let _ = handle(a, stream);
            });
        }
    }
    Ok(())
}

struct Req {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_req(s: &mut TcpStream) -> std::io::Result<Option<Req>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut head_end: Option<usize> = None;
    while head_end.is_none() {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
        head_end = find_sub(&buf, b"\r\n\r\n");
        if buf.len() > 64 * 1024 {
            return Ok(None);
        }
    }
    let head_end = head_end.unwrap();
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_query(q)),
        None => (target.clone(), HashMap::new()),
    };

    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    const MAX_BODY: usize = 1024 * 1024;
    let want = len.min(MAX_BODY);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < want {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(want);

    Ok(Some(Req {
        method,
        path,
        query,
        headers,
        body,
    }))
}

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for kv in q.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        m.insert(url_decode(k), url_decode(v));
    }
    m
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn handle(app: Arc<App>, mut s: TcpStream) -> std::io::Result<()> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
    let req = match read_req(&mut s)? {
        Some(r) => r,
        None => return Ok(()),
    };

    if let Some(tok) = app.token.clone() {
        let got = req
            .headers
            .get("x-roamway-token")
            .cloned()
            .or_else(|| req.query.get("token").cloned())
            .unwrap_or_default();
        if !ct_eq(&got, &tok) {
            return respond(&mut s, 401, &json!({"error": "unauthorized"}));
        }
    }

    let (code, body) = route(&app, &req);
    respond(&mut s, code, &body)
}

/// 常量时间比较，避免按字节提前返回泄露 token 前缀
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn respond(s: &mut TcpStream, code: u16, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(v).unwrap_or_else(|_| b"{}".to_vec());
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        code,
        reason(code),
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(&body)?;
    s.flush()
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

#[derive(Deserialize)]
struct NewRun {
    #[serde(default)]
    title: Option<String>,
    agent: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    prompt: String,
    #[serde(default)]
    timeout_s: Option<u64>,
}

#[derive(Deserialize)]
struct NewJob {
    #[serde(default)]
    title: Option<String>,
    agent: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    prompt: String,
    #[serde(default)]
    every_secs: Option<u64>,
    #[serde(default)]
    daily: Option<String>,
    #[serde(default)]
    tz_offset_hours: Option<i64>,
    #[serde(default)]
    timeout_s: Option<u64>,
    #[serde(default)]
    max_retries: Option<u32>,
    #[serde(default)]
    allow_overlap: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
}

fn route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let segs: Vec<&str> = req.path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let m = req.method.as_str();
    match (m, segs.as_slice()) {
        ("GET", ["health"]) => (200, health(app)),
        ("GET", ["v1", "runs"]) => {
            let limit: usize = req
                .query
                .get("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(50);
            let st = app.store.lock().unwrap();
            let mut list: Vec<Run> = st.state.runs.clone();
            list.reverse();
            list.truncate(limit);
            (200, json!({"runs": list}))
        }
        ("POST", ["v1", "runs"]) => create_run(app, req),
        ("GET", ["v1", "runs", id]) => get_run(app, id),
        ("GET", ["v1", "runs", id, "log"]) => run_log(app, req, id),
        ("POST", ["v1", "runs", id, "stop"]) => stop_run(app, id),
        ("DELETE", ["v1", "runs", id]) => delete_run(app, id),
        ("GET", ["v1", "jobs"]) => {
            let st = app.store.lock().unwrap();
            (200, json!({"jobs": st.state.jobs}))
        }
        ("POST", ["v1", "jobs"]) => create_job(app, req),
        ("POST", ["v1", "jobs", id, "run"]) => {
            let exists = {
                let st = app.store.lock().unwrap();
                st.state.jobs.iter().any(|j| j.id == *id)
            };
            if !exists {
                return (404, json!({"error": "no such job"}));
            }
            jobs::spawn_from_job(app.clone(), id);
            (200, json!({"ok": true, "triggered": id}))
        }
        ("POST", ["v1", "jobs", id, "enable"]) => set_job_enabled(app, id, true),
        ("POST", ["v1", "jobs", id, "disable"]) => set_job_enabled(app, id, false),
        ("DELETE", ["v1", "jobs", id]) => {
            let mut st = app.store.lock().unwrap();
            let before = st.state.jobs.len();
            st.state.jobs.retain(|j| j.id != *id);
            let removed = st.state.jobs.len() != before;
            st.save();
            if removed {
                (200, json!({"ok": true, "removed": id}))
            } else {
                (404, json!({"error": "no such job"}))
            }
        }
        _ => (404, json!({"error": "not found", "path": req.path})),
    }
}

fn health(app: &Arc<App>) -> Value {
    let st = app.store.lock().unwrap();
    let running = st.state.runs.iter().filter(|r| r.status == RunStatus::Running).count();
    let queued = st.state.runs.iter().filter(|r| r.status == RunStatus::Queued).count();
    let done = st.state.runs.iter().filter(|r| r.status == RunStatus::Done).count();
    let failed = st
        .state
        .runs
        .iter()
        .filter(|r| matches!(r.status, RunStatus::Failed | RunStatus::Timeout))
        .count();
    json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "now": iso(now()),
        "uptime_s": now() - app.started,
        "runs": {"total": st.state.runs.len(), "running": running, "queued": queued, "done": done, "failed": failed},
        "jobs": {"total": st.state.jobs.len(), "enabled": st.state.jobs.iter().filter(|j| j.enabled).count()},
        "data_dir": st.dir.to_string_lossy(),
    })
}

fn create_run(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewRun = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({"error": format!("bad json: {}", e)})),
    };
    if spec.agent.trim().is_empty() || spec.prompt.trim().is_empty() {
        return (400, json!({"error": "agent and prompt are required"}));
    }
    let workspace = spec
        .workspace
        .filter(|w| !w.trim().is_empty())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".into())
        });
    let run = {
        let mut st = app.store.lock().unwrap();
        let id = st.next_id("run");
        let r = Run {
            id: id.clone(),
            title: spec.title.unwrap_or_else(|| String::new()),
            agent: spec.agent,
            model: spec.model,
            workspace,
            prompt: spec.prompt,
            status: RunStatus::Queued,
            created_at: now(),
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            timeout_s: spec.timeout_s.unwrap_or(1800),
            job_id: None,
            pid: None,
            attempts: 0,
        };
        st.state.runs.push(r.clone());
        st.save();
        r
    };
    runner::start(app.clone(), run.id.clone());
    (200, json!({"run": run}))
}

fn get_run(app: &Arc<App>, id: &str) -> (u16, Value) {
    let st = app.store.lock().unwrap();
    match st.state.runs.iter().find(|r| r.id == id) {
        Some(r) => (200, json!({"run": r})),
        None => (404, json!({"error": "no such run"})),
    }
}

fn run_log(app: &Arc<App>, req: &Req, id: &str) -> (u16, Value) {
    let tail: usize = req
        .query
        .get("tail")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let (path, exists) = {
        let st = app.store.lock().unwrap();
        let exists = st.state.runs.iter().any(|r| r.id == id);
        (st.log_path(id), exists)
    };
    if !exists {
        return (404, json!({"error": "no such run"}));
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(tail);
    let body = lines[start..].join("\n");
    let body = if body.len() > 200_000 {
        body[body.len() - 200_000..].to_string()
    } else {
        body
    };
    (200, json!({"id": id, "log": body}))
}

fn stop_run(app: &Arc<App>, id: &str) -> (u16, Value) {
    let queued = {
        let mut st = app.store.lock().unwrap();
        match st.state.runs.iter().position(|r| r.id == id) {
            None => return (404, json!({"error": "no such run"})),
            Some(i) => {
                let r = &mut st.state.runs[i];
                if r.status.terminal() {
                    return (409, json!({"error": "already finished", "status": r.status}));
                }
                let was_queued = r.status == RunStatus::Queued;
                if was_queued {
                    r.status = RunStatus::Killed;
                    r.ended_at = Some(now());
                }
                st.save();
                was_queued
            }
        }
    };
    if let Ok(c) = app.cancels.lock() {
        if let Some(flag) = c.get(id) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    (200, json!({"ok": true, "id": id, "was_queued": queued}))
}

fn delete_run(app: &Arc<App>, id: &str) -> (u16, Value) {
    let mut st = app.store.lock().unwrap();
    let pos = match st.state.runs.iter().position(|r| r.id == id) {
        Some(i) => i,
        None => return (404, json!({"error": "no such run"})),
    };
    if !st.state.runs[pos].status.terminal() {
        return (409, json!({"error": "still running, stop it first"}));
    }
    let r = st.state.runs.remove(pos);
    let _ = std::fs::remove_file(st.log_path(&r.id));
    st.save();
    (200, json!({"ok": true, "removed": id}))
}

fn create_job(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewJob = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({"error": format!("bad json: {}", e)})),
    };
    if spec.agent.trim().is_empty() || spec.prompt.trim().is_empty() {
        return (400, json!({"error": "agent and prompt are required"}));
    }
    if spec.every_secs.is_none() && spec.daily.is_none() {
        return (400, json!({"error": "one of every_secs / daily is required"}));
    }
    if let Some(d) = spec.daily.as_deref() {
        if parse_hhmm(d).is_none() {
            return (400, json!({"error": "daily must be HH:MM"}));
        }
    }
    let tz = spec.tz_offset_hours.unwrap_or(app.tz_offset_hours);
    let workspace = spec
        .workspace
        .filter(|w| !w.trim().is_empty())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".into())
        });
    let mut st = app.store.lock().unwrap();
    let id = st.next_id("job");
    let j = Job {
        id: id.clone(),
        title: spec.title.unwrap_or_else(|| String::new()),
        agent: spec.agent,
        model: spec.model,
        workspace,
        prompt: spec.prompt,
        every_secs: spec.every_secs,
        daily: spec.daily,
        tz_offset_hours: tz,
        timeout_s: spec.timeout_s.unwrap_or(3600),
        max_retries: spec.max_retries.unwrap_or(2),
        enabled: spec.enabled.unwrap_or(true),
        allow_overlap: spec.allow_overlap.unwrap_or(false),
        next_at: now(),
        last_run: None,
        last_error: None,
        attempts: 0,
    };
    st.state.jobs.push(j.clone());
    st.save();
    (200, json!({"job": j}))
}

fn set_job_enabled(app: &Arc<App>, id: &str, enabled: bool) -> (u16, Value) {
    let mut st = app.store.lock().unwrap();
    match st.state.jobs.iter_mut().find(|j| j.id == id) {
        Some(j) => {
            j.enabled = enabled;
            j.attempts = 0;
            if enabled {
                j.next_at = now();
            }
            let out = json!({"job": j});
            st.save();
            (200, out)
        }
        None => (404, json!({"error": "no such job"})),
    }
}
