use crate::jobs;
use crate::types::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub fn serve(app: Arc<App>, bind: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind)?;
    println!("roamway listening on http://{}", bind);
    for s in listener.incoming() {
        match s {
            Ok(stream) => {
                let n = app.conns.fetch_add(1, Ordering::SeqCst);
                if n >= app.max_conn {
                    app.conns.fetch_sub(1, Ordering::SeqCst);
                    let mut reject = stream;
                    let _ = respond(&mut reject, 503, &json!({"error": "too many connections"}));
                    continue;
                }
                let a = app.clone();
                thread::spawn(move || {
                    let _ = handle(a.clone(), stream);
                    a.conns.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(e) => {
                eprintln!("accept error: {}", e);
                thread::sleep(Duration::from_millis(50));
            }
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

enum ReadOutcome {
    Req(Req),
    Eof,
    HeaderTimeout,
    BodyTooLarge,
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_req(s: &mut TcpStream) -> std::io::Result<ReadOutcome> {
    const MAX_HEADER: usize = 64 * 1024;
    const MAX_BODY: usize = 1024 * 1024;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut head_end: Option<usize> = None;
    while head_end.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(ReadOutcome::HeaderTimeout);
        }
        s.set_read_timeout(Some(remaining))?;
        let n = match s.read(&mut tmp) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut
                || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                if Instant::now() >= deadline {
                    return Ok(ReadOutcome::HeaderTimeout);
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        if n == 0 {
            return Ok(ReadOutcome::Eof);
        }
        buf.extend_from_slice(&tmp[..n]);
        head_end = find_sub(&buf, b"\r\n\r\n");
        if buf.len() > MAX_HEADER {
            return Ok(ReadOutcome::Eof);
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
    if len > MAX_BODY {
        return Ok(ReadOutcome::BodyTooLarge);
    }
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    let want = len;
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < want {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(want);

    Ok(ReadOutcome::Req(Req {
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
        ReadOutcome::Req(r) => r,
        ReadOutcome::Eof => return Ok(()),
        ReadOutcome::HeaderTimeout => {
            return respond(&mut s, 408, &json!({"error": "request timeout"}))
        }
        ReadOutcome::BodyTooLarge => {
            return respond(&mut s, 413, &json!({"error": "body too large"}))
        }
    };

    // /health 免鉴权；其余路由若配置了 token 则只接受 header
    if needs_auth(&req.path) {
        if let Some(tok) = app.token.clone() {
            let got = req
                .headers
                .get("x-roamway-token")
                .cloned()
                .unwrap_or_default();
            if !ct_eq(&got, &tok) {
                return respond(&mut s, 401, &json!({"error": "unauthorized"}));
            }
        }
    }

    let (code, body) = route(&app, &req);
    respond(&mut s, code, &body)
}

/// 只有 /health 免鉴权；/v1/health 仍需鉴权
fn needs_auth(path: &str) -> bool {
    path != "/health"
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
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewRun {
    #[serde(default)]
    title: Option<String>,
    agent: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    prompt: String,
    #[serde(default, alias = "timeout_secs")]
    timeout_s: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(default, alias = "timeout_secs")]
    timeout_s: Option<u64>,
    #[serde(default)]
    max_retries: Option<u32>,
    #[serde(default)]
    allow_overlap: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
}

fn route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let segs: Vec<&str> = req
        .path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let m = req.method.as_str();
    match (m, segs.as_slice()) {
        ("GET", ["health"]) => (200, health(app)),
        ("GET", ["v1", "health"]) => (200, v1_health(app)),
        ("GET", ["v1", "runs"]) => list_runs(app, req),
        ("POST", ["v1", "runs"]) => create_run(app, req),
        ("GET", ["v1", "runs", id]) => get_run(app, id),
        ("GET", ["v1", "runs", id, "log"]) => run_log(app, req, id),
        ("POST", ["v1", "runs", id, "stop"]) => stop_run(app, id),
        ("DELETE", ["v1", "runs", id]) => delete_run(app, id),
        ("GET", ["v1", "jobs"]) => {
            let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            (200, json!({"jobs": st.state.jobs}))
        }
        ("POST", ["v1", "jobs"]) => create_job(app, req),
        ("POST", ["v1", "jobs", id, "run"]) => {
            let exists = {
                let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
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
            let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
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
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let running = st
        .state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Running)
        .count();
    let queued = st
        .state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Queued)
        .count();
    json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": now() - app.started,
        "running": running,
        "queued": queued,
    })
}

fn v1_health(app: &Arc<App>) -> Value {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let running = st
        .state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Running)
        .count();
    let queued = st
        .state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Queued)
        .count();
    let done = st
        .state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Done)
        .count();
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
        "running": running,
        "queued": queued,
        "runs": {"total": st.state.runs.len(), "running": running, "queued": queued, "done": done, "failed": failed},
        "jobs": {"total": st.state.jobs.len(), "enabled": st.state.jobs.iter().filter(|j| j.enabled).count()},
        "data_dir": st.dir.to_string_lossy(),
    })
}

fn list_runs(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let limit = req
        .query
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let offset = req
        .query
        .get("offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut list: Vec<Run> = st.state.runs.clone();
    list.reverse();
    let start = offset.min(list.len());
    let end = (start + limit).min(list.len());
    let mut page: Vec<Run> = list[start..end].to_vec();
    for r in page.iter_mut() {
        r.prompt = String::new();
    }
    let running = st
        .state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Running)
        .count();
    let queued = st
        .state
        .runs
        .iter()
        .filter(|r| r.status == RunStatus::Queued)
        .count();
    (200, json!({"runs": page, "running": running, "queued": queued}))
}

fn valid_timeout_s(t: u64) -> bool {
    (1..=21600).contains(&t)
}

fn canonicalize_dir(path: &str) -> Option<String> {
    if !Path::new(path).is_dir() {
        return None;
    }
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

fn create_run(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewRun = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({"error": format!("bad json: {}", e)})),
    };
    if !is_known_agent(&spec.agent) {
        return (400, json!({"error": "unknown agent"}));
    }
    if spec.prompt.trim().is_empty() {
        return (400, json!({"error": "prompt is required"}));
    }
    let timeout_s = spec.timeout_s.unwrap_or(1800);
    if !valid_timeout_s(timeout_s) {
        return (400, json!({"error": "timeout_s must be in 1..=21600"}));
    }
    let workspace = match spec.workspace.clone().filter(|w| !w.trim().is_empty()) {
        Some(w) => match canonicalize_dir(&w) {
            Some(abs) => abs,
            None => return (400, json!({"error": "workspace not found", "path": w})),
        },
        None => std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into()),
    };
    let run = {
        let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
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
            timeout_s,
            job_id: None,
            pid: None,
            attempts: 0,
        };
        st.state.runs.push(r.clone());
        jobs::prune(&mut st);
        st.save();
        r
    };
    jobs::drain_queued(app);
    (200, json!({"run": run}))
}

fn get_run(app: &Arc<App>, id: &str) -> (u16, Value) {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    match st.state.runs.iter().find(|r| r.id == id) {
        Some(r) => (200, json!({"run": r})),
        None => (404, json!({"error": "no such run"})),
    }
}

fn tail_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start > 0 && !s.is_char_boundary(start) {
        start -= 1;
    }
    &s[start..]
}

fn run_log(app: &Arc<App>, req: &Req, id: &str) -> (u16, Value) {
    let tail: usize = req
        .query
        .get("tail")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let (path, exists) = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
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
    let body = tail_bytes(&body, 200_000).to_string();
    (200, json!({"id": id, "log": body}))
}

fn remove_cancel(app: &Arc<App>, id: &str) {
    if let Ok(mut c) = app.cancels.lock() {
        c.remove(id);
    }
}

fn stop_run(app: &Arc<App>, id: &str) -> (u16, Value) {
    // 先写取消标志，再改状态，防止 stop 与 runner 竞争导致任务复活
    let cancel = {
        let mut c = app.cancels.lock().unwrap_or_else(|e| e.into_inner());
        c.entry(id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    };
    cancel.store(true, Ordering::Relaxed);

    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let i = match st.state.runs.iter().position(|r| r.id == id) {
        Some(i) => i,
        None => {
            drop(st);
            remove_cancel(app, id);
            return (404, json!({"error": "no such run"}));
        }
    };
    let status = st.state.runs[i].status;
    if status.terminal() {
        drop(st);
        remove_cancel(app, id);
        return (409, json!({"error": "already finished", "status": status}));
    }
    // queued：尚未 spawn，取消标志足以阻止启动，任何 agent 都直接判 killed
    if status == RunStatus::Queued {
        st.state.runs[i].status = RunStatus::Killed;
        st.state.runs[i].ended_at = Some(now());
        st.save();
        drop(st);
        remove_cancel(app, id);
        return (200, json!({"ok": true, "id": id, "was_queued": true}));
    }
    // running：api:* 直连无法中断，其余由 runner 轮询取消标志并 kill_tree
    if st.state.runs[i].agent.starts_with("api:") {
        drop(st);
        remove_cancel(app, id);
        return (409, json!({"error": "api run not cancellable"}));
    }
    st.state.runs[i].status = RunStatus::Killed;
    st.state.runs[i].ended_at = Some(now());
    st.save();
    (200, json!({"ok": true, "id": id, "was_queued": false}))
}

fn delete_run(app: &Arc<App>, id: &str) -> (u16, Value) {
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
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
    if !is_known_agent(&spec.agent) {
        return (400, json!({"error": "unknown agent"}));
    }
    if spec.prompt.trim().is_empty() {
        return (400, json!({"error": "prompt is required"}));
    }
    if spec.every_secs.is_none() && spec.daily.is_none() {
        return (400, json!({"error": "one of every_secs / daily is required"}));
    }
    if let Some(d) = spec.daily.as_deref() {
        if parse_hhmm(d).is_none() {
            return (400, json!({"error": "daily must be HH:MM"}));
        }
    }
    if let Some(e) = spec.every_secs {
        if e < 1 {
            return (400, json!({"error": "every_secs must be >= 1"}));
        }
    }
    let timeout_s = spec.timeout_s.unwrap_or(3600);
    if !valid_timeout_s(timeout_s) {
        return (400, json!({"error": "timeout_s must be in 1..=21600"}));
    }
    let tz = spec.tz_offset_hours.unwrap_or(app.tz_offset_hours);
    let workspace = match spec.workspace.clone().filter(|w| !w.trim().is_empty()) {
        Some(w) => match canonicalize_dir(&w) {
            Some(abs) => abs,
            None => return (400, json!({"error": "workspace not found", "path": w})),
        },
        None => std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into()),
    };
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
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
        timeout_s,
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
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_constant_time() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
    }

    #[test]
    fn only_health_is_public() {
        assert!(!needs_auth("/health"));
        assert!(needs_auth("/v1/health"));
        assert!(needs_auth("/v1/runs"));
        assert!(needs_auth("/"));
    }

    #[test]
    fn timeout_s_bounds() {
        assert!(!valid_timeout_s(0));
        assert!(valid_timeout_s(1));
        assert!(valid_timeout_s(1800));
        assert!(valid_timeout_s(21600));
        assert!(!valid_timeout_s(21601));
    }

    #[test]
    fn url_decode_basic() {
        assert_eq!(url_decode("a+b"), "a b");
        assert_eq!(url_decode("%E4%B8%AD"), "中");
    }

    #[test]
    fn tail_bytes_keeps_utf8_boundary() {
        let s = "中".repeat(10_000); // 30_000 bytes
        let tail = tail_bytes(&s, 200_000);
        assert_eq!(tail.len(), s.len());
        let short = tail_bytes(&s, 100);
        assert!(short.len() >= 100 && short.len() <= 102);
        assert!(short.chars().next().is_some());
        // 不会 panic 且是合法 UTF-8
        assert!(std::str::from_utf8(short.as_bytes()).is_ok());
    }

    #[test]
    fn new_run_strict_parse_and_alias() {
        let ok = serde_json::from_slice::<NewRun>(
            br#"{"agent":"shell","prompt":"p","timeout_secs":99}"#,
        )
        .unwrap();
        assert_eq!(ok.timeout_s, Some(99));
        let ok2 = serde_json::from_slice::<NewRun>(br#"{"agent":"shell","prompt":"p","timeout_s":7}"#).unwrap();
        assert_eq!(ok2.timeout_s, Some(7));
        assert!(serde_json::from_slice::<NewRun>(br#"{"agent":"shell","prompt":"p","timeout":7}"#).is_err());
        assert!(serde_json::from_slice::<NewRun>(br#"{"agent":"shell"}"#).is_err());
    }

    #[test]
    fn new_job_strict_parse_and_alias() {
        let ok = serde_json::from_slice::<NewJob>(
            br#"{"agent":"shell","prompt":"p","every_secs":60,"timeout_secs":99}"#,
        )
        .unwrap();
        assert_eq!(ok.timeout_s, Some(99));
        assert!(serde_json::from_slice::<NewJob>(
            br#"{"agent":"shell","prompt":"p","every_secs":60,"timeout":7}"#
        )
        .is_err());
    }

    #[test]
    fn canonicalize_dir_existing_and_missing() {
        let dir = std::env::temp_dir().join(format!("roamway-ws-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let abs = canonicalize_dir(&dir.to_string_lossy()).expect("existing dir");
        assert!(std::path::Path::new(&abs).is_absolute());
        let missing = std::env::temp_dir().join(format!("roamway-ws-missing-{}", now()));
        assert!(canonicalize_dir(&missing.to_string_lossy()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_runs_clamps_limit_strips_prompt_and_offsets() {
        let dir = std::env::temp_dir().join(format!("roamway-list-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mk = |id: &str| Run {
            id: id.into(),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: "secret prompt".into(),
            status: RunStatus::Done,
            created_at: 0,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
        };
        let store = Store {
            dir: dir.clone(),
            state: State {
                runs: vec![mk("a"), mk("b"), mk("c")],
                jobs: vec![],
                seq: 3,
            },
            loaded_ok: true,
        };
        let app = Arc::new(App {
            store: std::sync::Mutex::new(store),
            token: None,
            started: now(),
            cancels: std::sync::Mutex::new(HashMap::new()),
            tz_offset_hours: 8,
            max_concurrent: 4,
            max_conn: 64,
            conns: std::sync::atomic::AtomicUsize::new(0),
        });
        let req = Req {
            method: "GET".into(),
            path: "/v1/runs".into(),
            query: HashMap::from([("limit".into(), "1".into()), ("offset".into(), "1".into())]),
            headers: HashMap::new(),
            body: vec![],
        };
        let (code, body) = list_runs(&app, &req);
        assert_eq!(code, 200);
        let runs = body["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["id"].as_str(), Some("b"));
        assert_eq!(runs[0]["prompt"].as_str(), Some(""));
        assert_eq!(body["running"].as_u64(), Some(0));
        assert_eq!(body["queued"].as_u64(), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_app(dir: &std::path::Path, agent: &str, status: RunStatus) -> Arc<App> {
        let run = Run {
            id: "run_000001".into(),
            title: String::new(),
            agent: agent.into(),
            model: None,
            workspace: String::new(),
            prompt: "echo hi".into(),
            status,
            created_at: 0,
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
        };
        let store = Store {
            dir: dir.to_path_buf(),
            state: State { runs: vec![run], jobs: vec![], seq: 1 },
            loaded_ok: true,
        };
        Arc::new(App {
            store: std::sync::Mutex::new(store),
            token: None,
            started: now(),
            cancels: std::sync::Mutex::new(HashMap::new()),
            tz_offset_hours: 8,
            max_concurrent: 4,
            max_conn: 64,
            conns: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    #[test]
    fn stop_queued_run_marks_killed() {
        let dir = std::env::temp_dir().join(format!("roamway-stop-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Queued);
        let (code, body) = stop_run(&app, "run_000001");
        assert_eq!(code, 200);
        assert_eq!(body["was_queued"].as_bool(), Some(true));
        {
            let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(st.state.runs[0].status, RunStatus::Killed);
        }
        let cancel = app
            .cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("run_000001")
            .cloned();
        assert!(cancel.is_none(), "queued stop 后取消标志应清理，避免泄漏");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_queued_api_run_still_killable() {
        let dir = std::env::temp_dir().join(format!("roamway-stop-api-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "api:deepseek", RunStatus::Queued);
        let (code, body) = stop_run(&app, "run_000001");
        assert_eq!(code, 200);
        assert_eq!(body["was_queued"].as_bool(), Some(true));
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(st.state.runs[0].status, RunStatus::Killed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_running_api_run_returns_conflict() {
        let dir = std::env::temp_dir().join(format!("roamway-stop-running-api-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "api:qwen", RunStatus::Running);
        let (code, body) = stop_run(&app, "run_000001");
        assert_eq!(code, 409);
        assert_eq!(body["error"].as_str(), Some("api run not cancellable"));
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(st.state.runs[0].status, RunStatus::Running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_too_large_reads_as_413() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sender = thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            c.write_all(b"POST /v1/runs HTTP/1.1\r\nHost: x\r\nContent-Length: 2000000\r\n\r\n")
                .unwrap();
            let _ = c.shutdown(std::net::Shutdown::Write);
        });
        let (mut s, _) = listener.accept().unwrap();
        let outcome = read_req(&mut s).unwrap();
        assert!(matches!(outcome, ReadOutcome::BodyTooLarge));
        sender.join().unwrap();
    }

    #[test]
    fn header_budget_times_out_as_408() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sender = thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            let _ = c.set_nodelay(true);
            let bytes = b"GET /health HTTP/1.1\r\n";
            for i in 0..bytes.len() {
                c.write_all(&bytes[i..i + 1]).unwrap();
                c.flush().unwrap();
                thread::sleep(Duration::from_millis(20));
            }
            for _ in 0..600 {
                c.write_all(b"X").unwrap();
                c.flush().unwrap();
                thread::sleep(Duration::from_millis(20));
            }
        });
        let (mut s, _) = listener.accept().unwrap();
        let outcome = read_req(&mut s).unwrap();
        assert!(matches!(outcome, ReadOutcome::HeaderTimeout));
        sender.join().unwrap();
    }
}
