mod jobs;
mod runner;
mod server;
mod types;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use types::*;

const BOOLS: &[&str] = &["no-wait", "quiet", "json", "allow-overlap", "disabled", "help"];

fn usage() {
    println!(
        r#"roamway {} — 迷你多智能体编排器（国内模型 / 长跑友好）

  roamway serve  [--bind 127.0.0.1:8796] [--data DIR] [--token T] [--tz 8] [--max-concurrent 4] [--max-conn 64]
  roamway run    --agent codex [--model M] [--workspace DIR] [--title T] [--timeout 1800] [--no-wait] "prompt"
  roamway ls     [--limit 20]
  roamway status ID
  roamway logs   ID [--tail 80]
  roamway stop   ID
  roamway rm     ID
  roamway jobs
  roamway jobs-add --agent shell --every 3600 | --daily 08:30 [--model M] [--workspace DIR] [--max-retries 2] "prompt"
  roamway jobs-run ID
  roamway jobs-rm  ID
  roamway health

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
                flags.insert(k.to_string(), "true".into());
                i += 1;
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                flags.insert(k.to_string(), args[i + 1].clone());
                i += 2;
            } else {
                flags.insert(k.to_string(), "true".into());
                i += 1;
            }
        } else {
            pos.push(a.clone());
            i += 1;
        }
    }
    (flags, pos)
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
                .unwrap_or_else(|| "http://127.0.0.1:8796".into()),
            token: flags
                .get("token")
                .cloned()
                .or_else(|| std::env::var("ROAMWAY_TOKEN").ok())
                .filter(|s| !s.is_empty()),
        }
    }

    fn req(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}{}", self.base.trim_end_matches('/'), path);
        let mut r = match method {
            "GET" => ureq::get(&url),
            "POST" => ureq::post(&url),
            "DELETE" => ureq::delete(&url),
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
        "run" => cmd_run(&flags, &pos),
        "ls" | "list" => cmd_ls(&flags),
        "status" => cmd_status(&flags, &pos),
        "logs" => cmd_logs(&flags, &pos),
        "stop" => cmd_stop(&flags, &pos),
        "rm" => cmd_rm(&flags, &pos),
        "jobs" => cmd_jobs(&flags),
        "jobs-add" => cmd_jobs_add(&flags, &pos),
        "jobs-run" => cmd_jobs_run(&flags, &pos),
        "jobs-rm" => cmd_jobs_rm(&flags, &pos),
        "health" => cmd_health(&flags),
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

    // 无 token 且绑定非 loopback 时拒绝启动，避免把任意命令执行暴露给整个网络
    if token.is_none() && !is_loopback_bind(&bind) {
        eprintln!(
            "roamway: refusing to serve without --token on non-loopback bind {}; use 127.0.0.1/::1 or set ROAMWAY_TOKEN",
            bind
        );
        std::process::exit(2);
    }

    let store = Store::load(&data);
    let loaded_ok = store.loaded_ok;
    let app = Arc::new(App {
        store: Mutex::new(store),
        token: token.clone(),
        started: now(),
        cancels: Mutex::new(HashMap::new()),
        tz_offset_hours: tz,
        max_concurrent,
        max_conn,
        conns: AtomicUsize::new(0),
    });
    if loaded_ok {
        app.save();
    }

    jobs::start_scheduler(app.clone());

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
        if token.is_some() { "token" } else { "off" },
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
    let prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        eprintln!("prompt required");
        return;
    }
    let c = Client::from_flags(flags);
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
    let c = Client::from_flags(flags);
    let limit = flags
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20);
    match c.req("GET", &format!("/v1/runs?limit={}", limit), None) {
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
    let c = Client::from_flags(flags);
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
    let c = Client::from_flags(flags);
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
    let c = Client::from_flags(flags);
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
    let c = Client::from_flags(flags);
    match c.req("DELETE", &format!("/v1/runs/{}", id), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("rm failed: {}", e),
    }
}

fn cmd_jobs(flags: &HashMap<String, String>) {
    let c = Client::from_flags(flags);
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
    let c = Client::from_flags(flags);
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
    let c = Client::from_flags(flags);
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
    let c = Client::from_flags(flags);
    match c.req("DELETE", &format!("/v1/jobs/{}", id), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("jobs-rm failed: {}", e),
    }
}

fn cmd_health(flags: &HashMap<String, String>) {
    let c = Client::from_flags(flags);
    match c.req("GET", "/health", None) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => eprintln!("health failed: {}", e),
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
