mod agents;
mod auth;
mod cron;
mod hosts;
mod hub;
mod jobs;
mod runner;
mod schedules;
mod server;
mod terminals;
mod types;
mod workspaces;
mod plugins;
mod providers;
mod speech;
mod usage;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use types::*;

const BOOLS: &[&str] = &["no-wait", "quiet", "json", "allow-overlap", "disabled", "help", "force", "probe", "all", "worktree", "keep-worktree", "run-now", "overlap", "enabled", "enter", "no-enter", "stdin"];

fn usage() {
    println!(
        r#"roamway {} — 迷你多智能体编排器（国内模型 / 长跑友好）

  roamway serve  [--bind 127.0.0.1:8796] [--data DIR] [--token T] [--tz 8] [--max-concurrent 4] [--max-conn 64]
  roamway run    --agent codex [--model M] [--workspace DIR] [--title T] [--timeout 1800] [--no-wait] [--host H] "prompt"
  roamway clone  <owner/repo | git url> [--dir DIR] [--protocol https|ssh] [--json] [--host H]
  roamway agent run <prompt> [--title --workspace --model --kind --worktree --no-wait]
  roamway agent ls|inspect|logs|wait|attach|archive [--keep-worktree]|reload|update|open|detach|import
  roamway attach|inspect|wait|archive|import <id>   # 顶层别名
  roamway ls     [--limit 20] [--host H|all] [--trigger ID]
  roamway status [ID] [--json] [--host H]
  roamway logs   ID [--tail 80] [--host H]
  roamway stop   ID [--host H]
  roamway rm     ID
  roamway jobs
  roamway schedule create|ls|inspect|logs|pause|resume|delete|run-once|update [--host H]
  roamway hooks [--json] [--host H]
  roamway hub [init [--name N] [--force] | permissions | projects | deploy | logout | status | token rotate]
  roamway jobs-add --agent shell --every 3600 | --daily 08:30 [--model M] [--workspace DIR] [--max-retries 2] "prompt"
  roamway jobs-run ID
  roamway jobs-rm  ID
  roamway health
  roamway host add <name> --endpoint <ep> [--token T] [--force]
  roamway host ls [--probe] [--json]
  roamway host rm <name> | host enable|disable <name> | host probe <name>
  roamway daemon status|config|start|stop|restart|reload|pair|set-password
  roamway pair [--json] [--host H]
  roamway token add|create --name N [--scope read|run|admin] [--ttl 24h]
  roamway token ls|list | token revoke <id>
  roamway approve <id> | deny <id> | approvals [--all]
  roamway onboard [--bind 0.0.0.0:8796]
  roamway restart | reload

运维扩展：
  roamway workspace ls [--json]
  roamway workspace rm <name>
  roamway terminal create [--name N] [--workspace DIR] [--shell bash] [--buffer-kb 256]
  roamway terminal ls [--json] | capture <id> [--since N] [--tail N] | kill <id>
  roamway terminal send-keys <id> <keys...> [--enter|--no-enter]
  roamway script ls [--json] | add --name N --cmd C [--workspace P] | rm <name>
  roamway script start <name> | stop <name>
  注：受管终端无 TTY，TUI 程序（vim/top 交互界面）不支持

生态扩展：
  roamway provider [ls|models [<id>] [--refresh]|diagnostic <id>]
  roamway usage [ls [--since 7d|24h|ISO] [--provider id]|inspect <run_id>] [--json]
  roamway plugin [ls|init <name>|logs <name>|reload <name>|enable|disable <name> [--trust]|remove <name> [--keep-files]]
  roamway speech [transcribe <file>|say <text> [--out file]]

通用参数：--host http://127.0.0.1:8796  --token T
环境变量：ROAMWAY_HOST / ROAMWAY_TOKEN / ROAMWAY_DEEPSEEK_KEY / ROAMWAY_QWEN_KEY
agent 可选：codex | hermes | claude | shell | api:deepseek | api:qwen"#,
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_flags(args: &[String]) -> (HashMap<String, String>, Vec<String>) {
    let mut flags = HashMap::new();
    let mut pos = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            pos.extend(args[i + 1..].iter().cloned());
            break;
        }
        if let Some(k) = a.strip_prefix("--") {
            if BOOLS.contains(&k) {
                push_flag(&mut flags, k, "true".into());
                i += 1;
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                push_flag(&mut flags, k, args[i + 1].clone());
                i += 2;
            } else {
                push_flag(&mut flags, k, "true".into());
                i += 1;
            }
        } else {
            pos.push(a.clone());
            i += 1;
        }
    }
    (flags, pos)
}

fn push_flag(flags: &mut HashMap<String, String>, key: &str, value: String) {
    match flags.get_mut(key) {
        Some(existing) => {
            existing.push('\u{1f}');
            existing.push_str(&value);
        }
        None => {
            flags.insert(key.to_string(), value);
        }
    }
}

fn flag_many(flags: &HashMap<String, String>, key: &str) -> Vec<String> {
    flags
        .get(key)
        .map(|v| v.split('\u{1f}').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

struct Client {
    base: String,
    token: Option<String>,
}

impl Client {
    fn from_flags(flags: &HashMap<String, String>) -> Client {
        Client {
            base: flags
                .get("host")
                .cloned()
                .or_else(|| std::env::var("ROAMWAY_HOST").ok())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| local_base(flags)),
            token: local_token(flags),
        }
    }

    fn req(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}{}", self.base.trim_end_matches('/'), path);
        let mut r = match method {
            "GET" => ureq::get(&url),
            "POST" => ureq::post(&url),
            "DELETE" => ureq::delete(&url),
            "PATCH" => ureq::patch(&url),
            _ => ureq::get(&url),
        }
        .timeout(Duration::from_secs(60));
        if let Some(t) = &self.token {
            r = r.set("x-roamway-token", t);
        }
        let res = match body {
            Some(b) => r.send_json(b),
            None => r.call(),
        };
        match res {
            Ok(resp) => resp
                .into_json::<Value>()
                .map_err(|e| format!("bad response: {}", e)),
            Err(ureq::Error::Status(code, resp)) => {
                let t = resp.into_string().unwrap_or_default();
                Err(format!("HTTP {}: {}", code, t))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

fn local_base(flags: &HashMap<String, String>) -> String {
    if let Ok(raw) = std::fs::read_to_string(data_dir_for(flags).join("bind")) {
        let raw = raw.trim().to_string();
        if !raw.is_empty() {
            return hosts::endpoint_normalize(&raw);
        }
    }
    "http://127.0.0.1:8796".to_string()
}

fn local_token(flags: &HashMap<String, String>) -> Option<String> {
    flags
        .get("token")
        .cloned()
        .or_else(|| std::env::var("ROAMWAY_TOKEN").ok())
        .or_else(|| {
            std::fs::read_to_string(data_dir_for(flags).join("token"))
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

fn local_client(flags: &HashMap<String, String>) -> Client {
    let mut c = Client::from_flags(flags);
    if let Some(raw) = host_flag(flags) {
        c.base = hosts::endpoint_normalize(&raw);
    }
    c
}

fn local_only_client(flags: &HashMap<String, String>) -> Client {
    Client {
        base: local_base(flags),
        token: local_token(flags),
    }
}

fn host_flag(flags: &HashMap<String, String>) -> Option<String> {
    flags
        .get("host")
        .cloned()
        .or_else(|| std::env::var("ROAMWAY_HOST").ok())
        .filter(|s| !s.is_empty())
}

fn data_dir_for(flags: &HashMap<String, String>) -> PathBuf {
    flags
        .get("data")
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir)
}

fn registry_for(flags: &HashMap<String, String>) -> hosts::Registry {
    hosts::Registry::load(&data_dir_for(flags))
}

fn resolve_host(flags: &HashMap<String, String>) -> Option<hosts::Host> {
    let raw = host_flag(flags)?;
    registry_for(flags).get(&raw).cloned()
}

fn dispatch_json(host: &hosts::Host, method: &str, path: &str) -> Result<Value, String> {
    let (code, text) = hosts::dispatch(host, method, path, None)?;
    if !(200..300).contains(&code) {
        return Err(format!("HTTP {}: {}", code, text));
    }
    serde_json::from_str::<Value>(&text).map_err(|e| format!("bad response: {}", e))
}

fn request(flags: &HashMap<String, String>, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    if let Some(h) = resolve_host(flags) {
        let body_str = match body {
            Some(v) => Some(serde_json::to_string(&v).map_err(|e| e.to_string())?),
            None => None,
        };
        let (code, text) = hosts::dispatch(&h, method, path, body_str.as_deref())?;
        if !(200..300).contains(&code) {
            return Err(format!("HTTP {}: {}", code, text));
        }
        return serde_json::from_str::<Value>(&text).map_err(|e| format!("bad response: {}", e));
    }
    local_client(flags).req(method, path, body)
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

fn host_state(h: &hosts::Host) -> &'static str {
    if !h.enabled {
        return "disabled";
    }
    match &h.last_probe {
        None => "unknown",
        Some(p) if p.ok => "online",
        Some(p) if p.error == "skipped" => "skipped",
        Some(_) => "error",
    }
}

fn retry_secs(h: &hosts::Host) -> i64 {
    match h.next_retry_at {
        Some(nrt) => (nrt - now()).max(0),
        None => 0,
    }
}

fn gen_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let rs = std::collections::hash_map::RandomState::new();
    let mut out = String::with_capacity(32);
    for i in 0u64..16 {
        let mut hasher = rs.build_hasher();
        hasher.write_u64(i);
        let v = hasher.finish();
        out.push_str(&format!("{:02x}", (v >> 56) as u8));
    }
    out
}

fn bind_parts(bind: &str) -> Option<(String, u16)> {
    let b = bind.trim();
    let (host, port) = if let Some(rest) = b.strip_prefix('[') {
        let (h, p) = rest.split_once("]:")?;
        (h.to_string(), p.parse::<u16>().ok()?)
    } else {
        let (h, p) = b.rsplit_once(':')?;
        (h.to_string(), p.parse::<u16>().ok()?)
    };
    Some((host, port))
}

fn pair_ip(bind: &str) -> String {
    match bind_parts(bind) {
        Some((host, _)) if !is_loopback_bind(&host) => host,
        _ => "127.0.0.1".to_string(),
    }
}

fn pid_path(flags: &HashMap<String, String>) -> PathBuf {
    data_dir_for(flags).join("roamway.pid")
}

fn read_pid(flags: &HashMap<String, String>) -> Option<(u32, String)> {
    let s = std::fs::read_to_string(pid_path(flags)).ok()?;
    let mut lines = s.lines();
    let pid = lines.next()?.trim().parse::<u32>().ok()?;
    let bind = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1:8796".to_string());
    Some((pid, bind))
}

fn port_open(bind: &str) -> bool {
    let (host, port) = match bind_parts(bind) {
        Some(p) => p,
        None => return false,
    };
    let host = if matches!(host.as_str(), "0.0.0.0" | "::" | "[::]") {
        "127.0.0.1"
    } else {
        &host
    };
    let addr = format!("{}:{}", host, port);
    match addr.to_socket_addrs().ok().and_then(|mut it| it.next()) {
        Some(a) => TcpStream::connect_timeout(&a, Duration::from_millis(300)).is_ok(),
        None => false,
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        usage();
        return;
    }
    let cmd = argv[0].clone();
    let (flags, pos) = parse_flags(&argv[1..]);
    match cmd.as_str() {
        "serve" => cmd_serve(&flags),
        "clone" => cmd_clone(&flags, &pos),
        "workspace" => cmd_workspace(&flags, &pos),
        "terminal" | "term" => cmd_terminal(&flags, &pos),
        "script" | "scripts" => cmd_script(&flags, &pos),
        "provider" => cmd_provider(&flags, &pos),
        "usage" => cmd_usage(&flags, &pos),
        "plugin" => cmd_plugin(&flags, &pos),
        "speech" => cmd_speech(&flags, &pos),
        "run" => cmd_run(&flags, &pos),
        "ls" | "list" => cmd_ls(&flags),
        "status" => {
            if pos.is_empty() {
                cmd_status_summary(&flags);
            } else {
                cmd_status(&flags, &pos);
            }
        }
        "schedule" => cmd_schedule(&flags, &pos),
        "hooks" => cmd_hooks(&flags),
        "hub" => cmd_hub(&flags, &pos),
        "agent" => cmd_agent(&flags, &pos),
        "attach" => cmd_agent_attach(&flags, &pos),
        "inspect" => cmd_agent_inspect(&flags, &pos),
        "wait" => cmd_agent_wait(&flags, &pos),
        "archive" => cmd_agent_archive(&flags, &pos),
        "import" => cmd_agent_import(&flags, &pos),
        "logs" => cmd_logs(&flags, &pos),
        "stop" => cmd_stop(&flags, &pos),
        "rm" => cmd_rm(&flags, &pos),
        "jobs" => cmd_jobs(&flags),
        "jobs-add" => cmd_jobs_add(&flags, &pos),
        "jobs-run" => cmd_jobs_run(&flags, &pos),
        "jobs-rm" => cmd_jobs_rm(&flags, &pos),
        "health" => cmd_health(&flags),
        "host" => cmd_host(&flags, &pos),
        "token" => cmd_token(&flags, &pos),
        "pair" => cmd_pair(&flags),
        "approve" => cmd_approve(&flags, &pos),
        "deny" => cmd_deny(&flags, &pos),
        "approvals" => cmd_approvals(&flags),
        "daemon" => cmd_daemon(&flags, &pos),
        "onboard" => cmd_onboard(&flags),
        "restart" => cmd_restart(&flags),
        "reload" => cmd_reload(&flags),
        "version" | "-V" | "--version" => println!("roamway {}", env!("CARGO_PKG_VERSION")),
        "-h" | "--help" | "help" => usage(),
        _ => {
            eprintln!("unknown command: {}", cmd);
            usage();
        }
    }
}

fn default_data_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".roamway")
}

fn is_loopback_bind(bind: &str) -> bool {
    let bind = bind.trim();
    if bind == "127.0.0.1" || bind == "::1" || bind == "localhost" {
        return true;
    }
    if let Some(rest) = bind.strip_prefix('[') {
        let host = rest.split(']').next().unwrap_or("");
        return host == "::1";
    }
    match bind.rsplit_once(':') {
        Some((host, _)) => matches!(host, "127.0.0.1" | "localhost" | "::1"),
        None => false,
    }
}

fn cmd_serve(flags: &HashMap<String, String>) {
    // 守护进程常见坑：非登录 shell 的 PATH 找不到 ~/.npm-global/bin 里的 codex
    if let Ok(home) = std::env::var("HOME") {
        let extra = format!("{}/.npm-global/bin:{}/.local/bin:/usr/local/bin", home, home);
        let cur = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", extra, cur));
    }
    let bind = flags
        .get("bind")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:8796".into());
    let data = flags
        .get("data")
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
    let token = flags
        .get("token")
        .cloned()
        .or_else(|| std::env::var("ROAMWAY_TOKEN").ok())
        .filter(|t| !t.is_empty());
    let tz: i64 = flags
        .get("tz")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let max_concurrent: usize = flags
        .get("max-concurrent")
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(4);
    let max_conn: usize = flags
        .get("max-conn")
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(64);
    let allow_hosts = flag_many(flags, "allow-host");
    let allow_origins = flag_many(flags, "allow-origin");
    let require_approval = flag_many(flags, "require-approval");
    let unix_socket = flags.get("unix-socket").map(PathBuf::from);
    let tls_cert = flags.get("tls-cert").map(PathBuf::from);
    let tls_key = flags.get("tls-key").map(PathBuf::from);
    let bind_host = bind_parts(&bind).map(|(h, _)| h).unwrap_or_else(|| bind.clone());

    // 无 token 且绑定非 loopback 时拒绝启动，避免把任意命令执行暴露给整个网络
    let token_store = auth::TokenStore::load(&data);
    let has_saved_tokens = !token_store.tokens.is_empty();
    if token.is_none() && !has_saved_tokens && !is_loopback_bind(&bind) {
        eprintln!(
            "roamway: refusing to serve without --token on non-loopback bind {}; use 127.0.0.1/::1 or set ROAMWAY_TOKEN",
            bind
        );
        std::process::exit(2);
    }

    let store = Store::load(&data);
    let loaded_ok = store.loaded_ok;
    let schedule_store = schedules::ScheduleStore::load(&data);
    // 受管终端：元数据落 data/terminals.json，运行态只在内存，重启后标 exited 不复活
    let terminal_store = terminals::TerminalStore::load(&data);
    let approval_store = auth::ApprovalStore::load(&data);
    let password_store = auth::PasswordStore::load(&data);
    let pairing_store = auth::PairingStore::load(&data);
    let app = Arc::new(App {
        store: Mutex::new(store),
        schedules: Mutex::new(schedule_store),
        terms: Mutex::new(terminal_store),
        term_rt: Mutex::new(HashMap::new()),
        token: token.clone(),
        started: now(),
        cancels: Mutex::new(HashMap::new()),
        tz_offset_hours: tz,
        max_concurrent,
        max_conn,
        conns: AtomicUsize::new(0),
        tokens: Mutex::new(token_store),
        approvals: Mutex::new(approval_store),
        passwords: Mutex::new(password_store),
        pairings: Mutex::new(pairing_store),
        hub_rt: Mutex::new(crate::hub::HubRuntime::default()),
        allow_hosts,
        allow_origins,
        require_approval,
        bind_host,
        unix_socket,
        tls_cert,
        tls_key,
    });
    if loaded_ok {
        app.save();
    }

    jobs::start_scheduler(app.clone());
    schedules::start_scheduler(app.clone());
    terminals::start_reaper(app.clone());

    let hb = app.clone();
    thread::spawn(move || loop {
        let p = {
            let st = hb.store.lock().unwrap_or_else(|e| e.into_inner());
            st.dir.join("heartbeat")
        };
        let _ = std::fs::write(&p, format!("{}\n", iso(now())));
        thread::sleep(Duration::from_secs(5));
    });

    println!(
        "roamway {} | data={} | auth={} | tz=+{} | max-concurrent={} | max-conn={}",
        env!("CARGO_PKG_VERSION"),
        data.display(),
        if token.is_some() { "token" } else if has_saved_tokens { "tokens" } else { "off" },
        tz,
        max_concurrent,
        max_conn
    );
    if let Err(e) = server::serve(app, &bind) {
        eprintln!("serve failed: {}", e);
        std::process::exit(1);
    }
}

fn cmd_run(flags: &HashMap<String, String>, pos: &[String]) {
    let mut prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        prompt = flags.get("prompt").cloned().unwrap_or_default();
    }
    if prompt.trim().is_empty() {
        eprintln!("prompt required");
        return;
    }
    if let Some(h) = resolve_host(flags) {
        cmd_run_remote(&h, flags, &pos);
        return;
    }
    let c = local_client(flags);
    let body = json!({
        "agent": flags.get("agent").cloned().unwrap_or_else(|| "codex".into()),
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
        "title": flags.get("title"),
        "prompt": prompt,
        "timeout_s": flags.get("timeout").and_then(|v| v.parse::<u64>().ok()),
    });
    let v = match c.req("POST", "/v1/runs", Some(body)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("dispatch failed: {}", e);
            return;
        }
    };
    let id = v["run"]["id"].as_str().unwrap_or("").to_string();
    if flags.contains_key("no-wait") {
        println!("{}", id);
        return;
    }
    let started = std::time::Instant::now();
    loop {
        thread::sleep(Duration::from_secs(2));
        match c.req("GET", &format!("/v1/runs/{}", id), None) {
            Ok(v) => {
                let st = v["run"]["status"].as_str().unwrap_or("?");
                if matches!(st, "done" | "failed" | "timeout" | "killed" | "interrupted") {
                    let err = v["run"]["error"].as_str().unwrap_or("");
                    println!(
                        "{}\t{}\t{}s{}",
                        id,
                        st,
                        started.elapsed().as_secs(),
                        if err.is_empty() {
                            String::new()
                        } else {
                            format!("\t{}", err)
                        }
                    );
                    if let Ok(l) = c.req("GET", &format!("/v1/runs/{}/log?tail=40", id), None) {
                        println!("---");
                        println!("{}", l["log"].as_str().unwrap_or(""));
                    }
                    return;
                }
                if flags.contains_key("quiet") {
                    continue;
                }
                eprintln!("[{}] {} {}s", id, st, started.elapsed().as_secs());
            }
            Err(e) => {
                eprintln!("poll failed: {}", e);
                return;
            }
        }
    }
}

fn trunc(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s.to_string()
    } else {
        chars[..n].iter().collect::<String>() + "…"
    }
}

fn cmd_ls(flags: &HashMap<String, String>) {
    if let Some(raw) = host_flag(flags) {
        if raw == "all" {
            cmd_ls_all(flags);
            return;
        }
        if let Some(h) = resolve_host(flags) {
            cmd_ls_remote(&h, flags);
            return;
        }
    }
    let c = local_client(flags);
    let limit = flags
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20);
    let trigger = flags.get("trigger").cloned().filter(|t| !t.is_empty() && t != "true");
    let mut url = format!("/v1/runs?limit={}", limit);
    if let Some(t) = trigger {
        url.push_str(&format!("&trigger={}", t));
    }
    match c.req("GET", &url, None) {
        Ok(v) => {
            let empty = vec![];
            let runs = v["runs"].as_array().unwrap_or(&empty);
            println!(
                "{:<12} {:<12} {:<8} {:<20} {:<26} {}",
                "ID", "STATUS", "AGENT", "MODEL", "CREATED", "TITLE"
            );
            for r in runs {
                println!(
                    "{:<12} {:<12} {:<8} {:<20} {:<26} {}",
                    r["id"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["agent"].as_str().unwrap_or(""),
                    trunc(r["model"].as_str().unwrap_or("-"), 20),
                    r["created_at"]
                        .as_i64()
                        .map(iso)
                        .unwrap_or_else(|| "-".into()),
                    trunc(r["title"].as_str().unwrap_or(""), 40)
                );
            }
        }
        Err(e) => eprintln!("list failed: {}", e),
    }
}

fn cmd_status(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway status ID"),
    };
    if let Some(h) = resolve_host(flags) {
        cmd_status_remote(&h, id);
        return;
    }
    let c = local_client(flags);
    match c.req("GET", &format!("/v1/runs/{}", id), None) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v["run"]).unwrap_or_default()),
        Err(e) => eprintln!("status failed: {}", e),
    }
}

fn cmd_logs(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway logs ID [--tail 80]"),
    };
    let tail = flags.get("tail").cloned().unwrap_or_else(|| "80".into());
    if let Some(h) = resolve_host(flags) {
        cmd_logs_remote(&h, id, &tail);
        return;
    }
    let c = local_client(flags);
    match c.req("GET", &format!("/v1/runs/{}/log?tail={}", id, tail), None) {
        Ok(v) => println!("{}", v["log"].as_str().unwrap_or("")),
        Err(e) => eprintln!("logs failed: {}", e),
    }
}

fn cmd_stop(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway stop ID"),
    };
    if let Some(h) = resolve_host(flags) {
        cmd_stop_remote(&h, id);
        return;
    }
    let c = local_client(flags);
    match c.req("POST", &format!("/v1/runs/{}/stop", id), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("stop failed: {}", e),
    }
}

fn cmd_rm(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway rm ID"),
    };
    let c = local_client(flags);
    match c.req("DELETE", &format!("/v1/runs/{}", id), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("rm failed: {}", e),
    }
}

fn cmd_jobs(flags: &HashMap<String, String>) {
    let c = local_client(flags);
    match c.req("GET", "/v1/jobs", None) {
        Ok(v) => {
            let empty = vec![];
            let jobs = v["jobs"].as_array().unwrap_or(&empty);
            println!(
                "{:<10} {:<5} {:<8} {:<10} {:<26} {:<20} {}",
                "ID", "ON", "AGENT", "EVERY", "NEXT", "LAST", "TITLE"
            );
            for j in jobs {
                let every = j["every_secs"]
                    .as_u64()
                    .map(|s| format!("{}s", s))
                    .or_else(|| j["daily"].as_str().map(|d| format!("@{}", d)))
                    .unwrap_or_else(|| "-".into());
                println!(
                    "{:<10} {:<5} {:<8} {:<10} {:<26} {:<20} {}",
                    j["id"].as_str().unwrap_or(""),
                    if j["enabled"].as_bool().unwrap_or(false) { "yes" } else { "no" },
                    j["agent"].as_str().unwrap_or(""),
                    every,
                    j["next_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                    j["last_run"].as_str().unwrap_or("-"),
                    trunc(j["title"].as_str().unwrap_or(""), 36)
                );
            }
        }
        Err(e) => eprintln!("jobs failed: {}", e),
    }
}

fn cmd_jobs_add(flags: &HashMap<String, String>, pos: &[String]) {
    let prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        eprintln!("prompt required");
        return;
    }
    let mut body = json!({
        "agent": flags.get("agent").cloned().unwrap_or_else(|| "shell".into()),
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
        "title": flags.get("title"),
        "prompt": prompt,
        "timeout_s": flags.get("timeout").and_then(|v| v.parse::<u64>().ok()),
        "max_retries": flags.get("max-retries").and_then(|v| v.parse::<u32>().ok()),
        "allow_overlap": flags.contains_key("allow-overlap"),
        "enabled": !flags.contains_key("disabled"),
    });
    if let Some(e) = flags.get("every") {
        body["every_secs"] = json!(e.parse::<u64>().ok());
    }
    if let Some(d) = flags.get("daily") {
        body["daily"] = json!(d);
    }
    if let Some(tz) = flags.get("tz") {
        body["tz_offset_hours"] = json!(tz.parse::<i64>().ok());
    }
    let c = local_client(flags);
    match c.req("POST", "/v1/jobs", Some(body)) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v["job"]).unwrap_or_default()),
        Err(e) => eprintln!("jobs-add failed: {}", e),
    }
}

fn cmd_jobs_run(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway jobs-run ID"),
    };
    let c = local_client(flags);
    match c.req("POST", &format!("/v1/jobs/{}/run", id), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("jobs-run failed: {}", e),
    }
}

fn cmd_jobs_rm(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway jobs-rm ID"),
    };
    let c = local_client(flags);
    match c.req("DELETE", &format!("/v1/jobs/{}", id), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("jobs-rm failed: {}", e),
    }
}

fn cmd_schedule(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("help") {
        "create" => cmd_schedule_create(flags, &pos[1..]),
        "ls" | "list" => cmd_schedule_ls(flags),
        "inspect" | "show" => cmd_schedule_inspect(flags, &pos[1..]),
        "logs" => cmd_schedule_logs(flags, &pos[1..]),
        "pause" => cmd_schedule_pause(flags, &pos[1..]),
        "resume" => cmd_schedule_resume(flags, &pos[1..]),
        "delete" | "rm" | "remove" => cmd_schedule_delete(flags, &pos[1..]),
        "run-once" | "run" => cmd_schedule_run_once(flags, &pos[1..]),
        "update" => cmd_schedule_update(flags, &pos[1..]),
        _ => eprintln!(
            "usage: roamway schedule create|ls|inspect|logs|pause|resume|delete|run-once|update ..."
        ),
    }
}

fn cmd_schedule_create(flags: &HashMap<String, String>, pos: &[String]) {
    let name = flags.get("name").cloned().unwrap_or_default();
    if name.trim().is_empty() {
        eprintln!("--name is required");
        return;
    }
    let prompt = pos.join(" ");
    let prompt = if prompt.trim().is_empty() {
        flags.get("prompt").cloned().unwrap_or_default()
    } else {
        prompt
    };
    if prompt.trim().is_empty() {
        eprintln!("prompt is required");
        return;
    }
    let body = json!({
        "name": name,
        "prompt": prompt,
        "agent": flags.get("agent"),
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
        "cron": flags.get("cron"),
        "every": flags.get("every"),
        "timezone": flags.get("tz"),
        "max_runs": flags.get("max-runs").and_then(|v| v.parse::<u64>().ok()),
        "expires_in": flags.get("expires-in").and_then(|v| v.parse::<i64>().ok()),
        "run_now": flags.contains_key("run-now"),
        "overlap": flags.contains_key("overlap"),
    });
    match request(flags, "POST", "/v1/schedules", Some(body)) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v["schedule"]).unwrap_or_default());
            } else {
                let s = &v["schedule"];
                println!(
                    "created {} {}\tnext={}",
                    s["id"].as_str().unwrap_or(""),
                    s["name"].as_str().unwrap_or(""),
                    s["next_run_at"].as_i64().map(iso).unwrap_or_else(|| "-".into())
                );
            }
        }
        Err(e) => eprintln!("schedule create failed: {}", e),
    }
}

fn cmd_schedule_ls(flags: &HashMap<String, String>) {
    let path = if flags.contains_key("all") {
        "/v1/schedules?all=1"
    } else {
        "/v1/schedules"
    };
    match request(flags, "GET", path, None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return;
            }
            let empty = vec![];
            let scheds = v["schedules"].as_array().unwrap_or(&empty);
            println!(
                "{:<13} {:<20} {:<26} {:<6} {:<26} {}",
                "ID", "NAME", "NEXT", "RUNS", "LAST", "CRON"
            );
            for s in scheds {
                println!(
                    "{:<13} {:<20} {:<26} {:<6} {:<26} {}",
                    s["id"].as_str().unwrap_or(""),
                    trunc(s["name"].as_str().unwrap_or(""), 20),
                    s["next_run_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                    s["runs_count"].as_u64().unwrap_or(0),
                    s["last_run_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                    trunc(s["cron"].as_str().unwrap_or("-"), 24)
                );
            }
        }
        Err(e) => eprintln!("schedule ls failed: {}", e),
    }
}

fn cmd_schedule_inspect(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return eprintln!("usage: roamway schedule inspect <id|name> [--json]"),
    };
    match request(flags, "GET", &format!("/v1/schedules/{}", key), None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return;
            }
            println!("{}", serde_json::to_string_pretty(&v["schedule"]).unwrap_or_default());
            let empty = vec![];
            let runs = v["runs"].as_array().unwrap_or(&empty);
            println!("{:<12} {:<10} {:<26} {}", "RUN", "STATUS", "CREATED", "ERROR");
            for r in runs {
                println!(
                    "{:<12} {:<10} {:<26} {}",
                    r["id"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["created_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                    trunc(r["error"].as_str().unwrap_or(""), 40)
                );
            }
        }
        Err(e) => eprintln!("schedule inspect failed: {}", e),
    }
}

fn cmd_schedule_logs(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return eprintln!("usage: roamway schedule logs <id|name> [--tail 20]"),
    };
    let tail = flags.get("tail").cloned().unwrap_or_else(|| "20".into());
    match request(flags, "GET", &format!("/v1/schedules/{}/logs?tail={}", key, tail), None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                println!("{}", v["logs"].as_str().unwrap_or(""));
            }
        }
        Err(e) => eprintln!("schedule logs failed: {}", e),
    }
}

fn cmd_schedule_pause(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return eprintln!("usage: roamway schedule pause <id|name>"),
    };
    match request(flags, "POST", &format!("/v1/schedules/{}/pause", key), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule pause failed: {}", e),
    }
}

fn cmd_schedule_resume(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return eprintln!("usage: roamway schedule resume <id|name>"),
    };
    match request(flags, "POST", &format!("/v1/schedules/{}/resume", key), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule resume failed: {}", e),
    }
}

fn cmd_schedule_delete(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return eprintln!("usage: roamway schedule delete <id|name>"),
    };
    match request(flags, "DELETE", &format!("/v1/schedules/{}", key), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule delete failed: {}", e),
    }
}

fn cmd_schedule_run_once(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return eprintln!("usage: roamway schedule run-once <id|name>"),
    };
    match request(flags, "POST", &format!("/v1/schedules/{}/run", key), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule run-once failed: {}", e),
    }
}

fn cmd_schedule_update(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return eprintln!("usage: roamway schedule update <id|name> [--cron|--every|--prompt|--max-runs|--tz|--enabled]"),
    };
    let mut body = json!({
        "name": flags.get("name"),
        "cron": flags.get("cron"),
        "every": flags.get("every"),
        "prompt": flags.get("prompt"),
        "max_runs": flags.get("max-runs").and_then(|v| v.parse::<u64>().ok()),
        "timezone": flags.get("tz"),
        "enabled": flags.contains_key("enabled").then(|| true),
    });
    if flags.contains_key("disabled") {
        body["enabled"] = json!(false);
    }
    match request(flags, "PATCH", &format!("/v1/schedules/{}", key), Some(body)) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v["schedule"]).unwrap_or_default()),
        Err(e) => eprintln!("schedule update failed: {}", e),
    }
}

fn cmd_hooks(flags: &HashMap<String, String>) {
    match request(flags, "GET", "/v1/hooks", None) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => eprintln!("hooks failed: {}", e),
    }
}

fn load_hub_or_exit(flags: &HashMap<String, String>) -> hub::Hub {
    match hub::load(&data_dir_for(flags)) {
        Some(h) => h,
        None => {
            eprintln!("hub not initialized (run 'roamway hub init')");
            std::process::exit(2);
        }
    }
}

fn cmd_hub(flags: &HashMap<String, String>, pos: &[String]) {
    let sub = pos.first().map(String::as_str).unwrap_or("");
    match sub {
        "" => cmd_hub_overview(flags),
        "init" => cmd_hub_init(flags),
        "permissions" | "perm" => cmd_hub_permissions(flags, pos),
        "projects" | "project" => cmd_hub_projects(flags, pos),
        "deploy" => cmd_hub_deploy(flags),
        "logout" => cmd_hub_logout(flags),
        "status" => cmd_hub_status(flags),
        "token" => cmd_hub_token(flags, pos),
        other => {
            eprintln!("unknown hub subcommand: {}", other);
            eprintln!("usage: roamway hub [init|permissions|projects|deploy|logout|status|token rotate]");
            std::process::exit(2);
        }
    }
}

fn cmd_hub_overview(flags: &HashMap<String, String>) {
    let h = load_hub_or_exit(flags);
    let enabled = h.triggers.iter().filter(|t| t.enabled).count();
    println!("hub_id      {}", h.hub_id);
    println!("name        {}", h.name);
    println!("created_at  {}", h.created_at);
    println!("projects    {}", h.projects.len());
    println!("permissions {}", h.permissions.len());
    println!("triggers    {} (enabled {})", h.triggers.len(), enabled);
}

fn cmd_hub_init(flags: &HashMap<String, String>) {
    let dir = data_dir_for(flags);
    let _ = std::fs::create_dir_all(&dir);
    let name = flags.get("name").cloned().filter(|s| !s.is_empty());
    let force = flags.contains_key("force");
    match hub::init(&dir, name, force) {
        Ok(h) => {
            println!("hub_id  {}", h.hub_id);
            println!("name    {}", h.name);
            println!("next:   roamway hub projects add <path>");
            println!("        roamway hub deploy --file .paseo/organization.json");
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    }
}

fn cmd_hub_permissions(flags: &HashMap<String, String>, pos: &[String]) {
    let action = pos.get(1).map(String::as_str).unwrap_or("ls");
    match action {
        "ls" => {
            let h = load_hub_or_exit(flags);
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&h.permissions).unwrap_or_default());
            } else {
                for p in &h.permissions {
                    println!("{:<20} {:<8} {}", p.subject, p.scope, p.granted_at);
                }
            }
        }
        "grant" => {
            let subject = pos.get(2).cloned().unwrap_or_default();
            let scope = pos.get(3).cloned().unwrap_or_default();
            if subject.is_empty() || scope.is_empty() {
                eprintln!("usage: roamway hub permissions grant <subject> <scope>");
                std::process::exit(2);
            }
            if !hub::valid_scope(&scope) {
                eprintln!("invalid scope '{}' (read|run|admin)", scope);
                std::process::exit(2);
            }
            let dir = data_dir_for(flags);
            let mut h = load_hub_or_exit(flags);
            hub::grant(&mut h, &subject, &scope, &iso(now()));
            if let Err(e) = hub::save(&dir, &h) {
                eprintln!("save failed: {}", e);
                std::process::exit(2);
            }
            println!("granted {} {}", scope, subject);
        }
        "revoke" => {
            let subject = pos.get(2).cloned().unwrap_or_default();
            if subject.is_empty() {
                eprintln!("usage: roamway hub permissions revoke <subject>");
                std::process::exit(2);
            }
            let dir = data_dir_for(flags);
            let mut h = load_hub_or_exit(flags);
            if hub::revoke(&mut h, &subject) {
                if let Err(e) = hub::save(&dir, &h) {
                    eprintln!("save failed: {}", e);
                    std::process::exit(2);
                }
                println!("revoked {}", subject);
            } else {
                eprintln!("no such permission: {}", subject);
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!("usage: roamway hub permissions [ls|grant <subject> <scope>|revoke <subject>]");
            std::process::exit(2);
        }
    }
}

fn cmd_hub_projects(flags: &HashMap<String, String>, pos: &[String]) {
    let action = pos.get(1).map(String::as_str).unwrap_or("ls");
    match action {
        "ls" => {
            let h = load_hub_or_exit(flags);
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&h.projects).unwrap_or_default());
            } else {
                for p in &h.projects {
                    println!("{}", p);
                }
            }
        }
        "add" => {
            let path = pos.get(2).cloned().unwrap_or_default();
            if path.is_empty() {
                eprintln!("usage: roamway hub projects add <path>");
                std::process::exit(2);
            }
            if !std::path::Path::new(&path).is_dir() {
                eprintln!("not a directory: {}", path);
                std::process::exit(2);
            }
            let dir = data_dir_for(flags);
            let mut h = load_hub_or_exit(flags);
            if hub::add_project(&mut h, &path) {
                if let Err(e) = hub::save(&dir, &h) {
                    eprintln!("save failed: {}", e);
                    std::process::exit(2);
                }
                println!("added {}", path);
            } else {
                println!("already registered: {}", path);
            }
        }
        "rm" => {
            let path = pos.get(2).cloned().unwrap_or_default();
            if path.is_empty() {
                eprintln!("usage: roamway hub projects rm <path>");
                std::process::exit(2);
            }
            let dir = data_dir_for(flags);
            let mut h = load_hub_or_exit(flags);
            if hub::remove_project(&mut h, &path) {
                if let Err(e) = hub::save(&dir, &h) {
                    eprintln!("save failed: {}", e);
                    std::process::exit(2);
                }
                println!("removed {}", path);
            } else {
                eprintln!("not registered: {}", path);
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!("usage: roamway hub projects [ls|add <path>|rm <path>]");
            std::process::exit(2);
        }
    }
}

fn cmd_hub_deploy(flags: &HashMap<String, String>) {
    let dir = data_dir_for(flags);
    let file = flags
        .get("file")
        .cloned()
        .unwrap_or_else(|| ".paseo/organization.json".into());
    let path = std::path::PathBuf::from(&file);
    if !path.is_file() {
        eprintln!("organization file not found: {}", file);
        std::process::exit(2);
    }
    match hub::deploy(&dir, &path) {
        Ok((imported, updated, skipped)) => {
            println!("imported {}, updated {}, skipped {}", imported, updated, skipped)
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    }
}

fn cmd_hub_logout(flags: &HashMap<String, String>) {
    let dir = data_dir_for(flags);
    hub::logout(&dir, flags.contains_key("keep-data"));
    println!("logged out");
}

fn cmd_hub_status(flags: &HashMap<String, String>) {
    match request(flags, "GET", "/v1/hub", None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return;
            }
            println!("endpoint  ready");
            println!("hub_id   {}", v["hub_id"].as_str().unwrap_or("-"));
            println!("name     {}", v["name"].as_str().unwrap_or("-"));
            println!(
                "triggers {} (enabled {})",
                v["triggers"].as_u64().unwrap_or(0),
                v["triggers_enabled"].as_u64().unwrap_or(0)
            );
            let dir = data_dir_for(flags);
            if let Some(h) = hub::load(&dir) {
                for t in &h.triggers {
                    println!(
                        "webhook  POST /v1/hub/hooks/{} [{}]",
                        t.id,
                        if t.enabled { "enabled" } else { "disabled" }
                    );
                }
            }
            if let Some(recent) = v["recent"].as_array() {
                println!("recent   {}", recent.len());
                for r in recent {
                    println!(
                        "  {}  {}  {}",
                        r["ts"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                        r["source"].as_str().unwrap_or("-"),
                        r["result"].as_str().unwrap_or("-")
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("hub status failed: {}", e);
            std::process::exit(2);
        }
    }
}

fn cmd_hub_token(flags: &HashMap<String, String>, pos: &[String]) {
    let sub = pos.get(1).map(String::as_str).unwrap_or("");
    if sub != "rotate" {
        eprintln!("usage: roamway hub token rotate");
        std::process::exit(2);
    }
    let dir = data_dir_for(flags);
    let _ = std::fs::create_dir_all(&dir);
    match hub::rotate_token(&dir) {
        Ok(t) => println!("token:   {}   # shown once, store it safely", t),
        Err(e) => {
            eprintln!("rotate failed: {}", e);
            std::process::exit(2);
        }
    }
}
fn cmd_status_summary(flags: &HashMap<String, String>) {
    match request(flags, "GET", "/v1/status", None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return;
            }
            println!("version   {}", v["version"].as_str().unwrap_or("-"));
            println!("uptime    {}s", v["uptime"].as_u64().unwrap_or(0));
            println!("running   {}", v["running"].as_u64().unwrap_or(0));
            println!("queued    {}", v["queued"].as_u64().unwrap_or(0));
            println!("max_conn  {}", v["max_concurrent"].as_u64().unwrap_or(0));
            println!(
                "schedules {} (enabled {})",
                v["schedules"].as_u64().unwrap_or(0),
                v["enabled_schedules"].as_u64().unwrap_or(0)
            );
            println!(
                "next_run  {}",
                v["next_run_at"].as_i64().map(iso).unwrap_or_else(|| "-".into())
            );
            println!(
                "last_fail {}",
                v["last_failure"].as_i64().map(iso).unwrap_or_else(|| "-".into())
            );
        }
        Err(e) => eprintln!("status failed: {}", e),
    }
}
fn cmd_health(flags: &HashMap<String, String>) {
    let c = local_client(flags);
    match c.req("GET", "/health", None) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => eprintln!("health failed: {}", e),
    }
}



fn print_workspaces_table(text: &str) {
    let v: Value = serde_json::from_str(text).unwrap_or_else(|_| json!({}));
    let empty = vec![];
    let workspaces = v["workspaces"].as_array().unwrap_or(&empty);
    println!(
        "{:<10} {:<16} {:<26} {:<8} {}",
        "ID", "NAME", "PATH", "HOST", "CREATED"
    );
    for w in workspaces {
        println!(
            "{:<10} {:<16} {:<26} {:<8} {}",
            w["id"].as_str().unwrap_or(""),
            w["name"].as_str().unwrap_or(""),
            w["path"].as_str().unwrap_or(""),
            w["host"].as_str().unwrap_or(""),
            w["created_at"].as_i64().map(iso).unwrap_or_else(|| "-".into())
        );
    }
}

fn cmd_clone(flags: &HashMap<String, String>, pos: &[String]) {
    let input = match pos.first() {
        Some(i) if !i.trim().is_empty() => i.clone(),
        _ => {
            eprintln!("usage: roamway clone <owner/repo | git url> [--dir DIR] [--protocol https|ssh] [--json] [--host H]");
            return;
        }
    };
    let protocol = flags.get("protocol").cloned().unwrap_or_else(|| "https".into());
    let dir = flags.get("dir").cloned();
    if let Some(h) = resolve_host(flags) {
        let body = json!({"input": input, "dir": dir, "protocol": protocol});
        let body_str = serde_json::to_string(&body).unwrap_or_default();
        match hosts::dispatch(&h, "POST", "/v1/workspaces/clone", Some(&body_str)) {
            Ok((code, text)) if (200..300).contains(&code) => {
                let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
                if flags.contains_key("json") {
                    println!("{}", serde_json::to_string_pretty(&v["workspace"]).unwrap_or_default());
                } else {
                    println!(
                        "cloned {} → {}",
                        v["workspace"]["name"].as_str().unwrap_or(""),
                        v["workspace"]["path"].as_str().unwrap_or("")
                    );
                }
            }
            Ok((code, text)) => eprintln!("clone failed: HTTP {}: {}", code, text),
            Err(e) => eprintln!("clone failed: {}", e),
        }
        return;
    }
    match workspaces::clone_repo(&data_dir_for(flags), &input, dir.as_deref(), &protocol) {
        Ok(ws) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&ws).unwrap_or_default());
            } else {
                println!("cloned {} → {}", ws.name, ws.path);
            }
        }
        Err(e) => eprintln!("clone failed: {}", e),
    }
}

fn cmd_workspace(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("help") {
        "ls" | "list" => cmd_workspace_ls(flags),
        "rm" | "remove" => cmd_workspace_rm(flags, &pos[1..]),
        _ => eprintln!("usage: roamway workspace ls [--json] | workspace rm <name>"),
    }
}

fn cmd_workspace_ls(flags: &HashMap<String, String>) {
    if let Some(h) = resolve_host(flags) {
        match hosts::dispatch(&h, "GET", "/v1/workspaces", None) {
            Ok((code, text)) if (200..300).contains(&code) => {
                if flags.contains_key("json") {
                    println!("{}", text);
                } else {
                    print_workspaces_table(&text);
                }
            }
            Ok((code, text)) => eprintln!("workspace ls failed: HTTP {}: {}", code, text),
            Err(e) => eprintln!("workspace ls failed: {}", e),
        }
        return;
    }
    let reg = workspaces::Registry::load(&data_dir_for(flags));
    if flags.contains_key("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"workspaces": reg.workspaces})).unwrap_or_default()
        );
        return;
    }
    println!(
        "{:<10} {:<16} {:<26} {:<8} {}",
        "ID", "NAME", "PATH", "HOST", "CREATED"
    );
    for w in &reg.workspaces {
        println!(
            "{:<10} {:<16} {:<26} {:<8} {}",
            w.id,
            w.name,
            w.path,
            w.host,
            iso(w.created_at)
        );
    }
}

fn cmd_workspace_rm(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) if !n.trim().is_empty() => n,
        _ => return eprintln!("usage: roamway workspace rm <name>"),
    };
    if let Some(h) = resolve_host(flags) {
        let path = format!("/v1/workspaces/{}", name);
        match hosts::dispatch(&h, "DELETE", &path, None) {
            Ok((code, text)) if (200..300).contains(&code) => println!("{}", text),
            Ok((code, text)) => eprintln!("workspace rm failed: HTTP {}: {}", code, text),
            Err(e) => eprintln!("workspace rm failed: {}", e),
        }
        return;
    }
    let mut reg = workspaces::Registry::load(&data_dir_for(flags));
    if reg.remove(name) {
        match reg.save() {
            Ok(()) => println!("removed {}", name),
            Err(e) => eprintln!("workspace rm failed: {}", e),
        }
    } else {
        eprintln!("no such workspace: {}", name);
    }
}
fn cmd_terminal(flags: &HashMap<String, String>, pos: &[String]) {
    let c = local_client(flags);
    let json_out = flags.contains_key("json");
    match pos.first().map(String::as_str).unwrap_or("help") {
        "create" | "new" => {
            let mut body = json!({
                "name": flags.get("name").cloned().unwrap_or_default(),
                "workspace": flags.get("workspace").cloned().unwrap_or_default(),
                "shell": flags.get("shell").cloned().unwrap_or_default(),
            });
            if let Some(kb) = flags.get("buffer-kb").and_then(|v| v.parse::<u64>().ok()) {
                body["buffer_kb"] = json!(kb);
            }
            match c.req("POST", "/v1/terminals", Some(body)) {
                Ok(v) => {
                    if json_out {
                        println!("{}", serde_json::to_string_pretty(&v["terminal"]).unwrap_or_default());
                    } else {
                        let t = &v["terminal"];
                        println!(
                            "{}\t{}\t{}\t{}",
                            t["id"].as_str().unwrap_or(""),
                            t["state"].as_str().unwrap_or(""),
                            t["pid"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                            t["workspace"].as_str().unwrap_or("")
                        );
                    }
                }
                Err(e) => eprintln!("terminal create failed: {}", e),
            }
        }
        "ls" | "list" => match c.req("GET", "/v1/terminals", None) {
            Ok(v) => {
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    return;
                }
                let empty = Vec::new();
                let list = v["terminals"].as_array().unwrap_or(&empty);
                println!(
                    "{:<14} {:<16} {:<8} {:<7} {:>9} {}",
                    "ID", "NAME", "STATE", "PID", "BYTES", "WORKSPACE"
                );
                for t in list {
                    println!(
                        "{:<14} {:<16} {:<8} {:<7} {:>9} {}",
                        t["id"].as_str().unwrap_or(""),
                        t["name"].as_str().unwrap_or(""),
                        t["state"].as_str().unwrap_or(""),
                        t["pid"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                        t["bytes_out"].as_u64().unwrap_or(0),
                        t["workspace"].as_str().unwrap_or("")
                    );
                }
            }
            Err(e) => eprintln!("terminal ls failed: {}", e),
        },
        "capture" | "output" => {
            let id = match pos.get(1).filter(|s| !s.trim().is_empty()) {
                Some(v) => v,
                None => return eprintln!("usage: roamway terminal capture <id> [--since N] [--tail N]"),
            };
            let mut path = format!("/v1/terminals/{}/output", id);
            let mut qs: Vec<String> = Vec::new();
            if let Some(s) = flags.get("since") {
                qs.push(format!("since={}", s));
            }
            if let Some(t) = flags.get("tail") {
                qs.push(format!("tail={}", t));
            }
            if !qs.is_empty() {
                path.push('?');
                path.push_str(&qs.join("&"));
            }
            match c.req("GET", &path, None) {
                Ok(v) => {
                    if json_out {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    } else {
                        print!("{}", v["output"].as_str().unwrap_or(""));
                    }
                }
                Err(e) => eprintln!("terminal capture failed: {}", e),
            }
        }
        "send-keys" | "send" | "keys" => {
            let id = match pos.get(1).filter(|s| !s.trim().is_empty()) {
                Some(v) => v,
                None => return eprintln!("usage: roamway terminal send-keys <id> <keys...> [--enter]"),
            };
            let keys = pos[2..].join(" ");
            let enter = flags.contains_key("enter") && !flags.contains_key("no-enter");
            let body = json!({ "keys": keys, "enter": enter });
            match c.req("POST", &format!("/v1/terminals/{}/keys", id), Some(body)) {
                Ok(v) => println!("sent {} bytes", v["sent"].as_u64().unwrap_or(0)),
                Err(e) => eprintln!("terminal send-keys failed: {}", e),
            }
        }
        "kill" | "rm" | "close" => {
            let id = match pos.get(1).filter(|s| !s.trim().is_empty()) {
                Some(v) => v,
                None => return eprintln!("usage: roamway terminal kill <id>"),
            };
            match c.req("DELETE", &format!("/v1/terminals/{}", id), None) {
                Ok(_) => println!("killed {}", id),
                Err(e) => eprintln!("terminal kill failed: {}", e),
            }
        }
        _ => eprintln!(
            "usage: roamway terminal create|ls|capture|send-keys|kill  (受管终端无 TTY，TUI 程序不支持)"
        ),
    }
}

fn cmd_script(flags: &HashMap<String, String>, pos: &[String]) {
    let c = local_client(flags);
    let json_out = flags.contains_key("json");
    match pos.first().map(String::as_str).unwrap_or("help") {
        "ls" | "list" => match c.req("GET", "/v1/scripts", None) {
            Ok(v) => {
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    return;
                }
                let empty = Vec::new();
                let list = v["scripts"].as_array().unwrap_or(&empty);
                println!("{:<16} {:<9} {:<7} {:<6} {}", "NAME", "STATE", "PID", "EXIT", "CMD");
                for s in list {
                    println!(
                        "{:<16} {:<9} {:<7} {:<6} {}",
                        s["name"].as_str().unwrap_or(""),
                        s["state"].as_str().unwrap_or(""),
                        s["pid"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                        s["last_exit"].as_i64().map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                        s["cmd"].as_str().unwrap_or("")
                    );
                }
            }
            Err(e) => eprintln!("script ls failed: {}", e),
        },
        "add" => {
            let name = flags.get("name").cloned().unwrap_or_default();
            let cmd = flags.get("cmd").cloned().unwrap_or_default();
            if name.trim().is_empty() || cmd.trim().is_empty() {
                return eprintln!("usage: roamway script add --name N --cmd \"...\" [--workspace DIR]");
            }
            let body = json!({
                "name": name,
                "cmd": cmd,
                "workspace": flags.get("workspace").cloned().unwrap_or_default(),
            });
            match c.req("POST", "/v1/scripts", Some(body)) {
                Ok(v) => {
                    if json_out {
                        println!("{}", serde_json::to_string_pretty(&v["script"]).unwrap_or_default());
                    } else {
                        println!("added {}", v["script"]["name"].as_str().unwrap_or(&name));
                    }
                }
                Err(e) => eprintln!("script add failed: {}", e),
            }
        }
        "rm" | "remove" => {
            let name = match pos.get(1).filter(|s| !s.trim().is_empty()) {
                Some(v) => v,
                None => return eprintln!("usage: roamway script rm <name>"),
            };
            match c.req("DELETE", &format!("/v1/scripts/{}", name), None) {
                Ok(_) => println!("removed {}", name),
                Err(e) => eprintln!("script rm failed: {}", e),
            }
        }
        "start" | "stop" => {
            let name = match pos.get(1).filter(|s| !s.trim().is_empty()) {
                Some(v) => v,
                None => return eprintln!("usage: roamway script start|stop <name>"),
            };
            let action = if pos[0] == "start" { "start" } else { "stop" };
            match c.req("POST", &format!("/v1/scripts/{}/{}", name, action), None) {
                Ok(v) => {
                    if json_out {
                        println!("{}", serde_json::to_string_pretty(&v["script"]).unwrap_or_default());
                    } else {
                        let s = &v["script"];
                        println!(
                            "{}\t{}\t{}",
                            s["name"].as_str().unwrap_or(name),
                            s["state"].as_str().unwrap_or(""),
                            s["pid"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| "-".into())
                        );
                    }
                }
                Err(e) => eprintln!("script {} failed: {}", action, e),
            }
        }
        _ => eprintln!("usage: roamway script ls|add|rm|start|stop"),
    }
}

fn cmd_run_remote(h: &hosts::Host, flags: &HashMap<String, String>, pos: &[String]) {
    let mut prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        prompt = flags.get("prompt").cloned().unwrap_or_default();
    }
    if prompt.trim().is_empty() {
        eprintln!("prompt required");
        return;
    }
    let body = json!({
        "agent": flags.get("agent").cloned().unwrap_or_else(|| "codex".into()),
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
        "title": flags.get("title"),
        "prompt": prompt,
        "timeout_s": flags.get("timeout").and_then(|v| v.parse::<u64>().ok()),
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let (code, text) = match hosts::dispatch(h, "POST", "/v1/runs", Some(&body_str)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("dispatch failed: {}", e);
            return;
        }
    };
    if !(200..300).contains(&code) {
        eprintln!("dispatch failed: HTTP {}: {}", code, text);
        return;
    }
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    let mut out = v.clone();
    out["host"] = json!(h.name);
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    let id = v["run"]["id"].as_str().unwrap_or("").to_string();
    if flags.contains_key("no-wait") {
        return;
    }
    let started = std::time::Instant::now();
    loop {
        thread::sleep(Duration::from_secs(2));
        match dispatch_json(h, "GET", &format!("/v1/runs/{}", id)) {
            Ok(v) => {
                let st = v["run"]["status"].as_str().unwrap_or("?");
                if matches!(st, "done" | "failed" | "timeout" | "killed" | "interrupted") {
                    let err = v["run"]["error"].as_str().unwrap_or("");
                    println!(
                        "{}\t{}\t{}s{}",
                        id,
                        st,
                        started.elapsed().as_secs(),
                        if err.is_empty() {
                            String::new()
                        } else {
                            format!("\t{}", err)
                        }
                    );
                    if let Ok(l) = dispatch_json(h, "GET", &format!("/v1/runs/{}/log?tail=40", id)) {
                        println!("---");
                        println!("{}", l["log"].as_str().unwrap_or(""));
                    }
                    return;
                }
                if flags.contains_key("quiet") {
                    continue;
                }
                eprintln!("[{}] {} {}s", id, st, started.elapsed().as_secs());
            }
            Err(e) => {
                eprintln!("poll failed: {}", e);
                return;
            }
        }
    }
}

fn print_runs_table(runs: &[Value]) {
    println!(
        "{:<10} {:<12} {:<12} {:<8} {:<20} {:<26} {}",
        "HOST", "ID", "STATUS", "AGENT", "MODEL", "CREATED", "TITLE"
    );
    for r in runs {
        println!(
            "{:<10} {:<12} {:<12} {:<8} {:<20} {:<26} {}",
            r["host"].as_str().unwrap_or("-"),
            r["id"].as_str().unwrap_or(""),
            r["status"].as_str().unwrap_or(""),
            r["agent"].as_str().unwrap_or(""),
            trunc(r["model"].as_str().unwrap_or("-"), 20),
            r["created_at"]
                .as_i64()
                .map(iso)
                .unwrap_or_else(|| "-".into()),
            trunc(r["title"].as_str().unwrap_or(""), 40)
        );
    }
}

fn cmd_ls_remote(h: &hosts::Host, flags: &HashMap<String, String>) {
    let limit = flags
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20);
    match dispatch_json(h, "GET", &format!("/v1/runs?limit={}", limit)) {
        Ok(v) => {
            let mut runs = v["runs"].as_array().cloned().unwrap_or_default();
            for r in &mut runs {
                r["host"] = json!(h.name);
            }
            print_runs_table(&runs);
        }
        Err(e) => eprintln!("list failed: {}", e),
    }
}

fn cmd_ls_all(flags: &HashMap<String, String>) {
    let limit = flags
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20);
    let reg = registry_for(flags);
    let mut all: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    match local_only_client(flags).req("GET", &format!("/v1/runs?limit={}", limit), None) {
        Ok(v) => {
            for mut r in v["runs"].as_array().cloned().unwrap_or_default() {
                r["host"] = json!("local");
                all.push(r);
            }
        }
        Err(e) => errors.push(format!("local: {}", e)),
    }
    let enabled: Vec<hosts::Host> = reg.hosts.iter().filter(|h| h.enabled).cloned().collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = enabled
            .iter()
            .map(|h| {
                s.spawn(move || -> Result<Vec<Value>, String> {
                    let (code, text) = hosts::dispatch(h, "GET", &format!("/v1/runs?limit={}", limit), None)?;
                    if !(200..300).contains(&code) {
                        return Err(format!("{}: HTTP {}: {}", h.name, code, text));
                    }
                    let v: Value =
                        serde_json::from_str(&text).map_err(|e| format!("{}: bad response: {}", h.name, e))?;
                    Ok(v["runs"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|mut r| {
                            r["host"] = json!(h.name);
                            r
                        })
                        .collect())
                })
            })
            .collect();
        for hd in handles {
            match hd.join() {
                Ok(Ok(runs)) => all.extend(runs),
                Ok(Err(e)) => errors.push(e),
                Err(_) => errors.push("worker panicked".into()),
            }
        }
    });
    all.sort_by(|a, b| {
        b["created_at"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["created_at"].as_i64().unwrap_or(0))
    });
    print_runs_table(&all);
    if !errors.is_empty() {
        eprintln!("errors:");
        for e in errors {
            eprintln!("  {}", e);
        }
    }
}

fn cmd_status_remote(h: &hosts::Host, id: &str) {
    match dispatch_json(h, "GET", &format!("/v1/runs/{}", id)) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v["run"]).unwrap_or_default()),
        Err(e) => eprintln!("status failed: {}", e),
    }
}

fn cmd_logs_remote(h: &hosts::Host, id: &str, tail: &str) {
    match dispatch_json(h, "GET", &format!("/v1/runs/{}/log?tail={}", id, tail)) {
        Ok(v) => println!("{}", v["log"].as_str().unwrap_or("")),
        Err(e) => eprintln!("logs failed: {}", e),
    }
}

fn cmd_stop_remote(h: &hosts::Host, id: &str) {
    match hosts::dispatch(h, "POST", &format!("/v1/runs/{}/stop", id), Some("{}")) {
        Ok((code, text)) => {
            if (200..300).contains(&code) {
                println!("{}", text);
            } else {
                eprintln!("stop failed: HTTP {}: {}", code, text);
            }
        }
        Err(e) => eprintln!("stop failed: {}", e),
    }
}

fn cmd_host(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("help") {
        "add" => cmd_host_add(flags, &pos[1..]),
        "ls" | "list" => cmd_host_ls(flags),
        "rm" | "remove" => cmd_host_rm(flags, &pos[1..]),
        "enable" => cmd_host_enable(flags, &pos[1..], true),
        "disable" => cmd_host_enable(flags, &pos[1..], false),
        "probe" => cmd_host_probe(flags, &pos[1..]),
        _ => eprintln!("usage: roamway host add|ls|rm|enable|disable|probe ..."),
    }
}

fn cmd_host_add(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) => n,
        None => return eprintln!("usage: roamway host add <name> --endpoint <ep> [--token <t>]"),
    };
    let endpoint = match flags.get("endpoint") {
        Some(e) => e,
        None => return eprintln!("--endpoint is required"),
    };
    let env_name = format!("ROAMWAY_TOKEN_{}", name.to_uppercase().replace('-', "_"));
    let token = flags
        .get("token")
        .cloned()
        .or_else(|| std::env::var(&env_name).ok())
        .filter(|s| !s.is_empty());
    let token = match token {
        Some(t) => t,
        None => return eprintln!("token required: --token or environment {}", env_name),
    };
    let mut reg = registry_for(flags);
    if reg.get(name).is_some() {
        if flags.contains_key("force") {
            reg.remove(name);
        } else {
            eprintln!("host '{}' already exists (use --force)", name);
            return;
        }
    }
    match reg.add(name, endpoint, &token) {
        Ok(h) => {
            let entry = host_json(h);
            let _ = reg.save();
            println!("{}", serde_json::to_string_pretty(&entry).unwrap_or_default());
        }
        Err(e) => eprintln!("host add failed: {}", e),
    }
}

fn cmd_host_ls(flags: &HashMap<String, String>) {
    let mut reg = registry_for(flags);
    if flags.contains_key("probe") {
        for h in reg.hosts.iter_mut() {
            if !h.enabled {
                continue;
            }
            let p = hosts::probe(h);
            if p.error != "skipped" {
                if p.ok {
                    h.failures = 0;
                    h.next_retry_at = None;
                } else {
                    h.failures += 1;
                    h.next_retry_at = Some(now() + hosts::backoff(h.failures));
                }
            }
            h.last_probe = Some(p);
        }
        let _ = reg.save();
    }
    if flags.contains_key("json") {
        let hosts: Vec<Value> = reg.hosts.iter().map(host_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"hosts": hosts})).unwrap_or_default()
        );
        return;
    }
    println!(
        "{:<16} {:<26} {:<10} {:<10} {:<8} {:<6}",
        "NAME", "ENDPOINT", "STATE", "VERSION", "LATENCY", "RETRY"
    );
    for h in &reg.hosts {
        let version = h
            .last_probe
            .as_ref()
            .map(|p| p.version.as_str())
            .unwrap_or("-");
        let latency = h
            .last_probe
            .as_ref()
            .map(|p| format!("{}ms", p.latency_ms))
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<16} {:<26} {:<10} {:<10} {:<8} {:<6}",
            h.name,
            h.endpoint,
            host_state(h),
            version,
            latency,
            retry_secs(h)
        );
    }
}

fn cmd_host_rm(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) => n,
        None => return eprintln!("usage: roamway host rm <name>"),
    };
    let mut reg = registry_for(flags);
    if reg.remove(name) {
        let _ = reg.save();
        println!("removed {}", name);
    } else {
        eprintln!("no such host: {}", name);
    }
}

fn cmd_host_enable(flags: &HashMap<String, String>, pos: &[String], enable: bool) {
    let name = match pos.first() {
        Some(n) => n,
        None => return eprintln!("usage: roamway host enable|disable <name>"),
    };
    let mut reg = registry_for(flags);
    match reg.hosts.iter_mut().find(|h| h.name == name.as_str()) {
        Some(h) => {
            h.enabled = enable;
            let _ = reg.save();
            println!("{} {}", name, if enable { "enabled" } else { "disabled" });
        }
        None => eprintln!("no such host: {}", name),
    }
}

fn cmd_host_probe(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) => n,
        None => return eprintln!("usage: roamway host probe <name>"),
    };
    let mut reg = registry_for(flags);
    let idx = match reg.hosts.iter().position(|h| h.name == name.as_str()) {
        Some(i) => i,
        None => return eprintln!("no such host: {}", name),
    };
    let p = hosts::probe(&reg.hosts[idx]);
    if p.error != "skipped" {
        if p.ok {
            reg.hosts[idx].failures = 0;
            reg.hosts[idx].next_retry_at = None;
        } else {
            reg.hosts[idx].failures += 1;
            reg.hosts[idx].next_retry_at = Some(now() + hosts::backoff(reg.hosts[idx].failures));
        }
        reg.hosts[idx].last_probe = Some(p);
    }
    let _ = reg.save();
    println!("{}", serde_json::to_string_pretty(&host_json(&reg.hosts[idx])).unwrap_or_default());
}

fn cmd_agent(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("help") {
        "run" => cmd_agent_run(flags, &pos[1..]),
        "ls" | "list" => cmd_agent_ls(flags),
        "inspect" => cmd_agent_inspect(flags, &pos[1..]),
        "logs" => cmd_agent_logs(flags, &pos[1..]),
        "wait" => cmd_agent_wait(flags, &pos[1..]),
        "attach" => cmd_agent_attach(flags, &pos[1..]),
        "archive" => cmd_agent_archive(flags, &pos[1..]),
        "reload" => cmd_agent_reload(flags, &pos[1..]),
        "update" => cmd_agent_update(flags, &pos[1..]),
        "open" => cmd_agent_open(flags, &pos[1..]),
        "detach" => cmd_agent_detach(flags, &pos[1..]),
        "import" => cmd_agent_import(flags, &pos[1..]),
        _ => eprintln!(
            "usage: roamway agent run|ls|inspect|logs|wait|attach|archive|reload|update|open|detach|import ..."
        ),
    }
}

fn cmd_agent_run(flags: &HashMap<String, String>, pos: &[String]) {
    let mut prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        prompt = flags.get("prompt").cloned().unwrap_or_default();
    }
    if prompt.trim().is_empty() {
        eprintln!("prompt required");
        return;
    }
    let body = json!({
        "kind": flags.get("kind").cloned().unwrap_or_else(|| "shell".into()),
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
        "title": flags.get("title"),
        "prompt": prompt,
        "worktree": flags.contains_key("worktree"),
    });
    let v = match request(flags, "POST", "/v1/agents", Some(body)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("agent run failed: {}", e);
            return;
        }
    };
    if flags.contains_key("json") {
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return;
    }
    let agent_id = v["agent"]["id"].as_str().unwrap_or("").to_string();
    let run_id = v["run"]["id"].as_str().unwrap_or("").to_string();
    if flags.contains_key("no-wait") {
        println!("agent: {}\nrun:   {}", agent_id, run_id);
        return;
    }
    let started = std::time::Instant::now();
    loop {
        thread::sleep(Duration::from_secs(2));
        match request(flags, "GET", &format!("/v1/agents/{}", agent_id), None) {
            Ok(v) => {
                let st = v["agent"]["state"].as_str().unwrap_or("?");
                if st != "running" {
                    println!(
                        "agent: {}\nrun:   {}\nstate: {}\n{}s",
                        agent_id,
                        run_id,
                        st,
                        started.elapsed().as_secs()
                    );
                    return;
                }
                if flags.contains_key("quiet") {
                    continue;
                }
                eprintln!("[{}] {} {}s", agent_id, st, started.elapsed().as_secs());
            }
            Err(e) => {
                eprintln!("poll failed: {}", e);
                return;
            }
        }
    }
}

fn cmd_agent_ls(flags: &HashMap<String, String>) {
    let path = if flags.contains_key("all") {
        "/v1/agents?all=1"
    } else {
        "/v1/agents"
    };
    match request(flags, "GET", path, None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return;
            }
            let empty = vec![];
            let agents = v["agents"].as_array().unwrap_or(&empty);
            println!(
                "{:<12} {:<10} {:<14} {:<16} {:<22} {}",
                "ID", "STATE", "KIND", "MODEL", "TITLE", "UPDATED"
            );
            for a in agents {
                println!(
                    "{:<12} {:<10} {:<14} {:<16} {:<22} {}",
                    a["id"].as_str().unwrap_or(""),
                    a["state"].as_str().unwrap_or(""),
                    a["kind"].as_str().unwrap_or(""),
                    trunc(a["model"].as_str().unwrap_or("-"), 16),
                    trunc(a["title"].as_str().unwrap_or(""), 22),
                    a["updated_at"]
                        .as_i64()
                        .map(iso)
                        .unwrap_or_else(|| "-".into())
                );
            }
        }
        Err(e) => eprintln!("agent ls failed: {}", e),
    }
}

fn cmd_agent_inspect(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway inspect <id>"),
    };
    match request(flags, "GET", &format!("/v1/agents/{}", id), None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return;
            }
            let a = &v["agent"];
            println!("id:         {}", a["id"].as_str().unwrap_or(""));
            println!("title:      {}", a["title"].as_str().unwrap_or(""));
            println!("kind:       {}", a["kind"].as_str().unwrap_or(""));
            println!("model:      {}", a["model"].as_str().unwrap_or("-"));
            println!("workspace:  {}", a["workspace"].as_str().unwrap_or(""));
            println!("session_id: {}", a["session_id"].as_str().unwrap_or(""));
            println!("state:      {}", a["state"].as_str().unwrap_or(""));
            println!("parent:     {}", a["parent"].as_str().unwrap_or(""));
            println!("history:    {} msgs", a["history"].as_array().map(|h| h.len()).unwrap_or(0));
            println!("created_at: {}", a["created_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()));
            println!("updated_at: {}", a["updated_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()));
            println!("last_run:   {}", a["last_run_id"].as_str().unwrap_or(""));
            println!("last_error: {}", a["last_error"].as_str().unwrap_or(""));
            let empty = vec![];
            let runs = v["runs"].as_array().unwrap_or(&empty);
            println!("runs (recent 5):");
            for r in runs.iter().take(5) {
                println!(
                    "  {:<12} {:<12} {} -> {}",
                    r["id"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["created_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                    r["ended_at"].as_i64().map(iso).unwrap_or_else(|| "-".into())
                );
            }
        }
        Err(e) => eprintln!("agent inspect failed: {}", e),
    }
}

fn cmd_agent_logs(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway agent logs <id> [--tail 80]"),
    };
    let tail = flags.get("tail").cloned().unwrap_or_else(|| "80".into());
    let detail = match request(flags, "GET", &format!("/v1/agents/{}", id), None) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("agent logs failed: {}", e);
            return;
        }
    };
    let empty = vec![];
    let runs = detail["runs"].as_array().unwrap_or(&empty);
    for r in runs {
        let rid = r["id"].as_str().unwrap_or("");
        let status = r["status"].as_str().unwrap_or("");
        println!("--- {} {}", rid, status);
        match request(flags, "GET", &format!("/v1/runs/{}/log?tail={}", rid, tail), None) {
            Ok(l) => println!("{}", l["log"].as_str().unwrap_or("")),
            Err(e) => eprintln!("  log fetch failed: {}", e),
        }
    }
}

fn cmd_agent_wait(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway wait <id> [--timeout 60]"),
    };
    let timeout = flags.get("timeout").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let started = std::time::Instant::now();
    loop {
        if timeout > 0 && started.elapsed().as_secs() > timeout {
            eprintln!("wait timed out after {}s", timeout);
            std::process::exit(1);
        }
        thread::sleep(Duration::from_secs(2));
        match request(flags, "GET", &format!("/v1/agents/{}", id), None) {
            Ok(v) => {
                let st = v["agent"]["state"].as_str().unwrap_or("?");
                if st != "running" {
                    println!("state={}", st);
                    return;
                }
            }
            Err(e) => {
                eprintln!("wait failed: {}", e);
                std::process::exit(1);
            }
        }
    }
}

fn cmd_agent_archive(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway archive <id>"),
    };
    let path = if flags.contains_key("keep-worktree") {
        format!("/v1/agents/{}?keep-worktree=1", id)
    } else {
        format!("/v1/agents/{}", id)
    };
    match request(flags, "DELETE", &path, None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                println!("archived {}", id);
            }
        }
        Err(e) => eprintln!("agent archive failed: {}", e),
    }
}

fn cmd_agent_reload(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway agent reload <id>"),
    };
    match request(flags, "POST", &format!("/v1/agents/{}/reload", id), Some(json!({}))) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                println!("run: {}", v["run"]["id"].as_str().unwrap_or(""));
            }
        }
        Err(e) => eprintln!("agent reload failed: {}", e),
    }
}

fn cmd_agent_update(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway agent update <id> [--title --model --workspace]"),
    };
    let body = json!({
        "title": flags.get("title"),
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
    });
    match request(flags, "PATCH", &format!("/v1/agents/{}", id), Some(body)) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                println!("updated {}", id);
            }
        }
        Err(e) => eprintln!("agent update failed: {}", e),
    }
}

fn cmd_agent_open(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway agent open <id>"),
    };
    let base = if let Some(h) = resolve_host(flags) {
        h.endpoint.clone()
    } else {
        local_base(flags)
    };
    println!("{}/agents/{}", base.trim_end_matches('/'), id);
}

fn cmd_agent_detach(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway agent detach <id>"),
    };
    match request(flags, "POST", &format!("/v1/agents/{}/detach", id), Some(json!({}))) {
        Ok(v) => {
            if v["detached"].as_bool() == Some(false) {
                println!("already independent");
            } else if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                println!("detached {}", id);
            }
        }
        Err(e) => eprintln!("agent detach failed: {}", e),
    }
}

fn cmd_agent_import(flags: &HashMap<String, String>, pos: &[String]) {
    let mut session = flags.get("session").cloned().unwrap_or_default();
    if session.trim().is_empty() {
        session = pos.first().cloned().unwrap_or_default();
    }
    if session.trim().is_empty() {
        eprintln!("--session is required");
        return;
    }
    let body = json!({
        "kind": flags.get("kind").cloned().unwrap_or_else(|| "codex".into()),
        "session": session,
        "title": flags.get("title"),
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
    });
    match request(flags, "POST", "/v1/agents/import", Some(body)) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                println!("imported {}", v["agent"]["id"].as_str().unwrap_or(""));
            }
        }
        Err(e) => eprintln!("import failed: {}", e),
    }
}

fn cmd_agent_attach(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return eprintln!("usage: roamway attach <id>"),
    };
    let (base, token) = if let Some(h) = resolve_host(flags) {
        (
            h.endpoint.clone(),
            if h.token.is_empty() {
                None
            } else {
                Some(h.token.clone())
            },
        )
    } else {
        let c = local_client(flags);
        (c.base.clone(), c.token.clone())
    };
    let mut url = format!("{}/v1/agents/{}/events", base.trim_end_matches('/'), id);
    if let Some(t) = &token {
        url.push_str("?token=");
        url.push_str(t);
    }
    let req = ureq::get(&url).timeout(Duration::from_secs(3600));
    match req.call() {
        Ok(resp) => {
            let reader = resp.into_reader();
            let mut r = std::io::BufReader::new(reader);
            let mut line = Vec::new();
            let mut last_state = String::new();
            loop {
                line.clear();
                let n = match r.read_until(b'\n', &mut line) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if n == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&line)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                if let Some(payload) = text.strip_prefix("data:") {
                    if let Ok(v) = serde_json::from_str::<Value>(payload.trim()) {
                        match v["type"].as_str() {
                            Some("log") => {
                                print!("{}", v["chunk"].as_str().unwrap_or(""));
                                let _ = std::io::stdout().flush();
                            }
                            Some("state") => {
                                last_state = v["state"].as_str().unwrap_or("").to_string();
                            }
                            _ => {}
                        }
                    }
                }
            }
            println!("state={}", last_state);
        }
        Err(ureq::Error::Status(code, resp)) => {
            let t = resp.into_string().unwrap_or_default();
            eprintln!("attach failed: HTTP {}: {}", code, t);
        }
        Err(e) => eprintln!("attach failed: {}", e),
    }
}

fn cmd_daemon(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("help") {
        "status" => cmd_daemon_status(flags),
        "config" => cmd_daemon_config(flags),
        "start" => cmd_daemon_start(flags),
        "stop" => cmd_daemon_stop(flags),
        "restart" => cmd_daemon_restart(flags),
        "reload" => cmd_daemon_reload(flags),
        "pair" => cmd_daemon_pair(flags),
        "set-password" => cmd_daemon_set_password(flags),
        _ => eprintln!("usage: roamway daemon status|config|start|stop|restart|reload|pair|set-password"),
    }
}

fn cmd_daemon_status(flags: &HashMap<String, String>) {
    if host_flag(flags).is_some() {
        match request(flags, "GET", "/health", None) {
            Ok(v) => {
                if flags.contains_key("json") {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                } else {
                    println!(
                        "remote {} | version {} | uptime {}s",
                        host_flag(flags).unwrap_or_default(),
                        v["version"].as_str().unwrap_or("-"),
                        v["uptime_s"].as_i64().unwrap_or(0)
                    );
                }
            }
            Err(e) => eprintln!("daemon status failed: {}", e),
        }
        return;
    }
    let data = data_dir_for(flags);
    let mut status = json!({
        "ok": false,
        "error": "not running",
        "version": "",
        "uptime_s": 0,
        "pid": Value::Null,
        "data_dir": data.to_string_lossy(),
    });
    if let Some((pid, bind)) = read_pid(flags) {
        status["pid"] = json!(pid);
        let c = Client {
            base: hosts::endpoint_normalize(&bind),
            token: local_token(flags),
        };
        match c.req("GET", "/health", None) {
            Ok(v) => {
                status["ok"] = json!(true);
                status["version"] = v["version"].clone();
                status["uptime_s"] = v["uptime_s"].clone();
                status["running"] = v["running"].clone();
                status["queued"] = v["queued"].clone();
                status["error"] = Value::Null;
            }
            Err(e) => status["error"] = json!(e),
        }
    }
    if flags.contains_key("json") {
        println!("{}", serde_json::to_string_pretty(&status).unwrap_or_default());
        return;
    }
    if status["pid"].is_null() {
        println!("daemon offline | data {}", data.display());
        return;
    }
    println!(
        "daemon {} | version {} | pid {} | uptime {}s | data {}",
        if status["ok"].as_bool().unwrap_or(false) { "online" } else { "offline" },
        status["version"].as_str().unwrap_or("-"),
        status["pid"].as_u64().map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
        status["uptime_s"].as_i64().unwrap_or(0),
        status["data_dir"].as_str().unwrap_or("-")
    );
}

fn cmd_daemon_config(flags: &HashMap<String, String>) {
    let data = data_dir_for(flags);
    let max_concurrent = flags
        .get("max-concurrent")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(4);
    let out = json!({
        "data_dir": data.to_string_lossy(),
        "hosts_file": data.join("hosts.json").to_string_lossy(),
        "max_concurrent": max_concurrent,
        "version": env!("CARGO_PKG_VERSION"),
    });
    if flags.contains_key("json") {
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        println!("data_dir:  {}", data.display());
        println!("hosts:     {}", data.join("hosts.json").display());
        println!("concurrency: {}", max_concurrent);
        println!("version:   {}", env!("CARGO_PKG_VERSION"));
    }
}

fn cmd_daemon_start(flags: &HashMap<String, String>) {
    let bind = flags
        .get("bind")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:8796".into());
    let data = data_dir_for(flags);
    let _ = std::fs::create_dir_all(&data);
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("current_exe failed: {}", e);
            return;
        }
    };
    let token = flags
        .get("token")
        .cloned()
        .or_else(|| std::env::var("ROAMWAY_TOKEN").ok())
        .filter(|s| !s.is_empty());
    let mut cmd = Command::new(&exe);
    cmd.arg("serve")
        .arg("--bind")
        .arg(&bind)
        .arg("--data")
        .arg(&data)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(t) = token {
        cmd.env("ROAMWAY_TOKEN", t);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id();
            let _ = std::fs::write(pid_path(flags), format!("{}\n{}\n", pid, bind));
            println!("daemon started pid={} bind={}", pid, bind);
        }
        Err(e) => eprintln!("daemon start failed: {}", e),
    }
}

fn cmd_daemon_stop(flags: &HashMap<String, String>) {
    let (pid, bind) = match read_pid(flags) {
        Some(p) => p,
        None => {
            eprintln!("daemon not running (no pid file)");
            return;
        }
    };
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .arg("/T")
            .arg("/F")
            .arg("/PID")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let mut released = false;
    for _ in 0..50 {
        if !port_open(&bind) {
            released = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if released {
        let _ = std::fs::remove_file(pid_path(flags));
        println!("daemon stopped (pid {} bind {})", pid, bind);
    } else {
        eprintln!("daemon pid {} still listening on {}", pid, bind);
    }
}

fn cmd_daemon_restart(flags: &HashMap<String, String>) {
    cmd_daemon_stop(flags);
    cmd_daemon_start(flags);
}

fn cmd_daemon_reload(flags: &HashMap<String, String>) {
    let reg = registry_for(flags);
    println!(
        "reloaded {} hosts from {}",
        reg.hosts.len(),
        data_dir_for(flags).join("hosts.json").display()
    );
}

fn cmd_daemon_pair(flags: &HashMap<String, String>) {
    let token = flags
        .get("token")
        .cloned()
        .or_else(|| std::env::var("ROAMWAY_TOKEN").ok())
        .or_else(|| {
            std::fs::read_to_string(data_dir_for(flags).join("token"))
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty());
    let token = match token {
        Some(t) => t,
        None => return eprintln!("token required: --token, ROAMWAY_TOKEN, or <data>/token"),
    };
    let bind = flags
        .get("bind")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:8796".into());
    let port = bind_parts(&bind).map(|(_, p)| p).unwrap_or(8796);
    println!("http://{}:{}?token={}", pair_ip(&bind), port, token);
}

fn cmd_daemon_set_password(flags: &HashMap<String, String>) {
    let data = data_dir_for(flags);
    let _ = std::fs::create_dir_all(&data);
    let password = if flags.contains_key("stdin") {
        let mut line = String::new();
        if let Err(e) = std::io::stdin().read_line(&mut line) {
            eprintln!("read password failed: {}", e);
            return;
        }
        line.trim_end_matches(['\r', '\n']).to_string()
    } else {
        print!("new password: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if let Err(e) = std::io::stdin().read_line(&mut line) {
            eprintln!("read password failed: {}", e);
            return;
        }
        line.trim_end_matches(['\r', '\n']).to_string()
    };
    let mut store = auth::PasswordStore::load(&data);
    match store.set_password(&password) {
        Ok(()) => println!("password set for {} (pbkdf2-sha256)", data.display()),
        Err(e) => eprintln!("set-password failed: {}", e),
    }
}

fn cmd_token(flags: &HashMap<String, String>, pos: &[String]) {
    let sub = pos.first().map(String::as_str).unwrap_or("help");
    let data = data_dir_for(flags);
    let _ = std::fs::create_dir_all(&data);
    match sub {
        "add" | "create" => cmd_token_create(flags, &data),
        "ls" | "list" => cmd_token_ls(flags, &data),
        "revoke" => {
            let id = pos.get(1).cloned().unwrap_or_default();
            if id.is_empty() {
                eprintln!("usage: roamway token revoke <id>");
                return;
            }
            let mut store = auth::TokenStore::load(&data);
            if store.revoke(&id) {
                println!("revoked {}", id);
            } else {
                eprintln!("no such token: {}", id);
            }
        }
        _ => eprintln!("usage: roamway token add|create --name N [--scope read|run|admin] [--ttl 24h] | ls|list | revoke <id>"),
    }
}

fn cmd_token_create(flags: &HashMap<String, String>, data: &std::path::Path) {
    let name_opt = flags.get("name").cloned().filter(|s| !s.is_empty());
    let mut scopes = flag_many(flags, "scope");
    if scopes.is_empty() {
        scopes = vec!["read".to_string(), "run".to_string()];
    }
    for s in &scopes {
        if !auth::ALL_SCOPES.contains(&s.as_str()) {
            eprintln!("invalid --scope '{}' (read|run|admin)", s);
            return;
        }
    }
    let ttl = flags.get("ttl").and_then(|v| auth::parse_ttl(v));
    let mut store = auth::TokenStore::load(data);
    let (id, plain) = store.create(name_opt.clone(), scopes.clone(), ttl);
    let name = name_opt.unwrap_or_else(|| format!("token-{}", &id));
    if flags.contains_key("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": id,
                "name": name,
                "scopes": scopes,
                "token": plain,
                "expires_at": ttl.map(|s| iso(now() + s)),
            }))
            .unwrap_or_default()
        );
    } else {
        println!("id:      {}", id);
        println!("name:    {}", name);
        println!("scopes:  {}", scopes.join(","));
        match ttl {
            Some(s) => println!("expires: {} ({}s)", iso(now() + s), s),
            None => println!("expires: never"),
        }
        println!("token:   {}   # shown once, store it safely", plain);
    }
}

fn cmd_token_ls(flags: &HashMap<String, String>, data: &std::path::Path) {
    let store = auth::TokenStore::load(data);
    let tokens = store.list();
    if flags.contains_key("json") {
        println!("{}", serde_json::to_string_pretty(&tokens).unwrap_or_default());
        return;
    }
    if tokens.is_empty() {
        println!("no tokens");
        return;
    }
    println!("{:<34} {:<20} {:<18} {:<21} sha256", "id", "name", "scopes", "expires");
    for t in &tokens {
        let name: String = t.name.chars().take(20).collect();
        let exp = match t.expires_at {
            Some(e) => iso(e),
            None => "never".to_string(),
        };
        println!(
            "{:<34} {:<20} {:<18} {:<21} {}",
            t.id,
            name,
            t.scopes.join(","),
            exp,
            mask_sha256(&t.sha256),
        );
    }
}

fn mask_sha256(sha: &str) -> String {
    if sha.len() <= 12 {
        return "*masked*".to_string();
    }
    format!("{}...{}", &sha[..8], &sha[sha.len() - 4..])
}

fn cmd_pair(flags: &HashMap<String, String>) {
    let data = data_dir_for(flags);
    let _ = std::fs::create_dir_all(&data);
    let mut store = auth::PairingStore::load(&data);
    let code = store.generate(auth::PAIR_TTL_SECS);
    let host = flags
        .get("host")
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8796".to_string());
    let endpoint = format!("{}/v1/auth/pair", host.trim_end_matches('/'));
    let exchange = format!(
        "curl -X POST {} -H 'content-type: application/json' -d '{{\"code\":\"{}\"}}'",
        endpoint,
        code
    );
    if flags.contains_key("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "code": code,
                "ttl_secs": auth::PAIR_TTL_SECS,
                "exchange": exchange,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("pair code: {}  (valid {}s)", code, auth::PAIR_TTL_SECS);
        println!("exchange : {}", exchange);
    }
}

fn cmd_approve(flags: &HashMap<String, String>, pos: &[String]) {
    let id = pos.first().map(String::as_str).unwrap_or("");
    if id.is_empty() {
        eprintln!("usage: roamway approve <id>");
        return;
    }
    match request(flags, "POST", &format!("/v1/approvals/{}/approve", id), None) {
        Ok(v) if flags.contains_key("json") => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Ok(v) => println!("{} {}", id, v["status"].as_str().unwrap_or("approved")),
        Err(e) => eprintln!("approve failed: {}", e),
    }
}

fn cmd_deny(flags: &HashMap<String, String>, pos: &[String]) {
    let id = pos.first().map(String::as_str).unwrap_or("");
    if id.is_empty() {
        eprintln!("usage: roamway deny <id>");
        return;
    }
    match request(flags, "POST", &format!("/v1/approvals/{}/deny", id), None) {
        Ok(v) if flags.contains_key("json") => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Ok(v) => println!("{} {}", id, v["status"].as_str().unwrap_or("denied")),
        Err(e) => eprintln!("deny failed: {}", e),
    }
}

fn cmd_approvals(flags: &HashMap<String, String>) {
    let path = if flags.contains_key("all") { "/v1/approvals?all=1" } else { "/v1/approvals" };
    match request(flags, "GET", path, None) {
        Ok(v) => {
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return;
            }
            let arr = v.get("approvals").and_then(|a| a.as_array()).cloned().unwrap_or_default();
            if arr.is_empty() {
                println!("no approvals");
                return;
            }
            for a in &arr {
                println!(
                    "{:<34} {:<8} {:<6} {} {}",
                    a["id"].as_str().unwrap_or("-"),
                    a["status"].as_str().unwrap_or("-"),
                    a["method"].as_str().unwrap_or("-"),
                    a["path"].as_str().unwrap_or("-"),
                    a["ts"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                );
            }
        }
        Err(e) => eprintln!("approvals failed: {}", e),
    }
}
fn cmd_onboard(flags: &HashMap<String, String>) {
    let data = data_dir_for(flags);
    let _ = std::fs::create_dir_all(&data);
    let token = gen_token();
    let token_file = data.join("token");
    if let Err(e) = std::fs::write(&token_file, format!("{}\n", token)) {
        eprintln!("write token failed: {}", e);
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600));
    }
    let bind = flags
        .get("bind")
        .cloned()
        .unwrap_or_else(|| "0.0.0.0:8796".into());
    let serve = format!("roamway serve --bind {} --data {}", bind, data.display());
    let log = data.join("roamway.log");
    println!("token:    {}", token_file.display());
    println!("foreground:  ROAMWAY_TOKEN=$(cat {}) {}", token_file.display(), serve);
    println!("background:  nohup env ROAMWAY_TOKEN=$(cat {}) {} >>{} 2>&1 &", token_file.display(), serve, log.display());
    println!("crontab:     @reboot /usr/bin/env ROAMWAY_TOKEN=$(cat {}) {} >>{} 2>&1", token_file.display(), serve, log.display());
    let port = bind_parts(&bind).map(|(_, p)| p).unwrap_or(8796);
    println!(
        "other machine: roamway host add nas --endpoint http://{}:{} --token $ROAMWAY_TOKEN",
        pair_ip(&bind),
        port
    );
}

fn cmd_restart(flags: &HashMap<String, String>) {
    if host_flag(flags).is_some() {
        eprintln!("restart --host is not supported over HTTP; run on the target host");
        return;
    }
    cmd_daemon_restart(flags);
}

fn cmd_reload(flags: &HashMap<String, String>) {
    if host_flag(flags).is_some() {
        eprintln!("reload --host is not supported over HTTP; run on the target host");
        return;
    }
    cmd_daemon_reload(flags);
}

fn cmd_provider(flags: &HashMap<String, String>, pos: &[String]) {
    let dir = data_dir_for(flags);
    let providers = providers::load(&dir);
    match pos.first().map(String::as_str).unwrap_or("ls") {
        "ls" | "list" | "" => {
            println!(
                "{:<14} {:<6} {:<10} {:<7} {:<9} {}",
                "ID", "KIND", "STATUS", "MODELS", "KEY", "ENDPOINT"
            );
            for p in &providers {
                let key = if p.kind == "api" {
                    if p.env_key.is_empty() {
                        "-".to_string()
                    } else {
                        match std::env::var(&p.env_key) {
                            Ok(v) if !v.trim().is_empty() => "[set]".to_string(),
                            _ => "[missing]".to_string(),
                        }
                    }
                } else {
                    "-".to_string()
                };
                let endpoint = if p.base_url.is_empty() { "-".to_string() } else { p.base_url.clone() };
                println!(
                    "{:<14} {:<6} {:<10} {:<7} {:<9} {}",
                    p.id,
                    p.kind,
                    providers::state(p),
                    p.models.len(),
                    key,
                    endpoint
                );
            }
        }
        "models" => cmd_provider_models(flags, &dir, &providers, &pos[1..]),
        "diagnostic" => {
            let id = match pos.get(1) {
                Some(id) => id,
                None => {
                    eprintln!("usage: roamway provider diagnostic <id>");
                    return;
                }
            };
            match providers::get(&providers, id) {
                Some(p) => {
                    for line in providers::diagnostic_lines(&dir, p) {
                        println!("{}", line);
                    }
                }
                None => eprintln!("provider '{}' not found", id),
            }
        }
        _ => eprintln!("usage: roamway provider [ls|models [<id>] [--refresh]|diagnostic <id>]"),
    }
}

fn cmd_provider_models(
    flags: &HashMap<String, String>,
    dir: &std::path::Path,
    providers: &[providers::Provider],
    pos: &[String],
) {
    let want = pos.first().map(String::as_str);
    for p in providers {
        if let Some(id) = want {
            if p.id != id {
                continue;
            }
        }
        let mut models = p.models.clone();
        if flags.contains_key("refresh") && p.kind == "api" && !p.base_url.is_empty() {
            match providers::fetch_models(&p.base_url) {
                Ok(list) => models = list,
                Err(e) => eprintln!("roamway: refresh {} models failed: {} (using builtins)", p.id, e),
            }
        }
        let rendered = if models.is_empty() {
            "-".to_string()
        } else {
            models.join(", ")
        };
        println!("{}: {}", p.id, rendered);
    }
    if let Some(id) = want {
        if !providers.iter().any(|p| p.id == id) {
            eprintln!("provider '{}' not found", id);
        }
    }
    let _ = dir;
}

fn cmd_usage(flags: &HashMap<String, String>, pos: &[String]) {
    let dir = data_dir_for(flags);
    let state = usage::read_state(&dir);
    match pos.first().map(String::as_str).unwrap_or("") {
        "ls" | "list" => {
            let since = match usage::since_ts(flags.get("since").map(String::as_str)) {
                Some(s) => s,
                None => {
                    eprintln!("usage: invalid --since (expected 7d, 24h or ISO timestamp)");
                    return;
                }
            };
            let provider = flags.get("provider").map(String::as_str);
            let rows = usage::aggregate(&state, since, provider);
            println!(
                "{:<12} {:<14} {:<20} {:<6} {:<8} {:<8} {}",
                "DAY", "PROVIDER", "MODEL", "RUNS", "IN", "OUT", "TOTAL"
            );
            for r in rows {
                println!(
                    "{:<12} {:<14} {:<20} {:<6} {:<8} {:<8} {}",
                    r.day, r.provider, r.model, r.runs, r.in_tokens, r.out_tokens, r.total_tokens
                );
            }
        }
        "inspect" => {
            let id = match pos.get(1) {
                Some(id) => id,
                None => {
                    eprintln!("usage: roamway usage inspect <run_id>");
                    return;
                }
            };
            match usage::inspect_lines(&state, id) {
                Some(lines) => {
                    for line in lines {
                        println!("{}", line);
                    }
                }
                None => eprintln!("no such run: {}", id),
            }
        }
        "" => {
            let report = usage::report_json(&state);
            if flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                return;
            }
            let ov = &report["overview"];
            println!(
                "overview: runs={} prompt_tokens={} completion_tokens={} total_tokens={}",
                ov["runs"], ov["prompt_tokens"], ov["completion_tokens"], ov["total_tokens"]
            );
            println!("\nby provider:");
            let empty: Vec<Value> = Vec::new();
            for item in report["by_provider"].as_array().unwrap_or(&empty) {
                println!(
                    "  {:<20} in={:<10} out={:<10} total={}",
                    item["provider"].as_str().unwrap_or("-"),
                    item["prompt_tokens"],
                    item["completion_tokens"],
                    item["total_tokens"]
                );
            }
            println!("\nby model:");
            for item in report["by_model"].as_array().unwrap_or(&empty) {
                println!(
                    "  {:<20} in={:<10} out={:<10} total={}",
                    item["model"].as_str().unwrap_or("-"),
                    item["prompt_tokens"],
                    item["completion_tokens"],
                    item["total_tokens"]
                );
            }
            println!("\ntop 20 most expensive runs:");
            for item in report["top_expensive"].as_array().unwrap_or(&empty) {
                println!(
                    "  {:<12} model={:<16} provider={:<14} total={}",
                    item["run_id"].as_str().unwrap_or("-"),
                    item["model"].as_str().unwrap_or("-"),
                    item["provider"].as_str().unwrap_or("-"),
                    item["total_tokens"]
                );
            }
        }
        _ => eprintln!("usage: roamway usage [ls [--since 7d|24h|ISO] [--provider id]|inspect <run_id>] [--json]"),
    }
}

fn cmd_plugin(flags: &HashMap<String, String>, pos: &[String]) {
    let dir = data_dir_for(flags);
    match pos.first().map(String::as_str).unwrap_or("ls") {
        "ls" | "list" | "" => {
            let reg = plugins::Registry::load(&dir);
            println!(
                "{:<16} {:<10} {:<8} {:<8} {:<8} {}",
                "NAME", "VERSION", "ENABLED", "TRUSTED", "TIMEOUT", "ENTRY"
            );
            for p in &reg.plugins {
                println!(
                    "{:<16} {:<10} {:<8} {:<8} {:<8} {}",
                    p.name,
                    p.version,
                    p.enabled,
                    p.trusted,
                    p.timeout_s,
                    p.entry.join(" ")
                );
            }
        }
        "init" => {
            let name = match pos.get(1) {
                Some(n) => n,
                None => {
                    eprintln!("usage: roamway plugin init <name>");
                    return;
                }
            };
            match plugins::init(&dir, name) {
                Ok(()) => println!("plugin '{}' initialized (not enabled)", name),
                Err(e) => eprintln!("plugin init failed: {}", e),
            }
        }
        "logs" => {
            let name = match pos.get(1) {
                Some(n) => n,
                None => {
                    eprintln!("usage: roamway plugin logs <name>");
                    return;
                }
            };
            let reg = plugins::Registry::load(&dir);
            if reg.get(name).is_none() {
                eprintln!("plugin '{}' not registered", name);
            }
            print!("{}", plugins::read_log(&dir, name));
        }
        "reload" => {
            let name = match pos.get(1) {
                Some(n) => n,
                None => {
                    eprintln!("usage: roamway plugin reload <name>");
                    return;
                }
            };
            match plugins::reload(&dir, name) {
                Ok(()) => println!("plugin '{}' reloaded", name),
                Err(e) => eprintln!("plugin reload failed: {}", e),
            }
        }
        "enable" | "disable" => {
            let name = match pos.get(1) {
                Some(n) => n,
                None => {
                    eprintln!("usage: roamway plugin enable|disable <name> [--trust]");
                    return;
                }
            };
            let enable = pos.first().map(String::as_str) == Some("enable");
            let trust = flags.contains_key("trust");
            let result = if enable {
                plugins::enable(&dir, name, trust)
            } else {
                plugins::disable(&dir, name)
            };
            match result {
                Ok(()) => println!(
                    "plugin '{}' {}",
                    name,
                    if enable { "enabled" } else { "disabled" }
                ),
                Err(e) => eprintln!("plugin {} failed: {}", name, e),
            }
        }
        "remove" => {
            let name = match pos.get(1) {
                Some(n) => n,
                None => {
                    eprintln!("usage: roamway plugin remove <name> [--keep-files]");
                    return;
                }
            };
            match plugins::remove(&dir, name, flags.contains_key("keep-files")) {
                Ok(()) => println!("plugin '{}' removed", name),
                Err(e) => eprintln!("plugin remove failed: {}", e),
            }
        }
        _ => eprintln!("usage: roamway plugin ls|init|logs|reload|enable|disable|remove ..."),
    }
}

fn cmd_speech(flags: &HashMap<String, String>, pos: &[String]) {
    let dir = data_dir_for(flags);
    match pos.first().map(String::as_str).unwrap_or("") {
        "" => {
            for line in speech::status_lines(&dir) {
                println!("{}", line);
            }
        }
        "transcribe" => {
            let file = match pos.get(1) {
                Some(f) => f,
                None => {
                    eprintln!("usage: roamway speech transcribe <file>");
                    return;
                }
            };
            match speech::transcribe(&dir, file) {
                Ok(text) => print!("{}", text),
                Err(e) => {
                    eprintln!("{}", e);
                    eprintln!("example config: {}", speech::example_config());
                }
            }
        }
        "say" => {
            let text = pos[1..].join(" ");
            if text.trim().is_empty() {
                eprintln!("usage: roamway speech say <text> [--out file]");
                return;
            }
            match speech::say(&dir, &text, flags.get("out").map(String::as_str)) {
                Ok(path) => println!("{}", path),
                Err(e) => {
                    eprintln!("{}", e);
                    eprintln!("example config: {}", speech::example_config());
                }
            }
        }
        _ => eprintln!("usage: roamway speech [transcribe <file>|say <text> [--out file]]"),
    }
}

/// 测试用的最小 App：共享给 server.rs / terminals.rs 的单测，避免每处重复拼装结构体。
#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::path::Path;

    pub fn test_app(dir: &Path) -> Arc<App> {
        let _ = std::fs::create_dir_all(dir);
        let store = Store::load(dir);
        Arc::new(App {
            store: Mutex::new(store),
            schedules: Mutex::new(schedules::ScheduleStore::load(dir)),
            terms: Mutex::new(terminals::TerminalStore::load(dir)),
            term_rt: Mutex::new(HashMap::new()),
            token: None,
            started: now(),
            cancels: Mutex::new(HashMap::new()),
            tz_offset_hours: 8,
            max_concurrent: 4,
            max_conn: 64,
            conns: AtomicUsize::new(0),
            tokens: Mutex::new(auth::TokenStore::load(dir)),
            approvals: Mutex::new(auth::ApprovalStore::load(dir)),
            passwords: Mutex::new(auth::PasswordStore::load(dir)),
            pairings: Mutex::new(auth::PairingStore::load(dir)),
            hub_rt: Mutex::new(crate::hub::HubRuntime::default()),
            allow_hosts: vec![],
            allow_origins: vec![],
            require_approval: vec![],
            bind_host: "127.0.0.1".to_string(),
            unix_socket: None,
            tls_cert: None,
            tls_key: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_parse_pairs_and_bools() {
        let args = [
            "--bind".to_string(),
            "127.0.0.1:1".to_string(),
            "--no-wait".to_string(),
            "run".to_string(),
        ];
        let (flags, pos) = parse_flags(&args);
        assert_eq!(flags.get("bind").map(String::as_str), Some("127.0.0.1:1"));
        assert_eq!(flags.get("no-wait").map(String::as_str), Some("true"));
        assert_eq!(pos, vec!["run".to_string()]);
    }

    #[test]
    fn repeated_flags_are_collected() {
        let args = [
            "--scope".to_string(),
            "read".to_string(),
            "--scope".to_string(),
            "run".to_string(),
            "--allow-host".to_string(),
            "a.example".to_string(),
            "--allow-host".to_string(),
            "b.example".to_string(),
        ];
        let (flags, _) = parse_flags(&args);
        assert_eq!(flag_many(&flags, "scope"), vec!["read", "run"]);
        assert_eq!(flag_many(&flags, "allow-host"), vec!["a.example", "b.example"]);
    }

    #[test]
    fn local_base_prefers_bind_file() {
        let dir = std::env::temp_dir().join(format!("roamway-bind-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("bind"), "127.0.0.1:8896\n").unwrap();
        let mut flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        flags.insert("data".to_string(), dir.to_string_lossy().to_string());
        assert_eq!(local_base(&flags), "http://127.0.0.1:8896");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_token_three_level_fallback() {
        let dir = std::env::temp_dir().join(format!("roamway-token-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("token"), "file-token\n").unwrap();
        let mut flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        flags.insert("data".to_string(), dir.to_string_lossy().to_string());
        let prev = std::env::var("ROAMWAY_TOKEN").ok();

        flags.insert("token".to_string(), "flag-token".to_string());
        std::env::set_var("ROAMWAY_TOKEN", "env-token");
        assert_eq!(local_token(&flags).as_deref(), Some("flag-token"));

        flags.remove("token");
        assert_eq!(local_token(&flags).as_deref(), Some("env-token"));

        std::env::remove_var("ROAMWAY_TOKEN");
        assert_eq!(local_token(&flags).as_deref(), Some("file-token"));

        match prev {
            Some(v) => std::env::set_var("ROAMWAY_TOKEN", v),
            None => std::env::remove_var("ROAMWAY_TOKEN"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backoff_window_skip_detection() {
        let at = now();
        let mut h = hosts::Host {
            id: "h1".to_string(),
            name: "nas".to_string(),
            endpoint: "http://127.0.0.1:1".to_string(),
            token: String::new(),
            enabled: true,
            created_at: at,
            last_probe: None,
            failures: 1,
            next_retry_at: Some(at + 3600),
        };
        let p = hosts::probe(&h);
        assert_eq!(p.error, "skipped");
        assert!(!p.ok);
        h.last_probe = Some(p);
        assert_eq!(host_state(&h), "skipped");
        assert!(retry_secs(&h) > 0);
        assert!(retry_secs(&h) <= 3600);
    }

    #[test]
    fn loopback_bind_detection() {
        assert!(is_loopback_bind("127.0.0.1:8796"));
        assert!(is_loopback_bind("127.0.0.1"));
        assert!(is_loopback_bind("::1"));
        assert!(is_loopback_bind("[::1]:8796"));
        assert!(is_loopback_bind("::1:8796"));
        assert!(is_loopback_bind("localhost:8796"));
        assert!(!is_loopback_bind("0.0.0.0:8796"));
        assert!(!is_loopback_bind("192.168.1.50:8796"));
        assert!(!is_loopback_bind("::"));
        assert!(!is_loopback_bind("[::]:8796"));
    }
}
