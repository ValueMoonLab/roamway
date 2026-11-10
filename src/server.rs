use crate::agents;
use crate::auth;
use crate::hosts;
use crate::hub;
use crate::jobs;
use crate::providers;
use crate::schedules;
use crate::terminals;
use crate::types::*;
use crate::usage;
use crate::workspaces;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub fn serve(app: Arc<App>, bind: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind)?;
    let actual = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| bind.to_string());
    let data_dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let _ = std::fs::create_dir_all(&data_dir);
    if let Err(e) = std::fs::write(data_dir.join("bind"), format!("{}\n", actual)) {
        eprintln!("roamway: write bind failed: {}", e);
    }
    if let Some(t) = app.token.as_deref().filter(|t| !t.is_empty()) {
        let token_path = data_dir.join("token");
        if let Err(e) = auth::write_private(&token_path, format!("{}\n", t).as_bytes()) {
            eprintln!("roamway: write token failed: {}", e);
        }
    }
    if app.tls_cert.is_some() || app.tls_key.is_some() {
        // TODO: TLS is intentionally not implemented yet. rustls-pemfile is not
        // available in the offline vendor cache; wiring rustls without it would
        // risk the default build. Serving plain HTTP keeps previous behavior.
        eprintln!("roamway: warning: --tls-cert/--tls-key requested but TLS is not implemented in this build; serving plain HTTP");
    }

    #[cfg(unix)]
    if let Some(path) = app.unix_socket.clone() {
        let listener = match bind_unix_listener(&path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("roamway: unix socket bind failed: {}", e);
                return Err(e);
            }
        };
        println!("roamway listening on unix://{}", path.display());
        let a = app.clone();
        thread::spawn(move || serve_unix(a, listener));
    }

    #[cfg(not(unix))]
    if let Some(path) = app.unix_socket.clone() {
        // 非 unix 平台没有 unix socket 实现：明确告警而不是静默忽略参数。
        // 该分支同时让字段在 not(unix) 下也被读取，否则 CI 的 RUSTFLAGS=-D warnings
        // 会把 types.rs 的 `field unix_socket is never read` 变成 error（Windows job 红）。
        eprintln!(
            "roamway: warning: --unix-socket {} is not supported on this platform; ignoring",
            path.display()
        );
    }

    println!("roamway listening on http://{}", actual);
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

#[cfg(unix)]
fn bind_unix_listener(path: &Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

#[cfg(unix)]
fn serve_unix(app: Arc<App>, listener: std::os::unix::net::UnixListener) {
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
}

struct Req {
    method: String,
    path: String,
    query: HashMap<String, String>,
    raw_query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

enum ReadOutcome {
    Req(Req),
    Eof,
    HeaderTimeout,
    BodyTooLarge,
}

trait Conn: Read + Write {
    fn set_read_timeout_opt(&self, d: Option<Duration>) -> std::io::Result<()>;
    fn set_write_timeout_opt(&self, d: Option<Duration>) -> std::io::Result<()>;
}

impl Conn for TcpStream {
    fn set_read_timeout_opt(&self, d: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(d)
    }
    fn set_write_timeout_opt(&self, d: Option<Duration>) -> std::io::Result<()> {
        self.set_write_timeout(d)
    }
}

#[cfg(unix)]
impl Conn for std::os::unix::net::UnixStream {
    fn set_read_timeout_opt(&self, d: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(d)
    }
    fn set_write_timeout_opt(&self, d: Option<Duration>) -> std::io::Result<()> {
        self.set_write_timeout(d)
    }
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_req<C: Conn>(s: &mut C) -> std::io::Result<ReadOutcome> {
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
        s.set_read_timeout_opt(Some(remaining))?;
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

    let (path, raw_query, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string(), parse_query(q)),
        None => (target.clone(), String::new(), HashMap::new()),
    };

    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY {
        return Ok(ReadOutcome::BodyTooLarge);
    }
    s.set_read_timeout_opt(Some(Duration::from_secs(30)))?;
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
        raw_query,
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

fn query_many(req: &Req, key: &str) -> Vec<String> {
    req.raw_query
        .split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (url_decode(k) == key).then(|| url_decode(v))
        })
        .collect()
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

#[derive(Debug)]
enum UiResponse {
    Redirect(String),
    Asset(u16, &'static str, &'static str),
    NotFound,
}

fn ui_response(method: &str, path: &str) -> Option<UiResponse> {
    if method != "GET" {
        return None;
    }
    if path == "/" {
        return Some(UiResponse::Redirect("/ui/".to_string()));
    }
    if path == "/ui" || path == "/ui/" {
        return Some(UiResponse::Asset(200, "text/html; charset=utf-8", crate::web::INDEX));
    }
    if let Some(name) = path.strip_prefix("/ui/") {
        return match crate::web::asset(name) {
            Some((ct, body)) => Some(UiResponse::Asset(200, ct, body)),
            None => Some(UiResponse::NotFound),
        };
    }
    None
}

fn respond_redirect<C: Conn>(s: &mut C, location: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        location
    );
    s.write_all(head.as_bytes())?;
    s.flush()
}

fn respond_static<C: Conn>(
    s: &mut C,
    code: u16,
    ct: &str,
    body: &[u8],
    extra: &[(&str, String)],
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: {}\r\nConnection: close\r\n",
        code,
        reason(code),
        ct,
        body.len(),
        crate::web::CSP
    );
    for (k, v) in extra {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()
}

/// 失败请求计数：所有 >= 400 的响应都从 `fail_json` 或 route 出口汇到这一个点，
/// 避免每加一条错误分支就漏记一次。
fn count_failed(app: &App, code: u16) {
    if code >= 400 {
        app.req_failed.fetch_add(1, Ordering::Relaxed);
    }
}

fn fail_json<C: Conn>(
    app: &App,
    s: &mut C,
    code: u16,
    v: &Value,
    extra: &[(&str, String)],
) -> std::io::Result<()> {
    count_failed(app, code);
    respond_full(s, code, v, extra)
}

/// Prometheus 标签值转义：`\` → `\\`，`"` → `\"`，换行 → `\n`。
fn esc_label(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 8);
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

fn run_status_label(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Done => "done",
        RunStatus::Failed => "failed",
        RunStatus::Timeout => "timeout",
        RunStatus::Killed => "killed",
        RunStatus::Interrupted => "interrupted",
    }
}

/// `/metrics` 的正文：Prometheus exposition 0.0.4 纯文本。
/// 只读内存态，临界区逐个短暂持有（store → schedules → terms，不嵌套），
/// 空 state 也输出合法文本（计数全 0，不含 NaN/Inf）。
fn metrics_text(app: &Arc<App>) -> String {
    const STATUSES: [RunStatus; 7] = [
        RunStatus::Queued,
        RunStatus::Running,
        RunStatus::Done,
        RunStatus::Failed,
        RunStatus::Timeout,
        RunStatus::Killed,
        RunStatus::Interrupted,
    ];
    let mut run_counts = [0u64; STATUSES.len()];
    let mut agents: BTreeMap<String, u64> = BTreeMap::new();
    let jobs_total: u64;
    let jobs_enabled: u64;
    {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        for r in st.state.runs.iter() {
            if let Some(i) = STATUSES.iter().position(|s| *s == r.status) {
                run_counts[i] += 1;
            }
        }
        for a in st.state.agents.iter() {
            *agents.entry(a.state.clone()).or_insert(0) += 1;
        }
        jobs_total = st.state.jobs.len() as u64;
        jobs_enabled = st.state.jobs.iter().filter(|j| j.enabled).count() as u64;
    }
    let (sched_total, sched_active, sched_failures) = {
        let sc = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
        let total = sc.schedules.len() as u64;
        let active = sc.schedules.iter().filter(|s| s.enabled && !s.paused).count() as u64;
        let failures: u64 = sc
            .schedules
            .iter()
            .map(|s| s.consecutive_failures as u64)
            .sum();
        (total, active, failures)
    };
    let terminals = {
        let t = app.terms.lock().unwrap_or_else(|e| e.into_inner());
        t.terminals.len() as u64
    };
    let uptime = now().saturating_sub(app.started).max(0) as u64;

    let mut out = String::with_capacity(2048);
    out.push_str("# HELP roamway_build_info Build information; value is always 1.\n");
    out.push_str("# TYPE roamway_build_info gauge\n");
    out.push_str(&format!(
        "roamway_build_info{{version=\"{}\"}} 1\n",
        esc_label(env!("CARGO_PKG_VERSION"))
    ));

    out.push_str("# HELP roamway_uptime_seconds Seconds since the daemon started.\n");
    out.push_str("# TYPE roamway_uptime_seconds gauge\n");
    out.push_str(&format!("roamway_uptime_seconds {}\n", uptime));

    out.push_str("# HELP roamway_runs Runs currently held in state, by status.\n");
    out.push_str("# TYPE roamway_runs gauge\n");
    for (i, s) in STATUSES.iter().enumerate() {
        out.push_str(&format!(
            "roamway_runs{{status=\"{}\"}} {}\n",
            run_status_label(*s),
            run_counts[i]
        ));
    }

    out.push_str("# HELP roamway_agents Sub-agent records in state, by state.\n");
    out.push_str("# TYPE roamway_agents gauge\n");
    if agents.is_empty() {
        out.push_str("roamway_agents 0\n");
    } else {
        for (state, n) in agents.iter() {
            out.push_str(&format!(
                "roamway_agents{{state=\"{}\"}} {}\n",
                esc_label(state),
                n
            ));
        }
    }

    out.push_str("# HELP roamway_jobs Jobs currently defined.\n");
    out.push_str("# TYPE roamway_jobs gauge\n");
    out.push_str(&format!("roamway_jobs {}\n", jobs_total));
    out.push_str("# HELP roamway_jobs_enabled Jobs with enabled=true.\n");
    out.push_str("# TYPE roamway_jobs_enabled gauge\n");
    out.push_str(&format!("roamway_jobs_enabled {}\n", jobs_enabled));

    out.push_str("# HELP roamway_schedules Schedules currently defined.\n");
    out.push_str("# TYPE roamway_schedules gauge\n");
    out.push_str(&format!("roamway_schedules {}\n", sched_total));
    out.push_str("# HELP roamway_schedules_paused Enabled schedules that are paused.\n");
    out.push_str("# TYPE roamway_schedules_paused gauge\n");
    out.push_str(&format!(
        "roamway_schedules_paused {}\n",
        sched_total.saturating_sub(sched_active)
    ));

    out.push_str("# HELP roamway_schedule_failures_total Sum of consecutive_failures across schedules (in-memory daemon state; resets on restart).\n");
    out.push_str("# TYPE roamway_schedule_failures_total counter\n");
    out.push_str(&format!("roamway_schedule_failures_total {}\n", sched_failures));

    out.push_str("# HELP roamway_terminals Terminal sessions held in memory.\n");
    out.push_str("# TYPE roamway_terminals gauge\n");
    out.push_str(&format!("roamway_terminals {}\n", terminals));

    out.push_str("# HELP roamway_http_concurrent_requests Connections currently open.\n");
    out.push_str("# TYPE roamway_http_concurrent_requests gauge\n");
    out.push_str(&format!(
        "roamway_http_concurrent_requests {}\n",
        app.conns.load(Ordering::Relaxed)
    ));

    out.push_str("# HELP roamway_http_requests_total HTTP requests handled since start (in-memory; resets on restart).\n");
    out.push_str("# TYPE roamway_http_requests_total counter\n");
    out.push_str(&format!(
        "roamway_http_requests_total {}\n",
        app.req_total.load(Ordering::Relaxed)
    ));

    out.push_str("# HELP roamway_http_requests_failed_total HTTP responses with status >= 400 since start (in-memory; resets on restart).\n");
    out.push_str("# TYPE roamway_http_requests_failed_total counter\n");
    out.push_str(&format!(
        "roamway_http_requests_failed_total {}\n",
        app.req_failed.load(Ordering::Relaxed)
    ));
    out
}

fn handle<C: Conn>(app: Arc<App>, mut s: C) -> std::io::Result<()> {
    let _ = s.set_read_timeout_opt(Some(Duration::from_secs(30)));
    let _ = s.set_write_timeout_opt(Some(Duration::from_secs(30)));
    // /metrics 计数器：每进一次 handle 就算一次请求（读失败也算，运维关心的是流量）。
    app.req_total.fetch_add(1, Ordering::Relaxed);
    let req = match read_req(&mut s)? {
        ReadOutcome::Req(r) => r,
        ReadOutcome::Eof => return Ok(()),
        ReadOutcome::HeaderTimeout => {
            return fail_json(&app, &mut s, 408, &json!({"error": "request timeout"}), &[])
        }
        ReadOutcome::BodyTooLarge => {
            return fail_json(&app, &mut s, 413, &json!({"error": "body too large"}), &[])
        }
    };

    let host = req.headers.get("host").map(|v| v.as_str());
    if !auth::host_allowed(&app.allow_hosts, &app.bind_host, host) {
        return fail_json(&app, &mut s, 421, &json!({"error": "misdirected request"}), &[]);
    }

    let origin = req.headers.get("origin").map(|v| v.as_str());
    let cors_extra: Vec<(&str, String)> = match auth::origin_allowed(&app.allow_origins, origin) {
        Some(allowed) => vec![
            ("Access-Control-Allow-Origin", allowed),
            ("Vary", "Origin".to_string()),
        ],
        None => vec![],
    };
    if req.method == "OPTIONS" {
        let mut extras = cors_extra;
        if !extras.is_empty() {
            extras.push(("Access-Control-Allow-Methods", "GET, POST, PATCH, DELETE, OPTIONS".to_string()));
            extras.push(("Access-Control-Allow-Headers", "content-type, x-roamway-token, x-approval".to_string()));
            extras.push(("Access-Control-Max-Age", "86400".to_string()));
        }
        return respond_preflight(&mut s, &extras);
    }

    let segs: Vec<&str> = req
        .path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // Static control-plane UI: `/` redirects to `/ui/`; `/ui/*` serves the
    // embedded assets without authentication so a browser can load the shell
    // before any API token is presented.
    if let Some(resp) = ui_response(req.method.as_str(), &req.path) {
        match resp {
            UiResponse::Redirect(location) => return respond_redirect(&mut s, &location),
            UiResponse::Asset(code, ct, body) => {
                return respond_static(&mut s, code, ct, body.as_bytes(), &[]);
            }
            UiResponse::NotFound => {
                count_failed(&app, 404);
                return respond_static(&mut s, 404, "text/plain; charset=utf-8", b"not found", &[]);
            }
        }
    }

    // SSE attach: only the events endpoint accepts a query token; everything
    // else must carry the token in the x-roamway-token header.
    let events_id = events_agent_id(&req);
    if events_id.is_some() && req.method == "GET" {
        let got = req
            .query
            .get("token")
            .cloned()
            .or_else(|| req.headers.get("x-roamway-token").cloned())
            .or_else(|| bearer_header(&req).map(|v| v.to_string()))
            .unwrap_or_default();
        match authenticate(&app, Some(&got)) {
            AuthOutcome::Reject => {
                return fail_json(&app, &mut s, 401, &json!({"error": "unauthorized"}), &[]);
            }
            AuthOutcome::Open | AuthOutcome::Identity { .. } => {}
        }
        let _ = s.set_write_timeout_opt(None);
        let _ = s.set_read_timeout_opt(None);
        stream_agent_events(&app, &mut s, events_id.as_deref().unwrap());
        return Ok(());
    }

    let public = !needs_auth(&req.path);
    let mut identity: Option<(String, Vec<String>)> = None;
    if !public {
        // 普通路径只认 header；`/metrics` 额外接受 `?token=`（Prometheus 的
        // params 抓取习惯，与 SSE 事件端点保持一致），其余路径行为不变。
        let presented = req.headers
            .get("x-roamway-token")
            .map(|v| v.as_str())
            .or_else(|| bearer_header(&req))
            .or_else(|| {
                if req.path == "/metrics" {
                    req.query.get("token").map(|v| v.as_str())
                } else {
                    None
                }
            });
        match authenticate(&app, presented) {
            AuthOutcome::Reject => {
                return fail_json(&app, &mut s, 401, &json!({"error": "unauthorized"}), &cors_extra);
            }
            AuthOutcome::Open => {}
            AuthOutcome::Identity { token_id, scopes } => {
                let need = auth::required_scope(req.method.as_str(), &segs);
                if !auth::has_scope(&scopes, need) {
                    return fail_json(
                        &app,
                        &mut s,
                        403,
                        &json!({"error": "insufficient scope", "required": need}),
                        &cors_extra,
                    );
                }
                let is_approval_meta = segs.first() == Some(&"v1") && segs.get(1) == Some(&"approvals");
                let requires_approval = auth::is_write_method(req.method.as_str())
                    && !is_approval_meta
                    && app.require_approval.iter().any(|s| s == need);
                if requires_approval {
                    let replay = req.headers.get("x-approval").map(|v| v.as_str());
                    match replay {
                        Some(aprid) => {
                            let check = {
                                let st = app.approvals.lock().unwrap_or_else(|e| e.into_inner());
                                st.verify_replay(aprid, req.method.as_str(), req.path.as_str(), &req.body)
                            };
                            if let Err(e) = check {
                                return fail_json(
                                    &app,
                                    &mut s,
                                    409,
                                    &json!({"error": e}),
                                    &cors_extra,
                                );
                            }
                        }
                        None => {
                            let rec = {
                                let mut st = app.approvals.lock().unwrap_or_else(|e| e.into_inner());
                                st.create(req.method.as_str(), req.path.as_str(), &req.body, &token_id)
                            };
                            return respond_full(
                                &mut s,
                                202,
                                &json!({"approval_id": rec.id, "status": "pending"}),
                                &cors_extra,
                            );
                        }
                    }
                }
                identity = Some((token_id, scopes));
            }
        }
    }

    // `/metrics`：Prometheus expo 文本。鉴权已在上面完成（未豁免 needs_auth），
    // 非 GET 一律 405，避免把 POST 当查询用。
    if req.path == "/metrics" {
        if req.method != "GET" {
            return fail_json(&app, &mut s, 405, &json!({"error": "method not allowed"}), &cors_extra);
        }
        let body = metrics_text(&app);
        return respond_static(
            &mut s,
            200,
            "text/plain; version=0.0.4; charset=utf-8",
            body.as_bytes(),
            &cors_extra,
        );
    }

    if req.method == "GET" && req.path == "/v1/auth/whoami" {
        let body = match &identity {
            Some((token_id, scopes)) => json!({ "token_id": token_id, "scopes": scopes }),
            None => json!({ "token_id": "anonymous", "scopes": [] }),
        };
        return respond_full(&mut s, 200, &body, &cors_extra);
    }

    let (code, body) = route(&app, &req);
    count_failed(&app, code);
    respond_full(&mut s, code, &body, &cors_extra)
}

enum AuthOutcome {
    Open,
    Reject,
    Identity { token_id: String, scopes: Vec<String> },
}

fn legacy_token_on_disk(app: &App) -> Option<String> {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::read_to_string(st.dir.join("token"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn authenticate(app: &Arc<App>, presented: Option<&str>) -> AuthOutcome {
    let has_tokens = {
        let st = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
        !st.tokens.is_empty()
    };
    if app.token.as_deref().map(|t| t.is_empty()).unwrap_or(true) && !has_tokens {
        return AuthOutcome::Open;
    }
    let got = presented.unwrap_or_default();
    let legacy_token = match app.token.as_deref().filter(|t| !t.is_empty()) {
        Some(flag_token) => legacy_token_on_disk(app).unwrap_or_else(|| flag_token.to_string()),
        None => String::new(),
    };
    if !legacy_token.is_empty() {
        if auth::ct_eq_str(&legacy_token, got) {
            return AuthOutcome::Identity {
                token_id: "legacy".to_string(),
                scopes: auth::ALL_SCOPES.iter().map(|s| s.to_string()).collect(),
            };
        }
    }
    if !got.is_empty() {
        let mut st = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rec) = st.resolve(got) {
            return AuthOutcome::Identity { token_id: rec.id, scopes: rec.scopes };
        }
    }
    AuthOutcome::Reject
}

/// Returns the agent id when the request targets /v1/agents/{id}/events.
fn events_agent_id(req: &Req) -> Option<String> {
    let segs: Vec<&str> = req
        .path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segs.len() == 4 && segs[0] == "v1" && segs[1] == "agents" && segs[3] == "events" {
        Some(segs[2].to_string())
    } else {
        None
    }
}

fn bearer_header(req: &Req) -> Option<&str> {
    req.headers
        .get("authorization")
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
}

fn agent_state(app: &Arc<App>, id: &str) -> String {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    st.state
        .agents
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.state.clone())
        .unwrap_or_else(|| "archived".into())
}

fn emit_sse<C: Conn>(s: &mut C, payload: &str) -> bool {
    let data = format!("data: {}\n\n", payload);
    s.write_all(data.as_bytes()).and_then(|_| s.flush()).is_ok()
}

fn stream_agent_events<C: Conn>(app: &Arc<App>, s: &mut C, id: &str) {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n";
    if s.write_all(head.as_bytes()).and_then(|_| s.flush()).is_err() {
        return;
    }
    let log_path = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let latest = st
            .state
            .runs
            .iter()
            .filter(|r| r.agent_id.as_deref() == Some(id))
            .max_by_key(|r| r.created_at)
            .map(|r| r.id.clone())
            .unwrap_or_else(|| id.to_string());
        st.log_path(&latest)
    };
    let mut offset: u64 = 0;
    let mut last_state = String::new();
    let started = Instant::now();
    let mut last_ping = 0u64;
    let mut last_len: u64 = 0;
    loop {
        let state = agent_state(app, id);
        if state != last_state {
            if emit_sse(s, &serde_json::to_string(&json!({"type":"state","state": state})).unwrap_or_default()) {
                last_state = state.clone();
            }
        }
        if let Ok(meta) = std::fs::metadata(&log_path) {
            let len = meta.len();
            if len < last_len || offset > len {
                offset = 0;
            }
            last_len = len;
        }
        let mut buf = [0u8; 4096];
        if let Ok(mut f) = std::fs::File::open(&log_path) {
            use std::io::{Read, Seek, SeekFrom};
            if f.seek(SeekFrom::Start(offset)).is_ok() {
                loop {
                    match f.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            offset += n as u64;
                            let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                            let ev = serde_json::to_string(&json!({"type":"log","chunk": chunk}))
                                .unwrap_or_default();
                            if !emit_sse(s, &ev) {
                                return;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        if state != "running" && state != "queued" {
            let _ = emit_sse(s, &serde_json::to_string(&json!({"type":"state","state": state})).unwrap_or_default());
            return;
        }
        let elapsed = started.elapsed().as_secs();
        if elapsed / 15 > last_ping {
            last_ping = elapsed / 15;
            if s.write_all(b": ping\n\n").and_then(|_| s.flush()).is_err() {
                return;
            }
        }
        thread::sleep(Duration::from_millis(300));
    }
}
fn needs_auth(path: &str) -> bool {
    !matches!(path, "/health" | "/v1/auth/login" | "/v1/auth/pair")
        && !path.starts_with("/v1/hub/hooks/")
}

/// 常量时间比较，避免按字节提前返回泄露 token 前缀
#[cfg(test)]
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

fn respond<C: Conn>(s: &mut C, code: u16, v: &Value) -> std::io::Result<()> {
    respond_full(s, code, v, &[])
}

fn respond_full<C: Conn>(s: &mut C, code: u16, v: &Value, extra: &[(&str, String)]) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(v).unwrap_or_else(|_| b"{}".to_vec());
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n",
        code,
        reason(code),
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(&body)?;
    s.flush()
}

fn respond_preflight<C: Conn>(s: &mut C, extra: &[(&str, String)]) -> std::io::Result<()> {
    let mut head = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n"
        .to_string();
    for (k, v) in extra {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.flush()
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        421 => "Misdirected Request",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WorktreeRequest {
    Bool(bool),
    Spec(WorktreeSpecRequest),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorktreeSpecRequest {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    new_branch: Option<String>,
    #[serde(default)]
    base: Option<String>,
}

/// 把 HTTP 上的 `"worktree"` 字段收敛成 (是否启用 worktree, WorktreeSpec)。
fn worktree_request_parts(
    req: Option<WorktreeRequest>,
) -> Result<(bool, workspaces::WorktreeSpec), String> {
    match req {
        None => Ok((false, workspaces::WorktreeSpec::default())),
        Some(WorktreeRequest::Bool(b)) => Ok((b, workspaces::WorktreeSpec::default())),
        Some(WorktreeRequest::Spec(s)) => {
            let spec = workspaces::spec_from_parts(s.slug, s.new_branch, s.base, s.branch)?;
            workspaces::validate_worktree_spec(&spec)?;
            Ok((true, spec))
        }
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
    #[serde(default)]
    options: Option<BTreeMap<String, String>>,
    #[serde(default)]
    images: Option<Vec<String>>,
    #[serde(default)]
    worktree: Option<WorktreeRequest>,
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
    /// 前置任务 id 列表（CLI `--after`，可重复）
    #[serde(default)]
    depends_on: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewClone {
    input: String,
    #[serde(default)]
    dir: Option<String>,
    #[serde(default)]
    protocol: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewWorkspace {
    name: String,
    path: String,
    #[serde(default)]
    remote: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewAgent {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    prompt: String,
    #[serde(default)]
    timeout_s: Option<u64>,
    #[serde(default)]
    worktree: Option<WorktreeRequest>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    env: Option<Vec<(String, String)>>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    worktree_mode: Option<String>,
    #[serde(default)]
    pr_number: Option<String>,
    #[serde(default)]
    options: Option<BTreeMap<String, String>>,
    #[serde(default)]
    images: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewImport {
    kind: String,
    session: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchAgent {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewSchedule {
    name: String,
    prompt: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    cron: Option<String>,
    #[serde(default)]
    every: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    max_runs: Option<u64>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    run_now: Option<bool>,
    #[serde(default)]
    overlap: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchSchedule {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    cron: Option<String>,
    #[serde(default)]
    every: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    max_runs: Option<u64>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    overlap: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueAgent {
    prompt: String,
    #[serde(default)]
    timeout_s: Option<u64>,
    #[serde(default)]
    options: Option<BTreeMap<String, String>>,
    #[serde(default)]
    images: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairBody {
    code: String,
}

fn login_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let body: LoginBody = match serde_json::from_slice(&req.body) {
        Ok(b) => b,
        Err(_) => return (400, json!({"error": "password required"})),
    };
    let ok = {
        let st = app.passwords.lock().unwrap_or_else(|e| e.into_inner());
        st.verify(&body.password)
    };
    if !ok {
        return (401, json!({"error": "invalid password"}));
    }
    let mut st = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
    let (id, plain) = st.create(
        Some("login".into()),
        auth::ALL_SCOPES.iter().map(|s| s.to_string()).collect(),
        Some(auth::LOGIN_TTL_SECS),
    );
    (200, json!({
        "token": plain,
        "token_id": id,
        "expires_at": crate::types::iso(crate::types::now() + auth::LOGIN_TTL_SECS),
    }))
}

fn pair_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let body: PairBody = match serde_json::from_slice(&req.body) {
        Ok(b) => b,
        Err(_) => return (400, json!({"error": "code required"})),
    };
    let consumed = {
        let mut st = app.pairings.lock().unwrap_or_else(|e| e.into_inner());
        st.consume(&body.code)
    };
    if let Err(e) = consumed {
        return (401, json!({"error": e}));
    }
    let mut st = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
    let (id, plain) = st.create(
        Some("pair".into()),
        auth::ALL_SCOPES.iter().map(|s| s.to_string()).collect(),
        None,
    );
    (200, json!({"token": plain, "token_id": id}))
}

fn approvals_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let all = query_true(req, "all");
    let st = app.approvals.lock().unwrap_or_else(|e| e.into_inner());
    (200, json!({"approvals": st.list(all)}))
}

fn approval_decide_route(app: &Arc<App>, req: &Req, id: &str, status: &str) -> (u16, Value) {
    let decided_by = req
        .headers
        .get("x-roamway-token")
        .map(|t| auth::sha256_hex(t.as_bytes()))
        .unwrap_or_else(|| "cli".to_string());
    let ok = {
        let mut st = app.approvals.lock().unwrap_or_else(|e| e.into_inner());
        st.decide(id, status, &decided_by)
    };
    if ok {
        (200, json!({"ok": true, "id": id, "status": status}))
    } else {
        (404, json!({"error": "no such pending approval"}))
    }
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
        ("POST", ["v1", "auth", "login"]) => login_route(app, req),
        ("POST", ["v1", "auth", "pair"]) => pair_route(app, req),
        ("GET", ["v1", "approvals"]) => approvals_route(app, req),
        ("POST", ["v1", "approvals", id, "approve"]) => approval_decide_route(app, req, id, "approved"),
        ("POST", ["v1", "approvals", id, "deny"]) => approval_decide_route(app, req, id, "denied"),
        ("GET", ["v1", "hosts"]) => list_hosts(app),
        ("POST", ["v1", "hosts"]) => create_host_route(app, req),
        ("DELETE", ["v1", "hosts", name]) => delete_host_route(app, name),
        ("POST", ["v1", "hosts", name, "probe"]) => probe_host_route(app, name),
        ("GET", ["v1", "workspaces"]) => list_workspaces_route(app),
        ("POST", ["v1", "workspaces"]) => create_workspace_route(app, req),
        ("POST", ["v1", "workspaces", "clone"]) => clone_workspace_route(app, req),
        ("DELETE", ["v1", "workspaces", name]) => remove_workspace_route(app, name),
        ("GET", ["v1", "terminals"]) => (200, json!({ "terminals": terminals::list(app) })),
        ("POST", ["v1", "terminals"]) => create_terminal_route(app, req),
        ("GET", ["v1", "terminals", id]) => get_terminal_route(app, id),
        ("DELETE", ["v1", "terminals", id]) => kill_terminal_route(app, id),
        ("GET", ["v1", "terminals", id, "output"]) => terminal_output_route(app, req, id),
        ("POST", ["v1", "terminals", id, "keys"]) => terminal_keys_route(app, req, id),
        ("GET", ["v1", "scripts"]) => (200, json!({ "scripts": terminals::scripts(app) })),
        ("POST", ["v1", "scripts"]) => create_script_route(app, req),
        ("DELETE", ["v1", "scripts", name]) => remove_script_route(app, name),
        ("POST", ["v1", "scripts", name, "start"]) => script_action_route(app, name, true),
        ("POST", ["v1", "scripts", name, "stop"]) => script_action_route(app, name, false),
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
            // 手工触发显式绕过依赖；依赖未满足时在响应里标注，便于脚本/CLI 提示
            let blocked = {
                let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
                match st.state.jobs.iter().find(|j| j.id == *id) {
                    None => return (404, json!({"error": "no such job"})),
                    Some(j) => jobs::dep_block(&st.state, j),
                }
            };
            jobs::spawn_from_job(app.clone(), id);
            let mut out = json!({"ok": true, "triggered": id});
            if blocked.is_some() {
                out["deps_bypassed"] = json!(true);
            }
            (200, out)
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
        ("GET", ["v1", "schedules"]) => list_schedules(app, req),
        ("POST", ["v1", "schedules"]) => create_schedule(app, req),
        ("GET", ["v1", "schedules", id]) => get_schedule(app, id),
        ("PATCH", ["v1", "schedules", id]) => patch_schedule(app, id, req),
        ("DELETE", ["v1", "schedules", id]) => delete_schedule(app, id),
        ("POST", ["v1", "schedules", id, "run"]) => run_schedule(app, id),
        ("POST", ["v1", "schedules", id, "pause"]) => pause_schedule(app, id),
        ("POST", ["v1", "schedules", id, "resume"]) => resume_schedule(app, id),
        ("GET", ["v1", "schedules", id, "logs"]) => schedule_logs(app, req, id),
        ("GET", ["v1", "status"]) => status_route(app),
        ("GET", ["v1", "usage"]) => usage_route(app),
        ("GET", ["v1", "providers"]) => providers_route(app),
        ("GET", ["v1", "tokens"]) => list_tokens_route(app),
        ("POST", ["v1", "tokens"]) => create_token_route(app, req),
        ("DELETE", ["v1", "tokens", id]) => revoke_token_route(app, id),
        ("GET", ["v1", "hooks"]) => hooks_route(app),
        ("GET", ["v1", "hub"]) => hub_overview_route(app),
        ("GET", ["v1", "hub", "triggers"]) => hub_triggers_route(app),
        ("POST", ["v1", "hub", "triggers", id, "enable"]) => hub_trigger_enabled_route(app, id, true),
        ("POST", ["v1", "hub", "triggers", id, "disable"]) => hub_trigger_enabled_route(app, id, false),
        ("POST", ["v1", "hub", "hooks", trigger_id]) => hub_hook_route(app, req, trigger_id),
        ("GET", ["v1", "agents"]) => list_agents_route(app, req),
        ("POST", ["v1", "agents"]) => create_agent_route(app, req),
        ("POST", ["v1", "agents", "import"]) => import_agent_route(app, req),
        ("GET", ["v1", "agents", id, "diff"]) => agent_diff_route(app, id),
        ("GET", ["v1", "agents", id, "session"]) => agent_session_route(app, id),
        ("GET", ["v1", "agents", id]) => get_agent_route(app, id),
        ("PATCH", ["v1", "agents", id]) => patch_agent_route(app, id, req),
        ("DELETE", ["v1", "agents", id]) => archive_agent_route(app, id, req),
        ("POST", ["v1", "agents", id, "reload"]) => reload_agent_route(app, id),
        ("POST", ["v1", "agents", id, "run"]) => continue_agent_route(app, id, req),
        ("POST", ["v1", "agents", id, "detach"]) => detach_agent_route(app, id),
        ("GET", ["v1", "agents", _id, "events"]) => (405, json!({"error": "events must be streamed"})),
        _ => (404, json!({"error": "not found", "path": req.path})),
    }
}

fn list_schedules(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let all = query_true(req, "all");
    let ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    let schedules: Vec<&schedules::Schedule> = ss
        .schedules
        .iter()
        .filter(|s| all || s.enabled)
        .collect();
    (200, json!({ "schedules": schedules }))
}

fn validate_schedule_input(
    app: &Arc<App>,
    cron: Option<&str>,
    every: Option<&str>,
    timezone: Option<&str>,
) -> Result<(String, u64, String), String> {
    let cron = cron.map(|s| s.trim().to_string()).unwrap_or_default();
    let every_secs = match every {
        Some(e) => schedules::parse_every(e).ok_or_else(|| "every must be like 30s|5m|2h|1d".to_string())?,
        None => 0,
    };
    if every_secs > 0 && !cron.is_empty() {
        return Err("provide either cron or every, not both".into());
    }
    if every_secs == 0 && cron.is_empty() {
        return Err("one of cron / every is required".into());
    }
    let tz = timezone
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| schedules::offset_to_tz(app.tz_offset_hours));
    let offset = crate::cron::timezone_offset_secs(&tz)
        .map_err(|e| format!("timezone '{}': {}", tz, e))?;
    if every_secs == 0 {
        let expr = crate::cron::parse_cron(&cron).map_err(|e| e)?;
        if expr.next_after(now(), offset).is_none() {
            return Err("cron never matches within 366 days".into());
        }
    }
    Ok((cron, every_secs, tz))
}

fn create_schedule(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewSchedule = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    let name = spec.name.trim().to_string();
    if name.is_empty() {
        return (400, json!({ "error": "name is required" }));
    }
    if spec.prompt.trim().is_empty() {
        return (400, json!({ "error": "prompt is required" }));
    }
    let agent = spec.agent.clone().unwrap_or_else(|| "shell".into());
    if !is_known_agent(&agent) {
        return (400, json!({ "error": "unknown agent" }));
    }
    let (cron, every_secs, tz) =
        match validate_schedule_input(app, spec.cron.as_deref(), spec.every.as_deref(), spec.timezone.as_deref()) {
            Ok(v) => v,
            Err(e) => return (400, json!({ "error": e })),
        };
    let workspace = match spec.workspace.clone().filter(|w| !w.trim().is_empty()) {
        Some(w) => {
            let resolved = resolve_workspace_input(app, &w);
            match canonicalize_dir(&resolved) {
                Some(abs) => abs,
                None => return (400, json!({ "error": "workspace not found", "path": w })),
            }
        }
        None => std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into()),
    };
    let max_runs = spec.max_runs.unwrap_or(0);
    let expires_at = spec.expires_in.map(|s| now() + s);
    let overlap = spec.overlap.unwrap_or(false);
    let t = now();
    let next_run_at = if every_secs > 0 {
        Some(t + every_secs as i64)
    } else {
        crate::cron::parse_cron(&cron)
            .ok()
            .and_then(|c| c.next_after(t, crate::cron::timezone_offset_secs(&tz).unwrap_or(0)))
    };
    let schedule = schedules::Schedule {
        id: schedules::gen_schedule_id(),
        name,
        agent_kind: agent,
        prompt: spec.prompt,
        workspace,
        model: spec.model.unwrap_or_default(),
        cron,
        every_secs,
        timezone: tz,
        enabled: true,
        paused: false,
        max_runs,
        runs_count: 0,
        expires_at,
        created_at: t,
        next_run_at,
        last_run_at: None,
        last_status: String::new(),
        consecutive_failures: 0,
        overlap,
    };
    {
        let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
        if ss.schedules.iter().any(|s| s.name == schedule.name) {
            return (409, json!({ "error": "schedule name already exists" }));
        }
        ss.schedules.push(schedule.clone());
        ss.save();
    }
    if spec.run_now.unwrap_or(false) {
        let _ = schedules::run_once(app, &schedule.id);
    }
    (200, json!({ "schedule": schedule }))
}

fn get_schedule(app: &Arc<App>, id: &str) -> (u16, Value) {
    let (schedule, runs) = {
        let ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = schedules::find(&ss, id) else {
            return (404, json!({ "error": "no such schedule" }));
        };
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let mut runs: Vec<&Run> = st
            .state
            .runs
            .iter()
            .filter(|r| r.job_id.as_deref() == Some(s.id.as_str()))
            .collect();
        runs.sort_by(|x, y| y.created_at.cmp(&x.created_at));
        let runs: Vec<Value> = runs.into_iter().take(5).map(agent_run_summary).collect();
        (s.clone(), runs)
    };
    (200, json!({ "schedule": schedule, "runs": runs }))
}

fn patch_schedule(app: &Arc<App>, id: &str, req: &Req) -> (u16, Value) {
    let spec: PatchSchedule = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    // 重名检查必须在拿可变借用之前做完，否则 E0502
    let target_id = match schedules::find(&ss, id) {
        Some(s) => s.id.clone(),
        None => return (404, json!({ "error": "no such schedule" })),
    };
    if let Some(n) = spec.name.as_deref() {
        let n = n.trim().to_string();
        if n.is_empty() {
            return (400, json!({ "error": "name must not be empty" }));
        }
        if ss.schedules.iter().any(|x| x.id != target_id && x.name == n) {
            return (409, json!({ "error": "schedule name already exists" }));
        }
    }
    let Some(s) = schedules::find_mut(&mut ss, &target_id) else {
        return (404, json!({ "error": "no such schedule" }));
    };
    if let Some(n) = spec.name {
        s.name = n.trim().to_string();
    }
    if let Some(a) = spec.agent {
        if !is_known_agent(&a) {
            return (400, json!({ "error": "unknown agent" }));
        }
        s.agent_kind = a;
    }
    if let Some(p) = spec.prompt {
        if p.trim().is_empty() {
            return (400, json!({ "error": "prompt must not be empty" }));
        }
        s.prompt = p;
    }
    if let Some(m) = spec.model {
        s.model = m;
    }
    if let Some(w) = spec.workspace {
        let resolved = resolve_workspace_input(app, &w);
        match canonicalize_dir(&resolved) {
            Some(abs) => s.workspace = abs,
            None => return (400, json!({ "error": "workspace not found", "path": w })),
        }
    }
    if let Some(mr) = spec.max_runs {
        s.max_runs = mr;
    }
    if let Some(ei) = spec.expires_in {
        s.expires_at = Some(now() + ei);
    }
    if let Some(ov) = spec.overlap {
        s.overlap = ov;
    }
    if let Some(en) = spec.enabled {
        s.enabled = en;
    }
    let cron = spec.cron.as_deref();
    let every = spec.every.as_deref();
    let tz = spec.timezone.as_deref();
    if cron.is_some() || every.is_some() || tz.is_some() {
        let current_cron = if cron.is_some() {
            cron.map(|x| x.trim().to_string())
        } else if every.is_some() {
            None
        } else {
            Some(s.cron.clone())
        };
        let current_every = if every.is_some() {
            every.map(|x| x.trim().to_string())
        } else if cron.is_some() {
            None
        } else if s.every_secs > 0 {
            Some(format!("{}s", s.every_secs))
        } else {
            None
        };
        let current_tz = tz.map(|x| x.trim().to_string()).or_else(|| Some(s.timezone.clone()));
        let (new_cron, new_every, new_tz) = match validate_schedule_input(
            app,
            current_cron.as_deref(),
            current_every.as_deref(),
            current_tz.as_deref(),
        ) {
            Ok(v) => v,
            Err(e) => return (400, json!({ "error": e })),
        };
        s.cron = new_cron;
        s.every_secs = new_every;
        s.timezone = new_tz;
        if s.enabled && !s.paused {
            s.next_run_at = schedules::schedule_next_after(s, now());
        }
    }
    let out = s.clone();
    ss.save();
    (200, json!({ "schedule": out }))
}

fn delete_schedule(app: &Arc<App>, id: &str) -> (u16, Value) {
    match schedules::delete(app, id) {
        Ok(out) => (200, json!({ "ok": true, "removed": out })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn run_schedule(app: &Arc<App>, id: &str) -> (u16, Value) {
    match schedules::run_once(app, id) {
        Ok(sid) => (200, json!({ "ok": true, "triggered": sid })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn pause_schedule(app: &Arc<App>, id: &str) -> (u16, Value) {
    match schedules::pause(app, id) {
        Ok(s) => (200, json!({ "ok": true, "schedule": s })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn resume_schedule(app: &Arc<App>, id: &str) -> (u16, Value) {
    match schedules::resume(app, id) {
        Ok(s) => (200, json!({ "ok": true, "schedule": s })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn schedule_logs(app: &Arc<App>, req: &Req, id: &str) -> (u16, Value) {
    let tail = req
        .query
        .get("tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 200);
    let schedule = {
        let ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
        match schedules::find(&ss, id) {
            Some(s) => s.clone(),
            None => return (404, json!({ "error": "no such schedule" })),
        }
    };
    let mut runs: Vec<Run> = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.state
            .runs
            .iter()
            .filter(|r| r.job_id.as_deref() == Some(schedule.id.as_str()))
            .cloned()
            .collect()
    };
    runs.sort_by(|x, y| y.created_at.cmp(&x.created_at));
    runs.truncate(tail);
    let mut logs = String::new();
    for r in runs.iter().rev() {
        let lp = {
            let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            st.log_path(&r.id)
        };
        if let Ok(text) = std::fs::read_to_string(&lp) {
            if !logs.is_empty() {
                logs.push_str("\n---\n");
            }
            logs.push_str(&format!("== {} ==\n{}", r.id, text));
        }
    }
    let runs: Vec<Value> = runs.iter().map(agent_run_summary).collect();
    (200, json!({ "schedule": schedule, "tail": tail, "runs": runs, "logs": logs }))
}

fn status_route(app: &Arc<App>) -> (u16, Value) {
    let (running, queued) = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let running = st.state.runs.iter().filter(|r| r.status == RunStatus::Running).count();
        let queued = st.state.runs.iter().filter(|r| r.status == RunStatus::Queued).count();
        (running, queued)
    };
    let ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    let enabled: Vec<&schedules::Schedule> = ss.schedules.iter().filter(|s| s.enabled && !s.paused).collect();
    let next_run_at = enabled.iter().filter_map(|s| s.next_run_at).min();
    let last_failure = ss
        .schedules
        .iter()
        .filter_map(|s| {
            if s.consecutive_failures > 0 {
                s.last_run_at
            } else {
                None
            }
        })
        .max();
    (200, json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime": now() - app.started,
        "running": running,
        "queued": queued,
        "max_concurrent": app.max_concurrent,
        "schedules": ss.schedules.len(),
        "enabled_schedules": enabled.len(),
        "next_run_at": next_run_at,
        "last_failure": last_failure,
    }))
}

fn hooks_route(app: &Arc<App>) -> (u16, Value) {
    let ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    (200, json!({
        "config": {
            "schedule_trigger": ["run"],
            "agent_lifecycle": ["queued", "running", "done", "failed", "timeout"]
        },
        "records": ss.hooks,
    }))
}
fn hub_data_dir(app: &Arc<App>) -> std::path::PathBuf {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    st.dir.clone()
}

fn hub_overview_route(app: &Arc<App>) -> (u16, Value) {
    let dir = hub_data_dir(app);
    let hub = match hub::load(&dir) {
        Some(h) => h,
        None => return (404, json!({"error": "hub not initialized"})),
    };
    let enabled = hub.triggers.iter().filter(|t| t.enabled).count();
    let recent = {
        let rt = app.hub_rt.lock().unwrap_or_else(|e| e.into_inner());
        rt.recent()
    };
    (200, json!({
        "hub_id": hub.hub_id,
        "name": hub.name,
        "created_at": hub.created_at,
        "projects": hub.projects.len(),
        "permissions": hub.permissions.len(),
        "triggers": hub.triggers.len(),
        "triggers_enabled": enabled,
        "triggers_disabled": hub.triggers.len() - enabled,
        "recent": recent,
    }))
}

fn hub_triggers_route(app: &Arc<App>) -> (u16, Value) {
    let dir = hub_data_dir(app);
    let hub = match hub::load(&dir) {
        Some(h) => h,
        None => return (404, json!({"error": "hub not initialized"})),
    };
    (200, json!({"triggers": hub.triggers}))
}

fn hub_trigger_enabled_route(app: &Arc<App>, id: &str, enabled: bool) -> (u16, Value) {
    let dir = hub_data_dir(app);
    let mut hub = match hub::load(&dir) {
        Some(h) => h,
        None => return (404, json!({"error": "hub not initialized"})),
    };
    match hub.triggers.iter_mut().find(|t| t.id == id) {
        Some(t) => {
            t.enabled = enabled;
            match hub::save(&dir, &hub) {
                Ok(()) => (200, json!({"ok": true, "id": id, "enabled": enabled})),
                Err(e) => (500, json!({"error": e})),
            }
        }
        None => (404, json!({"error": "no such trigger"})),
    }
}

fn hub_hook_route(app: &Arc<App>, req: &Req, trigger_id: &str) -> (u16, Value) {
    let dir = hub_data_dir(app);
    let hub = match hub::load(&dir) {
        Some(h) => h,
        None => return (404, json!({"error": "hub not initialized"})),
    };
    let trigger = match hub.triggers.iter().find(|t| t.id == trigger_id) {
        Some(t) => t,
        None => return (404, json!({"error": "no such trigger"})),
    };
    if !trigger.enabled {
        return (403, json!({"error": "trigger disabled"}));
    }
    let ip = req
        .headers
        .get("x-forwarded-for")
        .or_else(|| req.headers.get("x-real-ip"))
        .map(|v| v.as_str())
        .unwrap_or("");
    let source = hub::mask_ip(ip);

    let secret = match std::env::var(&trigger.secret_env) {
        Ok(s) if !s.is_empty() => s,
        _ => return (403, json!({"error": "missing webhook secret"})),
    };
    let sig = req
        .headers
        .get("x-hub-signature-256")
        .map(|v| v.as_str())
        .unwrap_or("");
    if let Err(e) = hub::verify_signature(secret.as_bytes(), &req.body, sig) {
        let mut rt = app.hub_rt.lock().unwrap_or_else(|e| e.into_inner());
        rt.record_activity(source.clone(), format!("auth_failed:{}", e));
        return (403, json!({"error": "bad signature"}));
    }

    let github_event = req.headers.get("x-github-event").map(|v| v.as_str());
    if !hub::event_matches(trigger, github_event) {
        return (200, json!({"skipped": "event"}));
    }

    let delivery = req.headers.get("x-github-delivery").map(|v| v.as_str());
    {
        let mut rt = app.hub_rt.lock().unwrap_or_else(|e| e.into_inner());
        if rt.check_delivery(delivery) {
            return (200, json!({"duplicate": true}));
        }
    }

    let busy = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let running = st.state.runs.iter().filter(|r| r.status == RunStatus::Running).count();
        let queued = st.state.runs.iter().filter(|r| r.status == RunStatus::Queued).count();
        running + queued >= app.max_concurrent
    };
    if busy {
        let mut rt = app.hub_rt.lock().unwrap_or_else(|e| e.into_inner());
        rt.record_activity(source.clone(), "skipped:busy".into());
        return (429, json!({"error": "busy", "skipped": "busy"}));
    }

    let spec = trigger.agent_spec.clone();
    match agents::create_agent_and_run(app, &spec.provider, None, Some(spec.cwd.clone()), None, &spec.prompt, 1800, false, None, None, BTreeMap::new(), Vec::new(), None, BTreeMap::new(), Vec::new(), workspaces::WorktreeSpec::default()) {
        Ok((_agent, run)) => {
            agents::tag_run_trigger(app, &run.id, trigger_id);
            let mut rt = app.hub_rt.lock().unwrap_or_else(|e| e.into_inner());
            rt.record_activity(source, format!("run:{}", run.id));
            (202, json!({"run_id": run.id}))
        }
        Err(e) => {
            let mut rt = app.hub_rt.lock().unwrap_or_else(|e| e.into_inner());
            rt.record_activity(source, format!("failed:{}", e));
            (400, json!({"error": e}))
        }
    }
}
fn query_true(req: &Req, key: &str) -> bool {
    req.query
        .get(key)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn agent_run_summary(r: &Run) -> Value {
    json!({
        "id": r.id,
        "title": r.title,
        "agent": r.agent,
        "status": r.status,
        "created_at": r.created_at,
        "started_at": r.started_at,
        "ended_at": r.ended_at,
        "exit_code": r.exit_code,
        "error": r.error,
        "worktree": r.worktree,
    })
}

fn list_agents_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let all = query_true(req, "all");
    let labels = query_many(req, "label");
    let kind = req
        .query
        .get("kind")
        .filter(|s| !s.is_empty())
        .map(String::as_str);
    let limit = req
        .query
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 500));
    let agents: Vec<&agents::Agent> =
        agents::filter_agents(&st.state.agents, all, &labels, kind, limit);
    (200, json!({ "agents": agents }))
}

fn get_agent_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    match st.state.agents.iter().find(|a| a.id == id) {
        Some(a) => {
            let mut runs: Vec<&Run> = st
                .state
                .runs
                .iter()
                .filter(|r| r.agent_id.as_deref() == Some(a.id.as_str()))
                .collect();
            runs.sort_by(|x, y| y.created_at.cmp(&x.created_at));
            let runs: Vec<Value> = runs.into_iter().map(agent_run_summary).collect();
            (200, json!({ "agent": a, "runs": runs, "resumable": agents::is_resumable_kind(&a.kind) }))
        }
        None => (404, json!({ "error": "no such agent" })),
    }
}

fn agent_session_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    match st.state.agents.iter().find(|a| a.id == id) {
        Some(a) => {
            let has_session = !a.session_id.trim().is_empty();
            (200, json!({
                "agent_id": a.id,
                "kind": a.kind,
                "resumable": agents::is_resumable_kind(&a.kind),
                "has_session": has_session,
                "session_id": a.session_id,
                "state": a.state,
                "last_error": a.last_error,
                "last_run_id": a.last_run_id,
            }))
        }
        None => (404, json!({"error": "no such agent"})),
    }
}

fn create_agent_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let raw: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    if raw.get("images").map(|v| !v.is_array()).unwrap_or(false) {
        return (400, json!({ "error": "images must be an array of paths" }));
    }
    if raw.get("options").map(|v| !v.is_object()).unwrap_or(false) {
        return (400, json!({ "error": "options must be an object" }));
    }
    let spec: NewAgent = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    let kind = spec.kind.unwrap_or_else(|| "shell".into());
    if !is_known_agent(&kind) {
        return (400, json!({ "error": "unknown agent" }));
    }
    if spec.prompt.trim().is_empty() {
        return (400, json!({ "error": "prompt is required" }));
    }
    let timeout_s = spec.timeout_s.unwrap_or(1800);
    if !valid_timeout_s(timeout_s) {
        return (400, json!({ "error": "timeout_s must be in 1..=21600" }));
    }
    let workspace = spec
        .workspace
        .clone()
        .filter(|w| !w.trim().is_empty())
        .map(|w| resolve_workspace_input(app, &w));
    let labels = spec.labels.unwrap_or_default();
    let env = spec.env.unwrap_or_default();
    if let Some((key, _)) = env.iter().find(|(k, _)| !is_valid_env_key(k)) {
        return (400, json!({ "error": format!("invalid env key: {}", key) }));
    }
    let cwd = match spec.cwd.clone().filter(|d| !d.trim().is_empty()) {
        Some(d) => match canonicalize_dir(&d) {
            Some(c) => Some(c),
            None => return (400, json!({ "error": format!("cwd not found: {}", d) })),
        },
        None => None,
    };
    let options = match crate::validate_run_options(&spec.options.unwrap_or_default()) {
        Ok(o) => o,
        Err(e) => return (400, json!({ "error": e })),
    };
    let images = spec.images.unwrap_or_default();
    for img in &images {
        if let Err(e) = crate::validate_image_path(img) {
            return (400, json!({ "error": e }));
        }
    }
    if let Err(e) = crate::runner::provider_args(
        &kind,
        &options,
        &images,
        options.get("mode").map(String::as_str),
    ) {
        return (400, json!({ "error": e }));
    }
    let (use_worktree, worktree_spec) = match worktree_request_parts(spec.worktree) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": e })),
    };
    if worktree_spec.slug.is_some() && !use_worktree {
        return (400, json!({ "error": "run --worktree-slug requires --new-workspace worktree" }));
    }
    let worktree_mode_for_base = spec.worktree_mode.clone().unwrap_or_else(|| "branch-off".into());
    if worktree_spec.base.is_some() && worktree_mode_for_base != "branch-off" {
        return (400, json!({ "error": "run --base: only valid with branch-off" }));
    }
    match agents::create_agent_and_run(
        app,
        &kind,
        spec.model,
        workspace,
        spec.title,
        &spec.prompt,
        timeout_s,
        use_worktree,
        spec.worktree_mode,
        spec.pr_number,
        labels,
        env,
        cwd,
        options,
        images,
        worktree_spec,
    ) {
        Ok((agent, run)) => (200, json!({ "agent": agent, "run": run })),
        Err(e) => (400, json!({ "error": e })),
    }
}

fn import_agent_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewImport = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    match agents::import_agent(
        app,
        &spec.kind,
        &spec.session,
        spec.title,
        spec.model,
        spec.workspace,
    ) {
        Ok(agent) => (200, json!({ "agent": agent })),
        Err(e) => (400, json!({ "error": e })),
    }
}

fn patch_agent_route(app: &Arc<App>, id: &str, req: &Req) -> (u16, Value) {
    let spec: PatchAgent = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    if spec.title.is_none() && spec.model.is_none() && spec.workspace.is_none() {
        return (400, json!({ "error": "nothing to update" }));
    }
    match agents::update_agent_meta(app, id, spec.title, spec.model, spec.workspace) {
        Ok(agent) => (200, json!({ "agent": agent })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn archive_agent_route(app: &Arc<App>, id: &str, req: &Req) -> (u16, Value) {
    let keep_worktree = query_true(req, "keep-worktree");
    match agents::archive_agent(app, id, keep_worktree) {
        Ok(()) => (200, json!({ "ok": true, "archived": id, "keep_worktree": keep_worktree })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn reload_agent_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    let exists = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.state.agents.iter().any(|a| a.id == id)
    };
    if !exists {
        return (404, json!({ "error": "no such agent" }));
    }
    match agents::reload_agent(app, id, 1800) {
        Ok(run) => {
            let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            let agent = st.state.agents.iter().find(|a| a.id == id).cloned();
            (200, json!({ "agent": agent, "run": run }))
        }
        Err(e) => (400, json!({ "error": e })),
    }
}

fn detach_agent_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    match agents::detach_agent(app, id) {
        Ok(detached) => (200, json!({ "ok": true, "id": id, "detached": detached })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn continue_agent_route(app: &Arc<App>, id: &str, req: &Req) -> (u16, Value) {
    let raw: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    if raw.get("images").map(|v| !v.is_array()).unwrap_or(false) {
        return (400, json!({ "error": "images must be an array of paths" }));
    }
    if raw.get("options").map(|v| !v.is_object()).unwrap_or(false) {
        return (400, json!({ "error": "options must be an object" }));
    }
    let spec: ContinueAgent = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    if spec.prompt.trim().is_empty() {
        return (400, json!({ "error": "prompt is required" }));
    }
    let timeout_s = spec.timeout_s.unwrap_or(1800);
    if !valid_timeout_s(timeout_s) {
        return (400, json!({ "error": "timeout_s must be in 1..=21600" }));
    }
    let options = match crate::validate_run_options(&spec.options.unwrap_or_default()) {
        Ok(o) => o,
        Err(e) => return (400, json!({ "error": e })),
    };
    let images = spec.images.unwrap_or_default();
    for img in &images {
        if let Err(e) = crate::validate_image_path(img) {
            return (400, json!({ "error": e }));
        }
    }
    let exists = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.state.agents.iter().any(|a| a.id == id)
    };
    if !exists {
        return (404, json!({ "error": "no such agent" }));
    }
    let agent_kind = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.state
            .agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.kind.clone())
            .unwrap_or_default()
    };
    if let Err(e) = crate::runner::provider_args(
        &agent_kind,
        &options,
        &images,
        options.get("mode").map(String::as_str),
    ) {
        return (400, json!({ "error": e }));
    }
    match agents::queue_agent_run(app, id, &spec.prompt, timeout_s, options, images) {
        Ok(run) => {
            let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            let agent = st.state.agents.iter().find(|a| a.id == id).cloned();
            (200, json!({ "agent": agent, "run": run }))
        }
        Err(e) => (400, json!({ "error": e })),
    }
}
fn list_hosts(app: &Arc<App>) -> (u16, Value) {
    let dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let reg = hosts::Registry::load(&dir);
    let hosts: Vec<Value> = reg
        .hosts
        .iter()
        .map(|h| {
            json!({
                "id": h.id,
                "name": h.name,
                "endpoint": h.endpoint,
                "token": hosts::mask(&h.token),
                "enabled": h.enabled,
                "created_at": h.created_at,
                "last_probe": h.last_probe.as_ref().map(|p| json!({
                    "at": p.at,
                    "ok": p.ok,
                    "latency_ms": p.latency_ms,
                    "version": p.version,
                    "error": p.error,
                })),
                "failures": h.failures,
                "next_retry_at": h.next_retry_at,
            })
        })
        .collect();
    (200, json!({ "hosts": hosts }))
}

fn host_json(h: &hosts::Host) -> Value {
    json!({
        "id": h.id,
        "name": h.name,
        "endpoint": h.endpoint,
        "token": hosts::mask(&h.token),
        "enabled": h.enabled,
        "created_at": h.created_at,
        "last_probe": h.last_probe.as_ref().map(|p| json!({
            "at": p.at,
            "ok": p.ok,
            "latency_ms": p.latency_ms,
            "version": p.version,
            "error": p.error,
        })),
        "failures": h.failures,
        "next_retry_at": h.next_retry_at,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewHost {
    name: String,
    endpoint: String,
    #[serde(default)]
    token: String,
}

fn create_host_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewHost = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    let dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let mut reg = hosts::Registry::load(&dir);
    let added = match reg.add(spec.name.trim(), spec.endpoint.trim(), spec.token.trim()) {
        Ok(h) => h.clone(),
        Err(e) => return (400, json!({ "error": e })),
    };
    if let Err(e) = reg.save() {
        return (500, json!({ "error": format!("save failed: {}", e) }));
    }
    (201, json!({ "host": host_json(&added) }))
}

fn delete_host_route(app: &Arc<App>, name: &str) -> (u16, Value) {
    let dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let mut reg = hosts::Registry::load(&dir);
    if !reg.remove(name) {
        return (404, json!({ "error": "no such host" }));
    }
    if let Err(e) = reg.save() {
        return (500, json!({ "error": format!("save failed: {}", e) }));
    }
    (200, json!({ "ok": true, "removed": name }))
}

fn probe_host_route(app: &Arc<App>, name: &str) -> (u16, Value) {
    let dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let mut reg = hosts::Registry::load(&dir);
    let idx = match reg.hosts.iter().position(|h| h.name == name) {
        Some(i) => i,
        None => return (404, json!({ "error": "no such host" })),
    };
    let p = hosts::probe(&reg.hosts[idx]);
    if p.error != "skipped" {
        let host = &mut reg.hosts[idx];
        host.last_probe = Some(p.clone());
        if p.ok {
            host.failures = 0;
            host.next_retry_at = None;
        } else {
            host.failures += 1;
            let next = crate::types::now() + hosts::backoff(host.failures);
            host.next_retry_at = Some(next);
        }
        if let Err(e) = reg.save() {
            return (500, json!({ "error": format!("save failed: {}", e) }));
        }
    }
    let host = reg.hosts[idx].clone();
    (200, json!({
        "host": host_json(&host),
        "ok": p.ok,
        "latency_ms": p.latency_ms,
        "version": p.version,
        "error": p.error,
    }))
}

fn usage_route(app: &Arc<App>) -> (u16, Value) {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let state = &st.state;
    let mut report = usage::report_json(state);
    let rows = usage::aggregate(state, 0, None);
    let by_day: Vec<Value> = rows
        .iter()
        .map(|r| json!({
            "day": r.day,
            "provider": r.provider,
            "model": r.model,
            "runs": r.runs,
            "in_tokens": r.in_tokens,
            "out_tokens": r.out_tokens,
            "total_tokens": r.total_tokens,
        }))
        .collect();
    if let Some(obj) = report.as_object_mut() {
        obj.insert("by_day".into(), json!(by_day));
    }
    (200, report)
}

fn providers_route(app: &Arc<App>) -> (u16, Value) {
    let dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    (200, json!({ "providers": providers::load(&dir) }))
}

fn list_tokens_route(app: &Arc<App>) -> (u16, Value) {
    let st = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
    (200, json!({ "tokens": st.list() }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewToken {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    ttl_secs: Option<i64>,
}

fn create_token_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewToken = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    let scopes = if spec.scopes.is_empty() {
        auth::ALL_SCOPES.iter().map(|s| s.to_string()).collect()
    } else {
        spec.scopes
    };
    let mut st = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
    let (id, plain) = st.create(spec.name, scopes, spec.ttl_secs);
    (201, json!({ "token": plain, "id": id }))
}

fn revoke_token_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    let mut st = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
    if st.revoke(id) {
        (200, json!({ "ok": true, "revoked": id }))
    } else {
        (404, json!({ "error": "no such token" }))
    }
}

fn agent_diff_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let agent = match st.state.agents.iter().find(|a| a.id == id) {
        Some(a) => a.clone(),
        None => return (404, json!({ "error": "no such agent" })),
    };
    if !agent.worktree {
        return (400, json!({ "error": "worktree not enabled" }));
    }
    let base = match agent.base_path.as_deref() {
        Some(p) if !p.trim().is_empty() => p,
        _ => return (400, json!({ "error": "worktree path unavailable" })),
    };
    let out = Command::new("git")
        .arg("-C")
        .arg(base)
        .arg("diff")
        .arg("--")
        .arg(&agent.workspace)
        .output();
    match out {
        Ok(o) if o.status.success() => (200, json!({ "diff": String::from_utf8_lossy(&o.stdout).to_string() })),
        Ok(o) => (400, json!({ "error": String::from_utf8_lossy(&o.stderr).trim().to_string() })),
        Err(e) => (400, json!({ "error": format!("git diff failed: {}", e) })),
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
    if let Some(t) = req.query.get("trigger") {
        list.retain(|r| r.trigger_id.as_deref() == Some(t.as_str()));
    }
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

/// A workspace argument may be an absolute path, a relative path, or a name
/// registered in `<data>/workspaces.json`.
fn resolve_workspace_input(app: &Arc<App>, input: &str) -> String {
    let data_dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let reg = workspaces::Registry::load(&data_dir);
    match reg.get(input) {
        Some(ws) => ws.path.clone(),
        None => input.to_string(),
    }
}

fn list_workspaces_route(app: &Arc<App>) -> (u16, Value) {
    let data_dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let reg = workspaces::Registry::load(&data_dir);
    (200, json!({ "workspaces": reg.workspaces }))
}

fn create_workspace_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewWorkspace = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    if spec.name.trim().is_empty() {
        return (400, json!({ "error": "name is required" }));
    }
    if spec.path.trim().is_empty() {
        return (400, json!({ "error": "path is required" }));
    }
    let data_dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let remote = spec.remote.unwrap_or_default();
    let mut reg = workspaces::Registry::load(&data_dir);
    match reg.add_named(&spec.name, Path::new(&spec.path), &remote) {
        Ok(ws) => match reg.save() {
            Ok(()) => (200, json!({ "workspace": ws })),
            Err(e) => (500, json!({ "error": format!("save workspaces.json failed: {}", e) })),
        },
        Err(e) => (400, json!({ "error": e })),
    }
}

fn clone_workspace_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewClone = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    if spec.input.trim().is_empty() {
        return (400, json!({ "error": "input is required" }));
    }
    let data_dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let protocol = spec.protocol.unwrap_or_else(|| "https".into());
    match workspaces::clone_repo(&data_dir, &spec.input, spec.dir.as_deref(), &protocol) {
        Ok(ws) => (200, json!({ "workspace": ws })),
        Err(e) => (400, json!({ "error": e })),
    }
}

fn remove_workspace_route(app: &Arc<App>, name: &str) -> (u16, Value) {
    let data_dir = {
        let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        st.dir.clone()
    };
    let mut reg = workspaces::Registry::load(&data_dir);
    if reg.remove(name) {
        match reg.save() {
            Ok(()) => (200, json!({ "ok": true, "removed": name })),
            Err(e) => (500, json!({ "error": format!("save workspaces.json failed: {}", e) })),
        }
    } else {
        (404, json!({ "error": "no such workspace" }))
    }
}

fn create_run(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let raw: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({"error": format!("bad json: {}", e)})),
    };
    if raw.get("images").map(|v| !v.is_array()).unwrap_or(false) {
        return (400, json!({"error": "images must be an array of paths"}));
    }
    if raw.get("options").map(|v| !v.is_object()).unwrap_or(false) {
        return (400, json!({"error": "options must be an object"}));
    }
    let spec: NewRun = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({"error": format!("bad json: {}", e)})),
    };
    if let Err(e) = worktree_request_parts(spec.worktree) {
        return (400, json!({ "error": e }));
    }
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
    let options = match crate::validate_run_options(&spec.options.unwrap_or_default()) {
        Ok(o) => o,
        Err(e) => return (400, json!({"error": e})),
    };
    let images = spec.images.unwrap_or_default();
    for img in &images {
        if let Err(e) = crate::validate_image_path(img) {
            return (400, json!({"error": e}));
        }
    }
    if let Err(e) = crate::runner::provider_args(
        &spec.agent,
        &options,
        &images,
        options.get("mode").map(String::as_str),
    ) {
        return (400, json!({"error": e}));
    }
    let workspace = match spec.workspace.clone().filter(|w| !w.trim().is_empty()) {
        Some(w) => {
            let resolved = resolve_workspace_input(app, &w);
            match canonicalize_dir(&resolved) {
                Some(abs) => abs,
                None => return (400, json!({"error": "workspace not found", "path": w})),
            }
        }
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
            resumed: false,
            resume_note: None,
            timeout_s,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,

            trigger_id: None,
            usage: None,
            labels: BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: options.clone(),
            images: images.clone(),
            worktree: None,
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

/// 依赖校验（服务端权威）：空串/不存在、自引用、成环。返回规范化后的依赖列表。
/// 错误文案与 CLI 侧严格一致，便于脚本 grep。
fn validate_job_deps(existing: &[Job], new_id: &str, deps: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for d in deps {
        if d.is_empty() {
            return Err(format!("jobs-add --after: no such job: {}", d));
        }
        if d == new_id {
            return Err("jobs-add --after: self reference".to_string());
        }
        if !existing.iter().any(|j| j.id == *d) {
            return Err(format!("jobs-add --after: no such job: {}", d));
        }
        if !out.contains(d) {
            out.push(d.clone());
        }
    }
    if out.is_empty() {
        return Ok(out);
    }
    // 环检测：从待建节点出发 DFS，遇到回边即报，路径按访问顺序拼接
    let mut adj: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for j in existing {
        adj.insert(j.id.clone(), j.depends_on.clone());
    }
    adj.insert(new_id.to_string(), out.clone());
    if let Some(path) = dep_cycle_path(&adj, new_id) {
        return Err(format!(
            "jobs-add --after: dependency cycle: {}",
            path.join(" -> ")
        ));
    }
    Ok(out)
}

/// 从 origin 出发找回到 origin 的依赖路径（`a -> b -> a`）。
fn dep_cycle_path(
    adj: &std::collections::BTreeMap<String, Vec<String>>,
    origin: &str,
) -> Option<Vec<String>> {
    fn walk(
        adj: &std::collections::BTreeMap<String, Vec<String>>,
        cur: &str,
        origin: &str,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        for next in adj.get(cur).map(|v| v.as_slice()).unwrap_or(&[]) {
            if next == origin {
                let mut path = stack.clone();
                path.push(next.clone());
                return Some(path);
            }
            if stack.iter().any(|s| s == next) {
                continue; // 与 origin 无关的环，不在此报
            }
            stack.push(next.clone());
            if let Some(p) = walk(adj, next, origin, stack) {
                return Some(p);
            }
            stack.pop();
        }
        None
    }
    let mut stack = vec![origin.to_string()];
    walk(adj, origin, origin, &mut stack)
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
        Some(w) => {
            let resolved = resolve_workspace_input(app, &w);
            match canonicalize_dir(&resolved) {
                Some(abs) => abs,
                None => return (400, json!({"error": "workspace not found", "path": w})),
            }
        }
        None => std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into()),
    };
    let spec_deps = spec.depends_on.unwrap_or_default();
    let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
    // next_id 只动内存里的 seq；校验失败直接返回，不落库（seq 不持久化）
    let id = st.next_id("job");
    let deps = match validate_job_deps(&st.state.jobs, &id, &spec_deps) {
        Ok(v) => v,
        Err(msg) => return (400, json!({ "error": msg })),
    };
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
        depends_on: deps,
        last_skip: None,
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

#[derive(Deserialize)]
struct NewTerminal {
    #[serde(default)]
    name: String,
    #[serde(default)]
    workspace: String,
    #[serde(default)]
    shell: String,
    #[serde(default)]
    buffer_kb: Option<usize>,
}

#[derive(Deserialize)]
struct KeysBody {
    #[serde(default)]
    keys: String,
    #[serde(default)]
    enter: bool,
}

#[derive(Deserialize)]
struct NewScript {
    #[serde(default)]
    name: String,
    #[serde(default)]
    cmd: String,
    #[serde(default)]
    workspace: String,
}

fn create_terminal_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewTerminal = if req.body.is_empty() {
        NewTerminal {
            name: String::new(),
            workspace: String::new(),
            shell: String::new(),
            buffer_kb: None,
        }
    } else {
        match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
        }
    };
    let kb = spec
        .buffer_kb
        .unwrap_or(terminals::DEFAULT_BUFFER_KB)
        .clamp(1, 65536);
    match terminals::create(app, &spec.name, &spec.workspace, &spec.shell, kb) {
        Ok(t) => (200, json!({ "terminal": t })),
        Err(e) => (400, json!({ "error": e })),
    }
}

fn get_terminal_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    match terminals::get(app, id) {
        Ok(t) => (200, json!({ "terminal": t })),
        Err(e) => terminal_error(e),
    }
}

fn kill_terminal_route(app: &Arc<App>, id: &str) -> (u16, Value) {
    match terminals::kill(app, id) {
        Ok(t) => (200, json!({ "killed": true, "terminal": t })),
        Err(e) => terminal_error(e),
    }
}

fn terminal_output_route(app: &Arc<App>, req: &Req, id: &str) -> (u16, Value) {
    let since: Option<u64> = req.query.get("since").and_then(|v| v.parse().ok());
    let tail: Option<usize> = req.query.get("tail").and_then(|v| v.parse().ok());
    match terminals::capture(app, id, since, tail) {
        Ok((bytes, total)) => (
            200,
            json!({
                "id": id,
                "output": String::from_utf8_lossy(&bytes).to_string(),
                "bytes": bytes.len(),
                "total": total,
                "next": total,
            }),
        ),
        Err(e) => terminal_error(e),
    }
}

fn terminal_keys_route(app: &Arc<App>, req: &Req, id: &str) -> (u16, Value) {
    let spec: KeysBody = if req.body.is_empty() {
        KeysBody {
            keys: String::new(),
            enter: false,
        }
    } else {
        match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
        }
    };
    match terminals::send_keys(app, id, &spec.keys, spec.enter) {
        Ok(()) => (
            200,
            json!({ "ok": true, "sent": spec.keys.len() + if spec.enter { 1 } else { 0 } }),
        ),
        Err(e) => terminal_error(e),
    }
}

fn create_script_route(app: &Arc<App>, req: &Req) -> (u16, Value) {
    let spec: NewScript = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return (400, json!({ "error": format!("bad json: {}", e) })),
    };
    match terminals::script_add(app, &spec.name, &spec.cmd, &spec.workspace) {
        Ok(s) => (200, json!({ "script": s })),
        Err(e) => (400, json!({ "error": e })),
    }
}

fn remove_script_route(app: &Arc<App>, name: &str) -> (u16, Value) {
    match terminals::script_rm(app, name) {
        Ok(n) => (200, json!({ "ok": true, "removed": n, "name": name })),
        Err(e) => (404, json!({ "error": e })),
    }
}

fn script_action_route(app: &Arc<App>, name: &str, start: bool) -> (u16, Value) {
    let r = if start {
        terminals::script_start(app, name)
    } else {
        terminals::script_stop(app, name)
    };
    match r {
        Ok(s) => (200, json!({ "script": s })),
        Err(e) => {
            if e.contains("no such") {
                (404, json!({ "error": e }))
            } else {
                (400, json!({ "error": e }))
            }
        }
    }
}

fn terminal_error(e: String) -> (u16, Value) {
    if e.contains("no such") {
        (404, json!({ "error": e }))
    } else {
        (400, json!({ "error": e }))
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
    fn whoami_still_requires_token() {
        assert!(needs_auth("/v1/auth/whoami"));
    }

    #[test]
    fn ui_response_routing() {
        match ui_response("GET", "/") {
            Some(UiResponse::Redirect(loc)) => assert_eq!(loc, "/ui/"),
            other => panic!("expected redirect, got {:?}", other),
        }
        match ui_response("GET", "/ui") {
            Some(UiResponse::Asset(code, ct, body)) => {
                assert_eq!(code, 200);
                assert_eq!(ct, "text/html; charset=utf-8");
                assert_eq!(body, crate::web::INDEX);
            }
            other => panic!("expected index asset, got {:?}", other),
        }
        match ui_response("GET", "/ui/app.js") {
            Some(UiResponse::Asset(code, ct, _)) => {
                assert_eq!(code, 200);
                assert_eq!(ct, "application/javascript; charset=utf-8");
            }
            other => panic!("expected js asset, got {:?}", other),
        }
        match ui_response("GET", "/ui/missing.js") {
            Some(UiResponse::NotFound) => {}
            other => panic!("expected not found, got {:?}", other),
        }
        assert!(ui_response("POST", "/ui/").is_none());
        assert!(ui_response("GET", "/v1/agents").is_none());
    }

    #[test]
    fn bearer_header_parses_authorization() {
        let req = Req {
            method: "GET".into(),
            path: "/v1/agents".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::from([("authorization".into(), "Bearer abc123".into())]),
            body: vec![],
        };
        assert_eq!(bearer_header(&req), Some("abc123"));
        let req = Req {
            method: "GET".into(),
            path: "/v1/agents".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::from([("authorization".into(), "bearer  xyz  ".into())]),
            body: vec![],
        };
        assert_eq!(bearer_header(&req), Some("xyz"));
        let req = Req {
            method: "GET".into(),
            path: "/v1/agents".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::from([("authorization".into(), "Basic abc".into())]),
            body: vec![],
        };
        assert_eq!(bearer_header(&req), None);
    }

    #[test]
    fn session_route_hits_session_endpoint_before_agent_route() {
        let dir = std::env::temp_dir().join(format!("roamway-session-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Done);
        {
            let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            st.state.agents.push(agents::Agent {
                id: "ag_00000001".into(),
                title: "t".into(),
                kind: "codex".into(),
                model: String::new(),
                workspace: String::new(),
                session_id: "sid_1".into(),
                state: "idle".into(),
                parent: String::new(),
                history: vec![],
                created_at: 0,
                updated_at: 0,
                last_run_id: "run_000001".into(),
                last_error: String::new(),
                worktree: false,
                branch: None,
                base_path: None,
                labels: BTreeMap::new(),
                worktree_mode: None,
            });
            st.save();
        }
        let req = Req {
            method: "GET".into(),
            path: "/v1/agents/ag_00000001/session".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::new(),
            body: vec![],
        };
        let (code, body) = route(&app, &req);
        assert_eq!(code, 200);
        assert_eq!(body["agent_id"].as_str(), Some("ag_00000001"));
        assert_eq!(body["resumable"].as_bool(), Some(true));
        assert_eq!(body["has_session"].as_bool(), Some(true));
        assert_eq!(body["session_id"].as_str(), Some("sid_1"));
        assert!(body.get("agent").is_none(), "session 路径不能被 get_agent_route 吞掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_route_missing_agent_returns_404() {
        let dir = std::env::temp_dir().join(format!("roamway-session-missing-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Done);
        let req = Req {
            method: "GET".into(),
            path: "/v1/agents/ag_nope/session".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::new(),
            body: vec![],
        };
        let (code, body) = route(&app, &req);
        assert_eq!(code, 404);
        assert_eq!(body["error"].as_str(), Some("no such agent"));
        let _ = std::fs::remove_dir_all(&dir);
    }

#[test]
    fn token_create_list_revoke() {
        let dir = std::env::temp_dir().join(format!("roamway-tokens-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Done);
        let req = Req {
            method: "POST".into(),
            path: "/v1/tokens".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::new(),
            body: br#"{"name":"ui","scopes":["read"]}"#.to_vec(),
        };
        let (code, body) = create_token_route(&app, &req);
        assert_eq!(code, 201);
        assert_eq!(body["token"].as_str().map(|s| s.len()), Some(64));
        let id = body["id"].as_str().unwrap().to_string();
        let (code, body) = list_tokens_route(&app);
        assert_eq!(code, 200);
        assert_eq!(body["tokens"].as_array().unwrap().len(), 1);
        let (code, _) = revoke_token_route(&app, &id);
        assert_eq!(code, 200);
        let (code, _) = revoke_token_route(&app, &id);
        assert_eq!(code, 404);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_create_list_delete() {
        let dir = std::env::temp_dir().join(format!("roamway-hosts-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Done);
        let req = Req {
            method: "POST".into(),
            path: "/v1/hosts".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::new(),
            body: br#"{"name":"nas1","endpoint":"192.0.2.10:8796","token":"secret"}"#.to_vec(),
        };
        let (code, body) = create_host_route(&app, &req);
        assert_eq!(code, 201);
        assert_eq!(body["host"]["name"].as_str(), Some("nas1"));
        assert_eq!(body["host"]["token"].as_str(), Some("****"));
        let (code, body) = list_hosts(&app);
        assert_eq!(code, 200);
        assert_eq!(body["hosts"].as_array().unwrap().len(), 1);
        let (code, _) = delete_host_route(&app, "nas1");
        assert_eq!(code, 200);
        let (code, _) = delete_host_route(&app, "nas1");
        assert_eq!(code, 404);
        let _ = std::fs::remove_dir_all(&dir);
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
        // 截断必须落在 UTF-8 边界，否则会 panic
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
            resumed: false,
            resume_note: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,

            trigger_id: None,
            usage: None,
            labels: BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: BTreeMap::new(),
            images: Vec::new(),
            worktree: None,
        };
        let store = Store {
            dir: dir.clone(),
            state: State {
                runs: vec![mk("a"), mk("b"), mk("c")],
                jobs: vec![],
                agents: vec![],
                seq: 3,
            },
            loaded_ok: true,
        };
        let app = Arc::new(App {
            store: std::sync::Mutex::new(store),
            schedules: std::sync::Mutex::new(Default::default()),
            terms: std::sync::Mutex::new(Default::default()),
            term_rt: std::sync::Mutex::new(HashMap::new()),
            token: None,
            started: now(),
            cancels: std::sync::Mutex::new(HashMap::new()),
            tz_offset_hours: 8,
            max_concurrent: 4,
            max_conn: 64,
            conns: std::sync::atomic::AtomicUsize::new(0),
            req_total: std::sync::atomic::AtomicU64::new(0),
            req_failed: std::sync::atomic::AtomicU64::new(0),
            tokens: std::sync::Mutex::new(auth::TokenStore::load(&dir)),
            approvals: std::sync::Mutex::new(auth::ApprovalStore::load(&dir)),
            passwords: std::sync::Mutex::new(auth::PasswordStore::load(&dir)),
            pairings: std::sync::Mutex::new(auth::PairingStore::load(&dir)),
            hub_rt: std::sync::Mutex::new(Default::default()),
            allow_hosts: vec![],
            allow_origins: vec![],
            require_approval: vec![],
            bind_host: "127.0.0.1".to_string(),
            unix_socket: None,
            tls_cert: None,
            tls_key: None,
        });
        let req = Req {
            method: "GET".into(),
            path: "/v1/runs".into(),
            query: HashMap::from([("limit".into(), "1".into()), ("offset".into(), "1".into())]),
            raw_query: String::new(),
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
        test_app_with_max(dir, agent, status, 4)
    }

    fn test_app_with_max(
        dir: &std::path::Path,
        agent: &str,
        status: RunStatus,
        max_concurrent: usize,
    ) -> Arc<App> {
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
            resumed: false,
            resume_note: None,
            timeout_s: 60,
            job_id: None,
            pid: None,
            attempts: 0,
            agent_id: None,
            trigger_id: None,
            usage: None,
            labels: BTreeMap::new(),
            env: Vec::new(),
            cwd: None,
            options: BTreeMap::new(),
            images: Vec::new(),
            worktree: None,
        };
        let store = Store {
            dir: dir.to_path_buf(),
            state: State { runs: vec![run], jobs: vec![], agents: vec![], seq: 1 },
            loaded_ok: true,
        };
        Arc::new(App {
            store: std::sync::Mutex::new(store),
            schedules: std::sync::Mutex::new(Default::default()),
            terms: std::sync::Mutex::new(Default::default()),
            term_rt: std::sync::Mutex::new(HashMap::new()),
            token: None,
            started: now(),
            cancels: std::sync::Mutex::new(HashMap::new()),
            tz_offset_hours: 8,
            max_concurrent,
            max_conn: 64,
            conns: std::sync::atomic::AtomicUsize::new(0),
            req_total: std::sync::atomic::AtomicU64::new(0),
            req_failed: std::sync::atomic::AtomicU64::new(0),
            tokens: std::sync::Mutex::new(auth::TokenStore::load(dir)),
            approvals: std::sync::Mutex::new(auth::ApprovalStore::load(dir)),
            passwords: std::sync::Mutex::new(auth::PasswordStore::load(dir)),
            pairings: std::sync::Mutex::new(auth::PairingStore::load(dir)),
            hub_rt: std::sync::Mutex::new(Default::default()),
            allow_hosts: vec![],
            allow_origins: vec![],
            require_approval: vec![],
            bind_host: "127.0.0.1".to_string(),
            unix_socket: None,
            tls_cert: None,
            tls_key: None,
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

    #[test]
    fn hub_webhook_paths_are_public() {
        assert!(!needs_auth("/v1/hub/hooks/trg_00000001"));
        assert!(needs_auth("/v1/hub"));
        assert!(needs_auth("/v1/hub/triggers"));
    }

    #[test]
    fn hub_overview_404_without_init() {
        let dir = std::env::temp_dir().join(format!("roamway-hub-404-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Done);
        let (code, body) = hub_overview_route(&app);
        assert_eq!(code, 404);
        assert_eq!(body["error"].as_str(), Some("hub not initialized"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hub_legacy_token_reads_disk_for_rotation() {
        let dir = std::env::temp_dir().join(format!("roamway-hub-rotate-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Done);
        assert_eq!(legacy_token_on_disk(&app), None);
        std::fs::write(dir.join("token"), "rotated-token\n").unwrap();
        assert_eq!(legacy_token_on_disk(&app).as_deref(), Some("rotated-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hub_trigger_enable_disable_and_missing() {
        let dir = std::env::temp_dir().join(format!("roamway-hub-trig-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut hub = hub::init(&dir, None, false).unwrap();
        hub.triggers.push(hub::HubTrigger {
            id: "trg_00000001".into(),
            source: "generic".into(),
            repo: None,
            events: vec![],
            secret_env: "ROAMWAY_HOOK_TRG_00000001".into(),
            agent_spec: hub::HubAgentSpec {
                provider: "shell".into(),
                cwd: ".".into(),
                prompt: "echo hi".into(),
            },
            enabled: true,
        });
        hub::save(&dir, &hub).unwrap();
        let app = test_app(&dir, "shell", RunStatus::Done);
        let (code, body) = hub_trigger_enabled_route(&app, "trg_00000001", false);
        assert_eq!(code, 200);
        assert_eq!(body["enabled"].as_bool(), Some(false));
        let (code2, _) = hub_trigger_enabled_route(&app, "trg_missing", true);
        assert_eq!(code2, 404);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_runs_filters_by_trigger() {
        let dir = std::env::temp_dir().join(format!("roamway-trig-filter-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app(&dir, "shell", RunStatus::Done);
        {
            let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            st.state.runs[0].trigger_id = Some("trg_a".into());
        }
        let req = Req {
            method: "GET".into(),
            path: "/v1/runs".into(),
            query: HashMap::from([("trigger".into(), "trg_a".into())]),
            raw_query: String::new(),
            headers: HashMap::new(),
            body: vec![],
        };
        let (code, body) = list_runs(&app, &req);
        assert_eq!(code, 200);
        assert_eq!(body["runs"].as_array().unwrap().len(), 1);

        let req = Req {
            method: "GET".into(),
            path: "/v1/runs".into(),
            query: HashMap::from([("trigger".into(), "trg_b".into())]),
            raw_query: String::new(),
            headers: HashMap::new(),
            body: vec![],
        };
        let (code, body) = list_runs(&app, &req);
        assert_eq!(code, 200);
        assert_eq!(body["runs"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hub_hook_busy_returns_429() {
        let dir = std::env::temp_dir().join(format!("roamway-hub-busy-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let secret_env = format!("ROAMWAY_HOOK_BUSY_{}", now());
        std::env::set_var(&secret_env, "s3cret");
        let mut hub = hub::init(&dir, None, false).unwrap();
        hub.triggers.push(hub::HubTrigger {
            id: "trg_00000001".into(),
            source: "generic".into(),
            repo: None,
            events: vec![],
            secret_env: secret_env.clone(),
            agent_spec: hub::HubAgentSpec {
                provider: "shell".into(),
                cwd: ".".into(),
                prompt: "echo hi".into(),
            },
            enabled: true,
        });
        hub::save(&dir, &hub).unwrap();
        let app = test_app_with_max(&dir, "shell", RunStatus::Running, 1);
        let mac = auth::hmac_sha256(b"s3cret", b"{}");
        let sig = format!("sha256={}", auth::hex_encode(&mac));
        let req = Req {
            method: "POST".into(),
            path: "/v1/hub/hooks/trg_00000001".into(),
            query: HashMap::new(),
            raw_query: String::new(),
            headers: HashMap::from([
                ("x-hub-signature-256".into(), sig),
                ("x-github-delivery".into(), "busy-1".into()),
            ]),
            body: b"{}".to_vec(),
        };
        let (code, body) = hub_hook_route(&app, &req, "trg_00000001");
        assert_eq!(code, 429);
        assert_eq!(body["skipped"].as_str(), Some("busy"));
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn unix_socket_path_permissions_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("roamway-unix-sock-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("roamway.sock");
        let listener = bind_unix_listener(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metrics_text_empty_state_is_valid_prometheus() {
        let dir = std::env::temp_dir().join(format!("roamway-metrics-empty-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app_with_max(&dir, "shell", RunStatus::Done, 1);
        {
            let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            st.state.runs.clear();
        }
        let body = metrics_text(&app);
        assert!(body.ends_with('\n'), "body must end with a newline");
        let names = [
            "roamway_build_info",
            "roamway_uptime_seconds",
            "roamway_runs",
            "roamway_agents",
            "roamway_jobs",
            "roamway_jobs_enabled",
            "roamway_schedules",
            "roamway_schedules_paused",
            "roamway_schedule_failures_total",
            "roamway_terminals",
            "roamway_http_concurrent_requests",
            "roamway_http_requests_total",
            "roamway_http_requests_failed_total",
        ];
        for n in names {
            assert!(body.contains(n), "missing metric {}", n);
            assert!(
                body.contains(&format!("# TYPE {} ", n)),
                "missing TYPE line for {}",
                n
            );
            assert!(
                body.contains(&format!("# HELP {} ", n)),
                "missing HELP line for {}",
                n
            );
        }
        assert!(!body.contains("NaN"), "must not emit NaN");
        assert!(!body.contains("+Inf") && !body.contains("-Inf"), "must not emit Inf");
        assert!(body.contains("roamway_runs{status=\"done\"} 0"));
        assert!(body.contains("roamway_agents 0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metrics_text_counts_runs_jobs_and_schedules() {
        let dir = std::env::temp_dir().join(format!("roamway-metrics-counts-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app_with_max(&dir, "shell", RunStatus::Done, 1);
        {
            let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            let mut running = st.state.runs[0].clone();
            running.id = "run_running".into();
            running.status = RunStatus::Running;
            st.state.runs.push(running);
            let job: Job = serde_json::from_value(json!({
                "id": "job_1",
                "title": "t",
                "agent": "shell",
                "workspace": ".",
                "prompt": "p",
                "timeout_s": 60,
                "next_at": 0,
                "enabled": true,
            }))
            .unwrap();
            st.state.jobs.push(job);
        }
        {
            let mut sc = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
            let paused: schedules::Schedule = serde_json::from_value(json!({
                "id": "sch_1",
                "enabled": true,
                "paused": true,
                "consecutive_failures": 2,
            }))
            .unwrap();
            sc.schedules.push(paused);
        }
        let body = metrics_text(&app);
        assert!(body.contains("roamway_runs{status=\"done\"} 1"), "{}", body);
        assert!(body.contains("roamway_runs{status=\"running\"} 1"));
        assert!(body.contains("roamway_runs{status=\"queued\"} 0"));
        assert!(body.contains("roamway_jobs 1"));
        assert!(body.contains("roamway_jobs_enabled 1"));
        assert!(body.contains("roamway_schedules 1"));
        assert!(body.contains("roamway_schedules_paused 1"));
        assert!(body.contains("roamway_schedule_failures_total 2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn esc_label_escapes_prometheus_reserved_chars() {
        assert_eq!(esc_label("plain"), "plain");
        assert_eq!(esc_label("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(esc_label("line1\nline2"), "line1\\nline2");
    }

    fn http_roundtrip(app: Arc<App>, raw: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let _ = handle(app, s);
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(raw.as_bytes()).unwrap();
        c.flush().unwrap();
        let mut buf = Vec::new();
        let _ = c.read_to_end(&mut buf);
        srv.join().unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }

    #[test]
    fn metrics_endpoint_requires_token_and_serves_text() {
        let dir = std::env::temp_dir().join(format!("roamway-metrics-http-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let app = test_app_with_max(&dir, "shell", RunStatus::Done, 1);
        let tok = {
            let mut ts = app.tokens.lock().unwrap_or_else(|e| e.into_inner());
            ts.create(
                Some("prometheus".into()),
                vec!["read".into(), "admin".into(), "run".into()],
                None,
            )
            .1
        };

        let anon = http_roundtrip(
            app.clone(),
            "GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );
        assert!(
            anon.starts_with("HTTP/1.1 401"),
            "expected 401, got {}",
            anon.lines().next().unwrap_or("")
        );

        let ok = http_roundtrip(
            app.clone(),
            &format!(
                "GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\n\r\n",
                tok
            ),
        );
        assert!(ok.starts_with("HTTP/1.1 200"), "{}", ok.lines().next().unwrap_or(""));
        assert!(ok.contains("text/plain; version=0.0.4"), "{}", ok);
        assert!(ok.contains("roamway_build_info"));

        let via_query = http_roundtrip(
            app.clone(),
            &format!(
                "GET /metrics?token={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                tok
            ),
        );
        assert!(
            via_query.starts_with("HTTP/1.1 200"),
            "query token must be accepted on /metrics"
        );

        let post = http_roundtrip(
            app.clone(),
            &format!(
                "POST /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\n\r\n",
                tok
            ),
        );
        assert!(
            post.starts_with("HTTP/1.1 405"),
            "expected 405, got {}",
            post.lines().next().unwrap_or("")
        );

        // 404 也走响应码 >= 400 的统一计数（UI 静态资源缺失）
        let failed_before = app.req_failed.load(Ordering::Relaxed);
        let miss = http_roundtrip(
            app.clone(),
            "GET /ui/missing.js HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );
        assert!(
            miss.starts_with("HTTP/1.1 404"),
            "expected 404, got {}",
            miss.lines().next().unwrap_or("")
        );
        assert_eq!(
            app.req_failed.load(Ordering::Relaxed),
            failed_before + 1,
            "404 must be counted exactly once"
        );

        assert!(
            app.req_total.load(Ordering::Relaxed) >= 4,
            "req_total must count every request"
        );
        assert!(
            app.req_failed.load(Ordering::Relaxed) >= 2,
            "req_failed must count 401/405"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn mk_job(id: &str, deps: &[&str]) -> Job {
        Job {
            id: id.into(),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: "echo hi".into(),
            every_secs: Some(60),
            daily: None,
            tz_offset_hours: 8,
            timeout_s: 60,
            max_retries: 0,
            enabled: true,
            allow_overlap: false,
            next_at: 0,
            last_run: None,
            last_error: None,
            attempts: 0,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            last_skip: None,
        }
    }

    #[test]
    fn after_rejects_unknown_and_empty() {
        let existing = vec![mk_job("job_000001", &[])];
        assert_eq!(
            validate_job_deps(&existing, "job_000002", &["job_999999"].map(String::from)),
            Err("jobs-add --after: no such job: job_999999".to_string())
        );
        assert_eq!(
            validate_job_deps(&existing, "job_000002", &["".to_string()]),
            Err("jobs-add --after: no such job: ".to_string())
        );
    }

    #[test]
    fn after_rejects_self_reference() {
        let existing = vec![mk_job("job_000001", &[])];
        assert_eq!(
            validate_job_deps(&existing, "job_000002", &["job_000002".to_string()]),
            Err("jobs-add --after: self reference".to_string())
        );
    }

    #[test]
    fn after_rejects_direct_cycle() {
        // 已有 A 依赖待建的 B；现在建 B 并让 B 依赖 A -> b -> a -> b
        let existing = vec![mk_job("job_000001", &["job_000002"])];
        assert_eq!(
            validate_job_deps(&existing, "job_000002", &["job_000001".to_string()]),
            Err("jobs-add --after: dependency cycle: job_000002 -> job_000001 -> job_000002"
                .to_string())
        );
    }

    #[test]
    fn after_rejects_indirect_cycle() {
        // A -> C -> B(待建) -> A
        let existing = vec![
            mk_job("job_000001", &["job_000003"]),
            mk_job("job_000003", &["job_000002"]),
        ];
        assert_eq!(
            validate_job_deps(&existing, "job_000002", &["job_000001".to_string()]),
            Err("jobs-add --after: dependency cycle: job_000002 -> job_000001 -> job_000003 -> job_000002"
                .to_string())
        );
    }

    #[test]
    fn after_accepts_acyclic_and_dedupes() {
        let existing = vec![mk_job("job_000001", &[]), mk_job("job_000002", &[])];
        let got = validate_job_deps(
            &existing,
            "job_000003",
            &["job_000001".to_string(), "job_000002".to_string(), "job_000001".to_string()],
        );
        assert_eq!(
            got,
            Ok(vec!["job_000001".to_string(), "job_000002".to_string()])
        );
    }
}
