mod agents;
mod auth;
mod backup;
mod config;
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
mod web;

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use types::*;
use crate::workspaces::WorktreeSpec;

const BOOLS: &[&str] = &["print", "board", "no-wait", "quiet", "json", "allow-overlap", "disabled", "help", "force", "probe", "all", "worktree", "keep-worktree", "run-now", "overlap", "enabled", "enter", "no-enter", "stdin", "no-browser", "no-headers", "no-color", "runs", "background", "global", "resume", "continue", "version", "dry-run", "yes"];

static FAIL_CODE: AtomicI32 = AtomicI32::new(0);

fn fail(code: i32) {
    FAIL_CODE.fetch_max(code, Ordering::SeqCst);
}

fn usage_error(msg: &str) {
    eprintln!("{}", msg);
    fail(2);
}

fn usage() {
    println!(
        r#"roamway {} — 迷你多智能体编排器（国内模型 / 长跑友好）

核心：
  roamway serve  [--bind 127.0.0.1:8796] [--data DIR] [--token T] [--tz 8] [--max-concurrent 4] [--max-conn 64]
  roamway start                                  # = daemon start
  roamway run    --agent codex [--model M] [--workspace DIR] [--title T] [--timeout 1800] [--no-wait] [--wait-timeout N] [--new-workspace local|worktree] [--worktree-mode branch-off|checkout-branch|checkout-pr] [--worktree-slug SLUG] [--new-branch B] [--base REF] [--branch B] [--cwd DIR] [--env K=V] [--label k=v] [--image PATH] [-i PATH] [--host H] "prompt"
  roamway send   <agent-id> <prompt...>          # 续跑 agent
  roamway delete <id> [--yes]                    # 运行中先 stop 再硬删
  roamway ls     [--limit 50] [-a|--all] [-g|--global] [--label k=v] [--kind K] [--host H|all] [--json|--format table|json|yaml] [-q] [--no-headers] ; --runs 看运行记录
  roamway status [ID]
  roamway logs   ID [--tail 80]
  roamway stop   ID
  roamway rm     ID
  roamway jobs | jobs-add | jobs-run | jobs-rm
  roamway schedule create|ls|inspect|logs|pause|resume|delete|run-once|update
  roamway hooks
  roamway agent run|ls|inspect|logs|wait|attach|archive|reload|update|open|detach|import
  roamway attach|inspect|wait|archive|import <id>
  roamway hub [init|permissions|projects|deploy|logout|status|token rotate]
  roamway health
  roamway host add|ls|rm|enable|disable|probe
  roamway daemon status|config|start|stop|restart|reload|pair|set-password|run
  roamway token add|create --name N [--scope read|run|admin] [--ttl 24h] | ls|list | revoke <id>
  roamway approve <id> | deny <id> | approvals [--all]
  roamway onboard [--bind 0.0.0.0:8796]
  roamway restart | reload

运维扩展：
  roamway permit ls | approve <id> | deny <id>
  roamway heartbeat ls|create|update|delete           # 落在 schedule 存储
  roamway workspace ls | create <name> --path DIR | archive <name> | setup <name> | rm <name>
  roamway terminal create|ls|capture|send-keys|kill
  roamway script ls|add|rm|start|stop
  roamway prune [--dry-run] [--keep N] [--max-age-days N] [--yes]
  roamway backup [--out DIR] [--keep N]                 # 本地状态快照
  roamway restore --from DIR [--data DIR] [--force] [--yes]  # 从快照回滚（仅本地）
  注：受管终端无 TTY，TUI 程序（vim/top 交互界面）不支持

生态扩展：
  roamway provider [ls|models [<id>] [--refresh]|diagnostic <id>]
  roamway usage [ls [--since 7d|24h|ISO] [--provider id]|inspect <run_id>]
  roamway plugin [ls|init|logs|reload|enable|disable|remove]
  roamway speech [transcribe <file>|say <text> [--out file]]

不支持：
  roamway project ls|delete   # 无 project 模型，请用 workspace ls（exit 2）

通用参数：--host http://127.0.0.1:8796  --token T  --format table|json|yaml  --json  -q/--quiet
          --no-headers  --no-color  --home DIR（等价 --data DIR）
          未知短标志 exit 2；prompt 里以 - 开头的词请放在 -- 之后
环境变量：ROAMWAY_HOST / ROAMWAY_TOKEN / ROAMWAY_DEEPSEEK_KEY / ROAMWAY_QWEN_KEY
agent 可选：codex | hermes | claude | shell | api:deepseek | api:qwen"#,
        env!("CARGO_PKG_VERSION")
    );
}
struct ParsedArgs {
    flags: HashMap<String, String>,
    pos: Vec<String>,
    unknown: Vec<String>,
}

fn parse_args(args: &[String]) -> ParsedArgs {
    let mut flags = HashMap::new();
    let mut pos = Vec::new();
    let mut unknown = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            pos.extend(args[i + 1..].iter().cloned());
            break;
        }
        if a.starts_with('-') && !a.starts_with("--") && a.len() >= 2 {
            match a[1..].as_ref() {
                "o" => {
                    if i + 1 < args.len() {
                        push_flag(&mut flags, "format", args[i + 1].clone());
                        i += 2;
                    } else {
                        push_flag(&mut flags, "format", "true".into());
                        i += 1;
                    }
                    continue;
                }
                "q" => {
                    push_flag(&mut flags, "quiet", "true".into());
                    i += 1;
                    continue;
                }
                "h" => {
                    push_flag(&mut flags, "help", "true".into());
                    i += 1;
                    continue;
                }
                "V" => {
                    push_flag(&mut flags, "version", "true".into());
                    i += 1;
                    continue;
                }
                "a" => {
                    push_flag(&mut flags, "all", "true".into());
                    i += 1;
                    continue;
                }
                "g" => {
                    push_flag(&mut flags, "global", "true".into());
                    i += 1;
                    continue;
                }
                "i" => {
                    if i + 1 < args.len() {
                        push_flag(&mut flags, "image", args[i + 1].clone());
                        i += 2;
                    } else {
                        usage_error("run --image: missing value");
                        i += 1;
                    }
                    continue;
                }
                _ => {
                    unknown.push(a.clone());
                    i += 1;
                    continue;
                }
            }
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
    ParsedArgs { flags, pos, unknown }
}

// 只有单测用得到薄包装；dispatch 已改走 parse_args，非 test 构建下它是死代码，
// 而 CI 带 RUSTFLAGS=-D warnings，dead_code 会直接把 Windows job 打红。
#[allow(dead_code)]
fn parse_flags(args: &[String]) -> (HashMap<String, String>, Vec<String>) {
    let p = parse_args(args);
    (p.flags, p.pos)
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


fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `--label k=v` 解析；key 不能为空，值可含 `=`
fn parse_labels(vals: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for v in vals {
        let (k, val) = v
            .split_once('=')
            .ok_or_else(|| format!("label must be k=v: {}", v))?;
        if k.is_empty() {
            return Err(format!("label key must not be empty: {}", v));
        }
        out.insert(k.to_string(), val.to_string());
    }
    Ok(out)
}

/// `ls --label` 与 `run --label` 用同一套语法校验：必须有 `=`、key 非空。
/// 历史实现只 `flag_many` 原样收集，`ls --label team` 被静默忽略并返回全部 agent（R1）。
/// 与 `parse_labels` 不同，这里保留原顺序与重复项（列表查询要把每个 filter 都传给服务端）。
fn validate_label_filters(vals: &[String]) -> Result<Vec<String>, String> {
    for v in vals {
        let (k, _) = v
            .split_once('=')
            .ok_or_else(|| format!("label must be k=v: {}", v))?;
        if k.is_empty() {
            return Err(format!("label key must not be empty: {}", v));
        }
    }
    Ok(vals.to_vec())
}

/// `--env K=V` 解析；key 必须是合法环境变量名
fn parse_env(vals: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for v in vals {
        let (k, val) = v
            .split_once('=')
            .ok_or_else(|| format!("env must be K=V: {}", v))?;
        if !is_valid_env_key(k) {
            return Err(format!("invalid env key: {}", k));
        }
        out.push((k.to_string(), val.to_string()));
    }
    Ok(out)
}

/// `--image` 必须是可读的已存在文件路径
pub fn validate_image_path(p: &str) -> Result<(), String> {
    if std::path::Path::new(p).is_file() {
        Ok(())
    } else {
        Err(format!("run --image: no such file: {}", p))
    }
}

/// `--output-schema` 必须是已存在文件，内容为带 `"type"` 的 JSON 对象
fn validate_output_schema(path: &str) -> Result<(), String> {
    let bad = || format!("run --output-schema: not a JSON object with type: {}", path);
    let text = std::fs::read_to_string(path).map_err(|_| bad())?;
    let value: Value = serde_json::from_str(&text).map_err(|_| bad())?;
    if value.get("type").is_some() {
        Ok(())
    } else {
        Err(bad())
    }
}

/// run options 的值校验（CLI 与 HTTP 共用），返回规范化后的有序拷贝
pub fn validate_run_options(raw: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        match k.as_str() {
            "thinking" => {
                if !matches!(v.as_str(), "none" | "low" | "medium" | "high") {
                    return Err("run --thinking must be one of none|low|medium|high".to_string());
                }
            }
            "mode" => {
                if !matches!(v.as_str(), "read-only" | "workspace-write" | "danger-full-access") {
                    return Err(
                        "run --mode must be one of read-only|workspace-write|danger-full-access".to_string(),
                    );
                }
            }
            "provider" => {
                let valid = !v.is_empty()
                    && v.len() <= 40
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
                if !valid {
                    return Err(format!("run --provider: invalid value: {}", v));
                }
            }
            "output_schema" => validate_output_schema(v)?,
            other => return Err(format!("unknown run option: {}", other)),
        }
        out.insert(k.clone(), v.clone());
    }
    Ok(out)
}

#[derive(Debug)]
struct RunPlan {
    kind: String,
    model: Option<String>,
    workspace: Option<String>,
    title: Option<String>,
    timeout_s: Option<u64>,
    worktree: bool,
    spec: WorktreeSpec,
    worktree_mode: Option<String>,
    pr_number: Option<String>,
    labels: BTreeMap<String, String>,
    env: Vec<(String, String)>,
    cwd: Option<String>,
    options: BTreeMap<String, String>,
    images: Vec<String>,
    wait_timeout: Option<u64>,
}

/// 把 `run` 的 flags 收敛成 RunPlan；先校验不支持项，避免无效请求打到服务端
fn run_plan(flags: &HashMap<String, String>) -> Result<RunPlan, String> {
    let spec = workspaces::spec_from_parts(
        flags.get("worktree-slug").cloned(),
        flags.get("new-branch").cloned(),
        flags.get("base").cloned(),
        flags.get("branch").cloned(),
    )?;
    let worktree_mode = flags.get("worktree-mode").cloned();
    if let Some(m) = worktree_mode.as_deref() {
        if !matches!(m, "branch-off" | "checkout-branch" | "checkout-pr") {
            return Err(format!(
                "run --worktree-mode: expected branch-off, checkout-branch or checkout-pr, got {}",
                m
            ));
        }
    }
    if worktree_mode.as_deref() == Some("checkout-pr")
        && flags.get("pr-number").map(|p| p.is_empty()).unwrap_or(true)
    {
        return Err("checkout-pr requires --pr-number".to_string());
    }
    let mut worktree = matches!(worktree_mode.as_deref(), Some("branch-off") | Some("checkout-branch"));
    if let Some(v) = flags.get("new-workspace") {
        match v.as_str() {
            "worktree" => worktree = true,
            "local" => worktree = false,
            other => {
                return Err(format!("run --new-workspace: expected local or worktree, got {}", other))
            }
        }
    }
    if spec.slug.is_some() && flags.get("new-workspace").map(String::as_str) != Some("worktree") {
        return Err("run --worktree-slug requires --new-workspace worktree".to_string());
    }
    if spec.base.is_some()
        && matches!(
            worktree_mode.as_deref(),
            Some("checkout-pr") | Some("checkout-branch")
        )
    {
        return Err("run --base: only valid with branch-off".to_string());
    }
    let images = flag_many(flags, "image");
    for img in &images {
        validate_image_path(img)?;
    }
    let mut options = BTreeMap::new();
    for key in ["thinking", "mode", "provider", "output-schema"] {
        if let Some(v) = flags.get(key) {
            let k = if key == "output-schema" { "output_schema" } else { key };
            options.insert(k.to_string(), v.clone());
        }
    }
    let options = validate_run_options(&options)?;
    let kind = flags.get("agent").cloned().unwrap_or_else(|| "codex".into());
    crate::runner::provider_args(
        &kind,
        &options,
        &images,
        options.get("mode").map(String::as_str),
    )?;
    Ok(RunPlan {
        kind,
        model: flags.get("model").cloned(),
        workspace: flags.get("workspace").cloned(),
        title: flags.get("title").cloned(),
        timeout_s: flags.get("timeout").and_then(|v| v.parse::<u64>().ok()),
        worktree,
        spec,
        worktree_mode,
        pr_number: flags.get("pr-number").cloned().filter(|p| !p.is_empty()),
        labels: parse_labels(&flag_many(flags, "label"))?,
        env: parse_env(&flag_many(flags, "env"))?,
        cwd: flags.get("cwd").cloned().filter(|c| !c.is_empty()),
        options,
        images,
        wait_timeout: flags.get("wait-timeout").and_then(|v| v.parse::<u64>().ok()),
    })
}

/// 构造 `POST /v1/agents` 的 body，避免字段所有权被 json! 移动
fn run_body(plan: &RunPlan, prompt: &str) -> Value {
    let worktree_value = if plan.spec.slug.is_some() || plan.spec.branch.is_some() || plan.spec.base.is_some() {
        json!({
            "slug": plan.spec.slug.clone(),
            "branch": if plan.spec.existing { plan.spec.branch.clone() } else { None },
            "new_branch": if plan.spec.existing { None } else { plan.spec.branch.clone() },
            "base": plan.spec.base.clone(),
        })
    } else {
        json!(plan.worktree)
    };
    json!({
        "kind": plan.kind.clone(),
        "model": plan.model.clone(),
        "workspace": plan.workspace.clone(),
        "title": plan.title.clone(),
        "prompt": prompt,
        "timeout_s": plan.timeout_s,
        "worktree": worktree_value,
        "labels": plan.labels.clone(),
        "env": plan.env.clone(),
        "cwd": plan.cwd.clone(),
        "worktree_mode": plan.worktree_mode.clone(),
        "pr_number": plan.pr_number.clone(),
        "options": plan.options.clone(),
        "images": plan.images.clone(),
    })
}

/// 从 server 返回的错误文本里抽出 `error` 字段；抽不出就原样返回。
fn server_error_message(raw: &str) -> String {
    if let Some(idx) = raw.find('{') {
        if let Ok(v) = serde_json::from_str::<Value>(&raw[idx..]) {
            if let Some(msg) = v["error"].as_str() {
                return msg.to_string();
            }
        }
    }
    raw.to_string()
}

/// worktree / checkout-pr 校验失败属于用法错误（exit 2），其余走 dispatch failed。
fn is_run_usage_error(msg: &str) -> bool {
    msg.starts_with("run --worktree-slug")
        || msg.starts_with("run --new-branch")
        || msg.starts_with("run --base")
        || msg.starts_with("run --branch")
        || msg == "checkout-pr requires gh in PATH"
}

#[derive(Debug)]
struct LsPlan {
    runs: bool,
    global: bool,
    all: bool,
    labels: Vec<String>,
    limit: usize,
    kind: Option<String>,
}

fn ls_plan(flags: &HashMap<String, String>) -> Result<LsPlan, String> {
    Ok(LsPlan {
        runs: flags.contains_key("runs"),
        global: flags.contains_key("global"),
        all: flags.contains_key("all"),
        labels: validate_label_filters(&flag_many(flags, "label"))?,
        limit: flags
            .get("limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50)
            .clamp(1, 500),
        kind: flags.get("kind").cloned().filter(|k| !k.is_empty()),
    })
}

/// `GET /v1/agents` 的查询串；label 值把 `=` 编码为 `%3D`，顺序固定便于断言
fn agent_list_url(plan: &LsPlan) -> String {
    let mut parts: Vec<String> = Vec::new();
    if plan.all {
        parts.push("all=1".to_string());
    }
    for l in &plan.labels {
        parts.push(format!("label={}", percent_encode(l)));
    }
    parts.push(format!("limit={}", plan.limit));
    if let Some(kind) = plan.kind.as_deref() {
        parts.push(format!("kind={}", percent_encode(kind)));
    }
    if parts.is_empty() {
        "/v1/agents".to_string()
    } else {
        format!("/v1/agents?{}", parts.join("&"))
    }
}

fn format_is_json(flags: &HashMap<String, String>) -> bool {
    flags.contains_key("json") || flags.get("format").map(|f| f == "json").unwrap_or(false)
}

fn format_is_yaml(flags: &HashMap<String, String>) -> bool {
    flags.get("format").map(|f| f == "yaml").unwrap_or(false)
}

fn format_value(flags: &HashMap<String, String>, v: &Value) -> String {
    if format_is_yaml(flags) {
        to_yaml(v)
    } else {
        format!("{}\n", serde_json::to_string_pretty(v).unwrap_or_default())
    }
}

fn print_formatted(flags: &HashMap<String, String>, v: &Value) {
    print!("{}", format_value(flags, v));
}

fn yaml_plain(s: &str) -> bool {
    !s.is_empty()
        && !matches!(
            s,
            "null" | "Null" | "NULL" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE" | "~" | "yes" | "no" | "on" | "off"
        )
        && !s.trim().is_empty()
        && s == s.trim()
        && !s.contains(|ch: char| {
            matches!(
                ch,
                ':' | '#' | '{' | '}' | '[' | ']' | ',' | '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%' | '@' | '`' | '\n' | '\r'
            ) || ch.is_ascii_control()
        })
        && s.parse::<f64>().is_err()
        && !s.starts_with('-')
        && !s.starts_with('?')
        && !s.starts_with(':')
}

fn yaml_scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => yaml_string(s),
        Value::Array(a) if a.is_empty() => "[]".to_string(),
        Value::Object(m) if m.is_empty() => "{}".to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn yaml_string(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if yaml_plain(s) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

fn yaml_inlineable(v: &Value) -> bool {
    match v {
        Value::String(s) => !s.contains('\n') && !s.contains('\r'),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        _ => true,
    }
}

fn emit_yaml(v: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                out.push_str(&format!("{}{}:", pad, yaml_string(k)));
                if yaml_inlineable(child) {
                    out.push(' ');
                    out.push_str(&yaml_scalar(child));
                    out.push('\n');
                } else {
                    out.push('\n');
                    emit_yaml(child, indent + 1, out);
                }
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                out.push_str(&format!("{}[]\n", pad));
                return;
            }
            for item in arr {
                match item {
                    Value::Object(map) => {
                        let mut iter = map.iter();
                        if let Some((k, child)) = iter.next() {
                            out.push_str(&format!("{}- {}:", pad, yaml_string(k)));
                            if yaml_inlineable(child) {
                                out.push(' ');
                                out.push_str(&yaml_scalar(child));
                                out.push('\n');
                            } else {
                                out.push('\n');
                                emit_yaml(child, indent + 2, out);
                            }
                        }
                        for (k, child) in iter {
                            out.push_str(&format!("{}  {}:", pad, yaml_string(k)));
                            if yaml_inlineable(child) {
                                out.push(' ');
                                out.push_str(&yaml_scalar(child));
                                out.push('\n');
                            } else {
                                out.push('\n');
                                emit_yaml(child, indent + 2, out);
                            }
                        }
                    }
                    Value::String(s) if s.contains('\n') || s.contains('\r') => {
                        out.push_str(&format!("{}-\n", pad));
                        out.push_str(&format!("{}  |-\n", pad));
                        for line in s.lines() {
                            out.push_str(&format!("{}    {}\n", pad, line));
                        }
                    }
                    other => {
                        out.push_str(&format!("{}- {}\n", pad, yaml_scalar(other)));
                    }
                }
            }
        }
        Value::String(s) if s.contains('\n') || s.contains('\r') => {
            out.push_str(&format!("{}|-\n", pad));
            for line in s.lines() {
                out.push_str(&format!("{}  {}\n", pad, line));
            }
        }
        other => {
            out.push_str(&format!("{}{}\n", pad, yaml_scalar(other)));
        }
    }
}

fn to_yaml(v: &Value) -> String {
    let mut out = String::new();
    emit_yaml(v, 0, &mut out);
    out
}

fn delete_plan(id: &str, running: bool) -> Vec<(String, String)> {
    let mut plan = Vec::new();
    if running {
        plan.push(("POST".to_string(), format!("/v1/runs/{}/stop", id)));
    }
    plan.push(("DELETE".to_string(), format!("/v1/runs/{}", id)));
    plan
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
            Ok(resp) => resp.into_json::<Value>().map_err(|e| {
                fail(1);
                format!("bad response: {}", e)
            }),
            Err(ureq::Error::Status(code, resp)) => {
                let t = resp.into_string().unwrap_or_default();
                fail(1);
                Err(format!("HTTP {}: {}", code, t))
            }
            Err(e) => {
                fail(1);
                Err(e.to_string())
            }
        }
    }
}

fn local_base(flags: &HashMap<String, String>) -> String {
    if let Ok(raw) = std::fs::read_to_string(data_dir_for(flags).join("bind")) {
        let raw = raw.trim().to_string();
        if !raw.is_empty() {
            // Wildcard binds (0.0.0.0 / ::) are listen addresses: sending them as a Host
            // header fails the --allow-host check with 421. Map to loopback, keep the port.
            return hosts::client_base_from_bind(&raw);
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
    let (code, text) = match hosts::dispatch(host, method, path, None) {
        Ok(v) => v,
        Err(e) => {
            fail(1);
            return Err(e);
        }
    };
    if !(200..300).contains(&code) {
        fail(1);
        return Err(format!("HTTP {}: {}", code, text));
    }
    serde_json::from_str::<Value>(&text).map_err(|e| {
        fail(1);
        format!("bad response: {}", e)
    })
}

fn request(flags: &HashMap<String, String>, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    if let Some(h) = resolve_host(flags) {
        let body_str = match body {
            Some(v) => Some(serde_json::to_string(&v).map_err(|e| e.to_string())?),
            None => None,
        };
        let (code, text) = match hosts::dispatch(&h, method, path, body_str.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                fail(1);
                return Err(e);
            }
        };
        if !(200..300).contains(&code) {
            fail(1);
            return Err(format!("HTTP {}: {}", code, text));
        }
        return serde_json::from_str::<Value>(&text).map_err(|e| {
            fail(1);
            format!("bad response: {}", e)
        });
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
    std::process::exit(dispatch(&argv));
}

fn print_command_help(cmd: &str) -> bool {
    let text = match cmd {
        "serve" => "roamway serve [--bind 127.0.0.1:8796] [--data DIR] [--token T] [--tz 8] [--max-concurrent 4] [--max-conn 64]",
        "start" => "roamway start [--bind 127.0.0.1:8796] [--data DIR] [--token T]",
        "clone" => "roamway clone <owner/repo | git url> [--dir DIR] [--protocol https|ssh] [--json] [--host H]",
        "workspace" => "roamway workspace ls | create <name> --path DIR | archive <name> | setup <name> | rm <name>",
        "terminal" | "term" => "roamway terminal create|ls|capture|send-keys|kill  (受管终端无 TTY，TUI 程序不支持)",
        "script" | "scripts" => "roamway script ls|add --name N --cmd C|rm <name>|start <name>|stop <name>",
        "provider" => "roamway provider [ls|models [<id>] [--refresh]|diagnostic <id>]",
        "usage" => "roamway usage [ls [--since 7d|24h|ISO] [--provider id]|inspect <run_id>] [--json]",
        "plugin" => "roamway plugin ls|init <name>|logs <name>|reload <name>|enable|disable <name> [--trust]|remove <name> [--keep-files]",
        "speech" => "roamway speech [transcribe <file>|say <text> [--out file]]",
        "run" => "roamway run --agent codex [--model M] [--workspace DIR] [--title T] [--timeout 1800] [--no-wait] [--wait-timeout N] [--resume <agent-id> | --continue <agent-id>] [--new-workspace local|worktree] [--worktree-mode branch-off|checkout-branch|checkout-pr] [--pr-number N] [--worktree-slug SLUG] [--new-branch B] [--base REF] [--branch B] [--cwd DIR] [--env K=V] [--label k=v] [--image PATH] [-i PATH] [--host H] \"prompt\"",
        "send" => "roamway send <agent-id> <prompt...>",
        "delete" => "roamway delete <id> [--yes]",
        "ls" | "list" => "roamway ls [--limit 50] [-a|--all] [-g|--global] [--label k=v] [--kind K] [--host H|all] [-q] [--no-headers] [--json|--format table|json|yaml] ; --runs 看运行记录",
        "status" => "roamway status [ID] [--json] [--host H]",
        "schedule" => "roamway schedule create|ls|inspect|logs|pause|resume|delete|run-once|update [--host H]",
        "hooks" => "roamway hooks [--json] [--host H]",
        "hub" => "roamway hub [init [--name N] [--force] | permissions | projects | deploy | logout | status | token rotate]",
        "agent" => "roamway agent run|ls|inspect|logs|wait|attach|archive|reload|update|open|detach|import ...",
        "ui" => "roamway ui [--print] [--board]",
        "attach" => "roamway attach <id>",
        "inspect" => "roamway inspect <id>",
        "wait" => "roamway wait <id> [--timeout 60]",
        "archive" => "roamway archive <id> [--keep-worktree]",
        "import" => "roamway import <id> [--session S] [--kind K]",
        "logs" => "roamway logs ID [--tail 80] [--host H]",
        "stop" => "roamway stop ID [--host H]",
        "rm" => "roamway rm ID",
        "prune" => "roamway prune [--dry-run] [--keep N] [--max-age-days N] [--yes]",
        "backup" => "roamway backup [--out DIR] [--keep N]",
        "restore" => "roamway restore --from DIR [--data DIR] [--force] [--yes]",
        "jobs" => "roamway jobs",
        "jobs-add" => "roamway jobs-add --agent shell --every 3600 | --daily 08:30 [--model M] [--workspace DIR] [--max-retries 2] [--after JOB]... \"prompt\"",
        "jobs-run" => "roamway jobs-run ID",
        "jobs-rm" => "roamway jobs-rm ID",
        "health" => "roamway health",
        "host" => "roamway host add <name> --endpoint <ep> [--token T] [--force] | ls [--probe] [--json] | rm <name> | enable|disable <name> | probe <name>",
        "token" => "roamway token add|create --name N [--scope read|run|admin] [--ttl 24h] | ls|list | revoke <id>",
        "pair" => "roamway pair [--json] [--host H]",
        "approve" => "roamway approve <id>",
        "deny" => "roamway deny <id>",
        "approvals" => "roamway approvals [--all]",
        "daemon" => "roamway daemon status|config [get <key>|set <key> <value>|unset <key>]|start|stop|restart|reload|pair|set-password|run [--foreground]",
        "onboard" => "roamway onboard [--bind 0.0.0.0:8796]",
        "restart" => "roamway restart",
        "reload" => "roamway reload",
        "permit" => "roamway permit ls | approve <id> | deny <id>",
        "heartbeat" => "roamway heartbeat ls [--agent id] | create --name N --agent id --cron C|--every E \"prompt\" | update <id> | delete <id>",
        "project" => "roamway project ls|delete",
        "version" => "roamway version",
        _ => return false,
    };
    println!("{}", text);
    true
}

fn dispatch(argv: &[String]) -> i32 {
    FAIL_CODE.store(0, Ordering::SeqCst);
    let parsed = parse_args(argv);
    let (flags, pos) = (parsed.flags, parsed.pos);
    if let Some(first) = parsed.unknown.first() {
        eprintln!("unknown flag: {}", first);
        eprintln!("pass it after '--' if it is part of the prompt");
        eprintln!("run 'roamway help' for usage");
        return 2;
    }
    if FAIL_CODE.load(Ordering::SeqCst) != 0 {
        return FAIL_CODE.load(Ordering::SeqCst);
    }
    if pos.is_empty() {
        if flags.contains_key("version") {
            println!("roamway {}", env!("CARGO_PKG_VERSION"));
            return 0;
        }
        if flags.contains_key("help") {
            usage();
            return 0;
        }
        usage();
        return 0;
    }

    let mut flags = flags;
    if flags.get("home").is_some() && flags.get("data").is_none() {
        let home = flags.get("home").cloned().unwrap();
        flags.insert("data".to_string(), home);
    }

    let cmd = pos[0].clone();
    let cmd_pos: Vec<String> = pos[1..].to_vec();

    if flags.contains_key("help") {
        if !print_command_help(&cmd) {
            eprintln!("unknown command: {}", cmd);
            eprintln!("run 'roamway help'");
            return 2;
        }
        return 0;
    }
    if flags.contains_key("version") {
        println!("roamway {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if cmd == "help" {
        let target = cmd_pos.first().map(String::as_str).unwrap_or("");
        if target.is_empty() {
            usage();
            return 0;
        }
        if !print_command_help(target) {
            eprintln!("unknown command: {}", target);
            eprintln!("run 'roamway help'");
            return 2;
        }
        return 0;
    }

    FAIL_CODE.store(0, Ordering::SeqCst);
    match cmd.as_str() {
        "serve" => cmd_serve(&flags),
        "start" => cmd_daemon_start(&flags),
        "clone" => cmd_clone(&flags, &cmd_pos),
        "workspace" => cmd_workspace(&flags, &cmd_pos),
        "terminal" | "term" => cmd_terminal(&flags, &cmd_pos),
        "script" | "scripts" => cmd_script(&flags, &cmd_pos),
        "provider" => cmd_provider(&flags, &cmd_pos),
        "usage" => cmd_usage(&flags, &cmd_pos),
        "plugin" => cmd_plugin(&flags, &cmd_pos),
        "speech" => cmd_speech(&flags, &cmd_pos),
        "run" => cmd_run(&flags, &cmd_pos),
        "send" => cmd_send(&flags, &cmd_pos),
        "delete" => cmd_delete(&flags, &cmd_pos),
        "permit" => cmd_permit(&flags, &cmd_pos),
        "heartbeat" => cmd_heartbeat(&flags, &cmd_pos),
        "project" => cmd_project(&flags, &cmd_pos),
        "ls" | "list" => cmd_ls(&flags),
        "status" => {
            if cmd_pos.is_empty() {
                cmd_status_summary(&flags);
            } else {
                cmd_status(&flags, &cmd_pos);
            }
        }
        "schedule" => cmd_schedule(&flags, &cmd_pos),
        "hooks" => cmd_hooks(&flags),
        "hub" => cmd_hub(&flags, &cmd_pos),
        "agent" => cmd_agent(&flags, &cmd_pos),
        "ui" => cmd_ui(&flags, &cmd_pos),
        "attach" => cmd_agent_attach(&flags, &cmd_pos),
        "inspect" => cmd_agent_inspect(&flags, &cmd_pos),
        "wait" => cmd_agent_wait(&flags, &cmd_pos),
        "archive" => cmd_agent_archive(&flags, &cmd_pos),
        "import" => cmd_agent_import(&flags, &cmd_pos),
        "logs" => cmd_logs(&flags, &cmd_pos),
        "stop" => cmd_stop(&flags, &cmd_pos),
        "rm" => cmd_rm(&flags, &cmd_pos),
        "prune" => cmd_prune(&flags, &cmd_pos),
        "backup" => cmd_backup(&flags, &cmd_pos),
        "restore" => cmd_restore(&flags, &cmd_pos),
        "jobs" => cmd_jobs(&flags),
        "jobs-add" => cmd_jobs_add(&flags, &cmd_pos),
        "jobs-run" => cmd_jobs_run(&flags, &cmd_pos),
        "jobs-rm" => cmd_jobs_rm(&flags, &cmd_pos),
        "health" => cmd_health(&flags),
        "host" => cmd_host(&flags, &cmd_pos),
        "token" => cmd_token(&flags, &cmd_pos),
        "pair" => cmd_pair(&flags),
        "approve" => cmd_approve(&flags, &cmd_pos),
        "deny" => cmd_deny(&flags, &cmd_pos),
        "approvals" => cmd_approvals(&flags),
        "daemon" => cmd_daemon(&flags, &cmd_pos),
        "onboard" => cmd_onboard(&flags),
        "restart" => cmd_restart(&flags),
        "reload" => cmd_reload(&flags),
        "version" => println!("roamway {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("unknown command: {}", cmd);
            eprintln!("run 'roamway help'");
            fail(2);
        }
    }
    FAIL_CODE.load(Ordering::SeqCst)
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

fn env_map() -> HashMap<String, String> {
    std::env::vars().collect()
}

fn resolved_config(flags: &HashMap<String, String>) -> Result<config::EffectiveConfig, String> {
    let file = config::load(&data_dir_for(flags))?;
    config::resolve_config(flags, &env_map(), &file)
}

/// 守护进程常见坑：非登录 shell 的 PATH 找不到 ~/.npm-global/bin 里的 codex。
/// 只能追加、不能前置：调用方显式设置 PATH 时（自定义安装路径、验收用替身）最前面的
/// 目录必须继续优先，前置会把这些目录顶掉，导致实际跑起来的不是预期那个二进制。
fn path_with_fallbacks(cur: &str, home: &str) -> String {
    let extras = [
        format!("{}/.npm-global/bin", home),
        format!("{}/.local/bin", home),
        "/usr/local/bin".to_string(),
    ];
    let mut parts: Vec<String> = cur
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for e in extras {
        if !parts.iter().any(|p| p == &e) {
            parts.push(e);
        }
    }
    parts.join(":")
}

fn cmd_serve(flags: &HashMap<String, String>) {
    if let Ok(home) = std::env::var("HOME") {
        let cur = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", path_with_fallbacks(&cur, &home));
    }
    let cfg = match resolved_config(flags) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            fail(2);
            return;
        }
    };
    let bind = cfg.bind.clone();
    let data = data_dir_for(flags);
    let token = flags
        .get("token")
        .cloned()
        .or_else(|| std::env::var("ROAMWAY_TOKEN").ok())
        .filter(|t| !t.is_empty());
    let tz = cfg.tz_offset_hours;
    let max_concurrent = cfg.max_concurrent;
    let max_conn = cfg.max_conn;
    let allow_hosts = cfg.allow_hosts.clone();
    let allow_origins = cfg.allow_origins.clone();
    let require_approval = cfg.require_approval.clone();
    let unix_socket = cfg.unix_socket.clone().map(PathBuf::from);
    let tls_cert = cfg.tls_cert.clone().map(PathBuf::from);
    let tls_key = cfg.tls_key.clone().map(PathBuf::from);
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

    let mut store = Store::load(&data);
    if jobs::prune(&mut store, cfg.keep_runs, cfg.keep_days) {
        store.save();
    }
    if let Err(e) = backup::write_pid_file(&data) {
        eprintln!("roamway: write pid file failed: {}", e);
    }
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
        keep_runs: cfg.keep_runs,
        keep_days: cfg.keep_days,
        max_log_bytes: cfg.max_log_bytes,
        max_conn,
        conns: AtomicUsize::new(0),
        req_total: AtomicU64::new(0),
        req_failed: AtomicU64::new(0),

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
    match server::serve(app, &bind) {
        Ok(()) => backup::clear_pid_file(&data),
        Err(e) => {
            eprintln!("serve failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn wait_run(
    flags: &HashMap<String, String>,
    id: &str,
    wait_timeout: Option<u64>,
    fetch_status: &mut dyn FnMut(&str) -> Result<Value, String>,
    fetch_log: &mut dyn FnMut(&str) -> Result<Value, String>,
) {
    let started = std::time::Instant::now();
    let mut last_status = String::from("?");
    loop {
        if let Some(n) = wait_timeout {
            if started.elapsed().as_secs() > n {
                eprintln!("wait timeout after {}s; run {} still {}", n, id, last_status);
                fail(1);
                return;
            }
        }
        thread::sleep(Duration::from_secs(2));
        match fetch_status(id) {
            Ok(v) => {
                let st = v["run"]["status"].as_str().unwrap_or("?").to_string();
                last_status = st.clone();
                if matches!(st.as_str(), "done" | "failed" | "timeout" | "killed" | "interrupted") {
                    if format_is_json(flags) {
                        print_formatted(flags, &v["run"]);
                        return;
                    }
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
                    if let Ok(l) = fetch_log(id) {
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

fn resume_target(
    flags: &HashMap<String, String>,
    pos: &[String],
) -> Result<(String, String), String> {
    let has_resume = flags.contains_key("resume");
    let has_continue = flags.contains_key("continue");
    if has_resume && has_continue {
        return Err("run: --resume and --continue are mutually exclusive".into());
    }
    if !has_resume && !has_continue {
        return Err("run: --resume requires an agent id".into());
    }
    let agent_id = pos
        .first()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "resume agent required".to_string())?
        .clone();
    let prompt = pos[1..].join(" ");
    if prompt.trim().is_empty() {
        return Err("prompt required".into());
    }
    Ok((agent_id, prompt))
}

fn cmd_run_resume(flags: &HashMap<String, String>, pos: &[String]) {
    let (agent_id, prompt) = match resume_target(flags, pos) {
        Ok(v) => v,
        Err(e) => {
            usage_error(&e);
            return;
        }
    };
    let body = json!({
        "prompt": prompt,
        "timeout_s": flags.get("timeout").and_then(|v| v.parse::<u64>().ok()),
    });
    match request(flags, "POST", &format!("/v1/agents/{}/run", agent_id), Some(body)) {
        Ok(v) => {
            let id = v["run"]["id"].as_str().unwrap_or("").to_string();
            if flags.contains_key("no-wait") {
                println!("{}", id);
                return;
            }
            let mut fetch_status =
                |rid: &str| request(flags, "GET", &format!("/v1/runs/{}", rid), None);
            let mut fetch_log =
                |rid: &str| request(flags, "GET", &format!("/v1/runs/{}/log?tail=40", rid), None);
            let wait_timeout = flags.get("wait-timeout").and_then(|v| v.parse::<u64>().ok());
            wait_run(flags, &id, wait_timeout, &mut fetch_status, &mut fetch_log);
        }
        Err(e) => eprintln!("resume failed: {}", e),
    }
}

fn cmd_run(flags: &HashMap<String, String>, pos: &[String]) {
    if flags.contains_key("resume") || flags.contains_key("continue") {
        cmd_run_resume(flags, pos);
        return;
    }
    let mut prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        prompt = flags.get("prompt").cloned().unwrap_or_default();
    }
    if prompt.trim().is_empty() {
        usage_error("prompt required");
        return;
    }
    let plan = match run_plan(flags) {
        Ok(p) => p,
        Err(e) => {
            usage_error(&e);
            return;
        }
    };
    if let Some(h) = resolve_host(flags) {
        cmd_run_remote(&h, flags, &pos, &plan);
        return;
    }
    let c = local_client(flags);
    let body = run_body(&plan, &prompt);
    let v = match c.req("POST", "/v1/agents", Some(body)) {
        Ok(v) => v,
        Err(e) => {
            let msg = server_error_message(&e);
            if is_run_usage_error(&msg) {
                usage_error(&msg);
            } else {
                eprintln!("dispatch failed: {}", e);
            }
            return;
        }
    };
    let id = v["run"]["id"].as_str().unwrap_or("").to_string();
    if flags.contains_key("no-wait") {
        println!("{}", id);
        return;
    }
    let mut fetch_status = |rid: &str| c.req("GET", &format!("/v1/runs/{}", rid), None);
    let mut fetch_log = |rid: &str| c.req("GET", &format!("/v1/runs/{}/log?tail=40", rid), None);
    wait_run(flags, &id, plan.wait_timeout, &mut fetch_status, &mut fetch_log);
}

fn trunc(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s.to_string()
    } else {
        chars[..n].iter().collect::<String>() + "…"
    }
}

fn print_agents(flags: &HashMap<String, String>, v: &Value) {
    if format_is_json(flags) || format_is_yaml(flags) {
        print_formatted(flags, v);
        return;
    }
    let empty = vec![];
    let agents = v["agents"].as_array().unwrap_or(&empty);
    if flags.contains_key("quiet") {
        for a in agents {
            println!("{}", a["id"].as_str().unwrap_or(""));
        }
        return;
    }
    if !flags.contains_key("no-headers") {
        println!(
            "{:<12} {:<10} {:<14} {:<16} {:<22} {}",
            "ID", "STATE", "KIND", "MODEL", "TITLE", "UPDATED"
        );
    }
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

fn cmd_ls(flags: &HashMap<String, String>) {
    if flags.contains_key("background") {
        usage_error("ls --background: not supported");
        return;
    }
    let plan = match ls_plan(flags) {
        Ok(p) => p,
        Err(e) => {
            usage_error(&e);
            return;
        }
    };
    if plan.runs {
        cmd_ls_runs(flags);
        return;
    }
    let global = plan.global || host_flag(flags).as_deref() == Some("all");
    if global {
        cmd_ls_agents_global(flags, &plan);
        return;
    }
    cmd_ls_agents_single(flags, &plan);
}

fn cmd_ls_agents_single(flags: &HashMap<String, String>, plan: &LsPlan) {
    match request(flags, "GET", &agent_list_url(plan), None) {
        Ok(v) => print_agents(flags, &v),
        Err(e) => eprintln!("list failed: {}", e),
    }
}

fn cmd_ls_agents_global(flags: &HashMap<String, String>, plan: &LsPlan) {
    let reg = registry_for(flags);
    let mut all: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    match local_only_client(flags).req("GET", &agent_list_url(plan), None) {
        Ok(v) => {
            for mut a in v["agents"].as_array().cloned().unwrap_or_default() {
                a["host"] = json!("local");
                all.push(a);
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
                    let (code, text) = hosts::dispatch(h, "GET", &agent_list_url(plan), None)?;
                    if !(200..300).contains(&code) {
                        return Err(format!("{}: HTTP {}: {}", h.name, code, text));
                    }
                    let v: Value =
                        serde_json::from_str(&text).map_err(|e| format!("{}: bad response: {}", h.name, e))?;
                    Ok(v["agents"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|mut a| {
                            a["host"] = json!(h.name);
                            a
                        })
                        .collect())
                })
            })
            .collect();
        for hd in handles {
            match hd.join() {
                Ok(Ok(agents)) => all.extend(agents),
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
    if format_is_json(flags) || format_is_yaml(flags) {
        print_formatted(flags, &json!({"agents": all}));
    } else if flags.contains_key("quiet") {
        for a in &all {
            println!("{}", a["id"].as_str().unwrap_or(""));
        }
    } else {
        println!(
            "{:<10} {:<12} {:<10} {:<14} {:<16} {:<22} {}",
            "HOST", "ID", "STATE", "KIND", "MODEL", "TITLE", "UPDATED"
        );
        for a in &all {
            println!(
                "{:<10} {:<12} {:<10} {:<14} {:<16} {:<22} {}",
                a["host"].as_str().unwrap_or("-"),
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
    if !errors.is_empty() {
        eprintln!("errors:");
        for e in errors {
            eprintln!("  {}", e);
        }
    }
}

fn cmd_ls_runs(flags: &HashMap<String, String>) {
    // run 列表接口没有 label 过滤；静默忽略会让 `ls --runs --label k=v` 返回全部（同类 R1）
    if flags.contains_key("label") {
        usage_error("ls --runs: --label is not supported (run list has no label filter)");
        return;
    }

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
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v);
                return;
            }
            let empty = vec![];
            let runs = v["runs"].as_array().unwrap_or(&empty);
            if flags.contains_key("quiet") {
                for r in runs {
                    println!("{}", r["id"].as_str().unwrap_or(""));
                }
                return;
            }
            if !flags.contains_key("no-headers") {
                println!(
                    "{:<12} {:<12} {:<8} {:<20} {:<26} {}",
                    "ID", "STATUS", "AGENT", "MODEL", "CREATED", "TITLE"
                );
            }
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
        None => return usage_error("usage: roamway status ID"),
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
        None => return usage_error("usage: roamway logs ID [--tail 80]"),
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
        None => return usage_error("usage: roamway stop ID"),
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
        None => return usage_error("usage: roamway rm ID"),
    };
    let c = local_client(flags);
    match c.req("DELETE", &format!("/v1/runs/{}", id), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("rm failed: {}", e),
    }
}

fn prune_keep(flags: &HashMap<String, String>) -> Result<Option<usize>, ()> {
    match flags.get("keep") {
        None => Ok(None),
        Some(v) => match v.parse::<usize>() {
            Ok(n) => Ok(Some(n)),
            Err(_) => {
                usage_error("prune: --keep must be a non-negative integer");
                Err(())
            }
        },
    }
}

fn prune_max_age_days(flags: &HashMap<String, String>) -> Result<Option<u64>, ()> {
    match flags.get("max-age-days") {
        None => Ok(None),
        Some(v) => match v.parse::<u64>() {
            Ok(n) => Ok(Some(n)),
            Err(_) => {
                usage_error("prune: --max-age-days must be a non-negative integer");
                Err(())
            }
        },
    }
}

fn cmd_prune(flags: &HashMap<String, String>, pos: &[String]) {
    const PRUNE_FLAGS: &[&str] = &[
        "dry-run", "keep", "max-age-days", "yes", "host", "data", "home", "token", "format", "json",
        "quiet", "no-color", "no-headers",
    ];
    for key in flags.keys() {
        if !PRUNE_FLAGS.contains(&key.as_str()) {
            usage_error(&format!("prune: unknown flag --{}", key));
        }
    }
    if !pos.is_empty() {
        usage_error("usage: roamway prune [--dry-run] [--keep N] [--max-age-days N] [--yes]");
    }
    if FAIL_CODE.load(Ordering::SeqCst) != 0 {
        return;
    }
    let dry_run = flags.contains_key("dry-run");
    let keep = match prune_keep(flags) {
        Ok(k) => k,
        Err(()) => return,
    };
    let max_age_days = match prune_max_age_days(flags) {
        Ok(v) => v,
        Err(()) => return,
    };
    let mut body = json!({ "dry_run": dry_run });
    if let Some(k) = keep {
        body["keep"] = json!(k);
    }
    if let Some(v) = max_age_days {
        body["max_age_days"] = json!(v);
    }
    match request(flags, "POST", "/v1/prune", Some(body)) {
        Ok(v) => {
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v);
                return;
            }
            if !flags.contains_key("no-headers") {
                println!("RUNS_REMOVED LOGS_REMOVED BYTES_FREED MODE");
            }
            println!(
                "{} {} {} {}",
                v["runs_removed"].as_u64().unwrap_or(0),
                v["logs_removed"].as_u64().unwrap_or(0),
                v["bytes_freed"].as_u64().unwrap_or(0),
                if v["dry_run"].as_bool().unwrap_or(false) {
                    "dry-run"
                } else {
                    "apply"
                }
            );
        }
        Err(e) => eprintln!("prune failed: {}", e),
    }
}

fn cmd_backup(flags: &HashMap<String, String>, pos: &[String]) {
    const BACKUP_FLAGS: &[&str] = &[
        "out", "keep", "data", "home", "token", "format", "json", "quiet",
        "no-color", "no-headers",
    ];
    for key in flags.keys() {
        if !BACKUP_FLAGS.contains(&key.as_str()) {
            if key == "host" {
                usage_error("backup/restore are local-only");
                return;
            }
            usage_error(&format!("backup: unknown flag --{}", key));
        }
    }
    if !pos.is_empty() {
        usage_error("usage: roamway backup [--out DIR] [--keep N]");
    }
    if FAIL_CODE.load(Ordering::SeqCst) != 0 {
        return;
    }
    let keep = match flags.get("keep") {
        None => 0,
        Some(v) => match v.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                usage_error("backup: --keep must be a non-negative integer");
                return;
            }
        },
    };
    let data = data_dir_for(flags);
    let out = flags
        .get("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("backups"));
    let tz = match resolved_config(flags) {
        Ok(c) => c.tz_offset_hours,
        Err(e) => {
            eprintln!("{}", e);
            fail(2);
            return;
        }
    };
    maybe_flush_daemon_state(flags, &data);
    match backup::backup(&data, &out, keep, tz) {
        Ok(res) => {
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(
                    flags,
                    &json!({
                        "ok": true,
                        "dir": res.dir.display().to_string(),
                        "files": res.files,
                        "bytes": res.bytes,
                        "pruned": res.pruned,
                    }),
                );
                return;
            }
            if !flags.contains_key("no-headers") {
                println!("BACKUP_DIR FILES BYTES");
            }
            println!("{} {} {}", res.dir.display(), res.files, res.bytes);
        }
        Err(e) => {
            eprintln!("{}", e.message());
            fail(e.exit_code());
        }
    }
}

fn cmd_restore(flags: &HashMap<String, String>, pos: &[String]) {
    const RESTORE_FLAGS: &[&str] = &[
        "from", "force", "yes", "data", "home", "token", "format", "json", "quiet",
        "no-color", "no-headers",
    ];
    for key in flags.keys() {
        if !RESTORE_FLAGS.contains(&key.as_str()) {
            if key == "host" {
                usage_error("backup/restore are local-only");
                return;
            }
            usage_error(&format!("restore: unknown flag --{}", key));
        }
    }
    if !pos.is_empty() {
        usage_error("usage: roamway restore --from DIR [--data DIR] [--force] [--yes]");
    }
    if FAIL_CODE.load(Ordering::SeqCst) != 0 {
        return;
    }
    let from = match flags.get("from").map(PathBuf::from) {
        Some(dir) => dir,
        None => {
            usage_error("usage: roamway restore --from DIR [--data DIR] [--force] [--yes]");
            return;
        }
    };
    let data = data_dir_for(flags);
    let force = flags.contains_key("force");
    match backup::restore(&data, &from, force) {
        Ok(res) => {
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(
                    flags,
                    &json!({
                        "ok": true,
                        "files": res.files,
                        "from": res.from,
                    }),
                );
                return;
            }
            if !flags.contains_key("no-headers") {
                println!("RESTORED FROM");
            }
            println!("{}", res.from);
        }
        Err(e) => {
            eprintln!("{}", e.message());
            fail(e.exit_code());
        }
    }
}

/// 备份前尽量让 daemon 把内存态刷到 state.json：复用既有 /v1/prune 保存路径。
/// keep=usize::MAX 保证不删 run，just 触发一次落盘；daemon 不可达时静默回退为复制现有文件。
fn maybe_flush_daemon_state(flags: &HashMap<String, String>, data: &std::path::Path) {
    if backup::daemon_alive(data).is_none() {
        return;
    }
    let _ = local_only_client(flags).req(
        "POST",
        "/v1/prune",
        Some(json!({"dry_run": false, "keep": usize::MAX, "max_age_days": 0})),
    );
    FAIL_CODE.store(0, Ordering::SeqCst);
}

fn cmd_jobs(flags: &HashMap<String, String>) {
    let c = local_client(flags);
    match c.req("GET", "/v1/jobs", None) {
        Ok(v) => {
            let empty = vec![];
            let jobs = v["jobs"].as_array().unwrap_or(&empty);
            println!(
                "{:<10} {:<5} {:<8} {:<10} {:<26} {:<20} {:<16} {}",
                "ID", "ON", "AGENT", "EVERY", "NEXT", "LAST", "AFTER", "TITLE"
            );
            for j in jobs {
                let every = j["every_secs"]
                    .as_u64()
                    .map(|s| format!("{}s", s))
                    .or_else(|| j["daily"].as_str().map(|d| format!("@{}", d)))
                    .unwrap_or_else(|| "-".into());
                let after = j["depends_on"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "-".into());
                // last_skip 不单独占列，行尾追加，避免破坏既有对齐
                let suffix = match j["last_skip"].as_str() {
                    Some(s) if !s.is_empty() => format!("  ! {}", s),
                    _ => String::new(),
                };
                println!(
                    "{:<10} {:<5} {:<8} {:<10} {:<26} {:<20} {:<16} {}{}",
                    j["id"].as_str().unwrap_or(""),
                    if j["enabled"].as_bool().unwrap_or(false) { "yes" } else { "no" },
                    j["agent"].as_str().unwrap_or(""),
                    every,
                    j["next_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()),
                    j["last_run"].as_str().unwrap_or("-"),
                    after,
                    trunc(j["title"].as_str().unwrap_or(""), 36),
                    suffix
                );
            }
        }
        Err(e) => eprintln!("jobs failed: {}", e),
    }
}

fn cmd_jobs_add(flags: &HashMap<String, String>, pos: &[String]) {
    let prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        usage_error("prompt required");
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
    // --after 可重复：前置任务，全部成功后才允许触发
    let after = flag_many(flags, "after");
    if !after.is_empty() {
        body["depends_on"] = json!(after);
    }
    let c = local_client(flags);
    match c.req("POST", "/v1/jobs", Some(body)) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v["job"]).unwrap_or_default()),
        Err(e) => eprintln!("{}", api_error_text("jobs-add", &e)),
    }
}

/// 把 `HTTP 400: {"error":"jobs-add --after: ..."}` 收敛成裸文案，
/// 让 CLI 打印的文案与服务端校验完全一致（脚本可直接 grep）。
fn api_error_text(cmd: &str, raw: &str) -> String {
    if let Some(pos) = raw.find('{') {
        if let Ok(v) = serde_json::from_str::<Value>(&raw[pos..]) {
            if let Some(msg) = v["error"].as_str() {
                return msg.to_string();
            }
        }
    }
    format!("{} failed: {}", cmd, raw)
}

fn cmd_jobs_run(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return usage_error("usage: roamway jobs-run ID"),
    };
    let c = local_client(flags);
    match c.req("POST", &format!("/v1/jobs/{}/run", id), Some(json!({}))) {
        Ok(v) => {
            println!("{}", serde_json::to_string(&v).unwrap_or_default());
            if v["deps_bypassed"].as_bool().unwrap_or(false) && !format_is_json(flags) {
                eprintln!("note: dependencies not satisfied, running anyway");
            }
        }
        Err(e) => eprintln!("jobs-run failed: {}", e),
    }
}

fn cmd_jobs_rm(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return usage_error("usage: roamway jobs-rm ID"),
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
        _ => usage_error(
            "usage: roamway schedule create|ls|inspect|logs|pause|resume|delete|run-once|update ..."
        ),
    }
}

fn cmd_schedule_create(flags: &HashMap<String, String>, pos: &[String]) {
    let name = flags.get("name").cloned().unwrap_or_default();
    if name.trim().is_empty() {
        eprintln!("--name is required");
        fail(2);
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
        fail(2);
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
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v);
                return;
            }
            let empty = vec![];
            let scheds = v["schedules"].as_array().unwrap_or(&empty);
            if flags.contains_key("quiet") {
                for s in scheds {
                    println!("{}", s["id"].as_str().unwrap_or(""));
                }
                return;
            }
            if !flags.contains_key("no-headers") {
                println!(
                    "{:<13} {:<20} {:<26} {:<6} {:<26} {}",
                    "ID", "NAME", "NEXT", "RUNS", "LAST", "CRON"
                );
            }
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
        None => return usage_error("usage: roamway schedule inspect <id|name> [--json]"),
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
        None => return usage_error("usage: roamway schedule logs <id|name> [--tail 20]"),
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
        None => return usage_error("usage: roamway schedule pause <id|name>"),
    };
    match request(flags, "POST", &format!("/v1/schedules/{}/pause", key), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule pause failed: {}", e),
    }
}

fn cmd_schedule_resume(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return usage_error("usage: roamway schedule resume <id|name>"),
    };
    match request(flags, "POST", &format!("/v1/schedules/{}/resume", key), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule resume failed: {}", e),
    }
}

fn cmd_schedule_delete(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return usage_error("usage: roamway schedule delete <id|name>"),
    };
    match request(flags, "DELETE", &format!("/v1/schedules/{}", key), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule delete failed: {}", e),
    }
}

fn cmd_schedule_run_once(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return usage_error("usage: roamway schedule run-once <id|name>"),
    };
    match request(flags, "POST", &format!("/v1/schedules/{}/run", key), Some(json!({}))) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("schedule run-once failed: {}", e),
    }
}

fn cmd_schedule_update(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) => k,
        None => return usage_error("usage: roamway schedule update <id|name> [--cron|--every|--prompt|--max-runs|--tz|--enabled]"),
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
            usage_error("usage: roamway hub [init|permissions|projects|deploy|logout|status|token rotate]");
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
                usage_error("usage: roamway hub permissions grant <subject> <scope>");
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
                usage_error("usage: roamway hub permissions revoke <subject>");
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
            usage_error("usage: roamway hub permissions [ls|grant <subject> <scope>|revoke <subject>]");
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
                usage_error("usage: roamway hub projects add <path>");
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
                usage_error("usage: roamway hub projects rm <path>");
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
            usage_error("usage: roamway hub projects [ls|add <path>|rm <path>]");
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
        usage_error("usage: roamway hub token rotate");
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
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v);
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
            usage_error("usage: roamway clone <owner/repo | git url> [--dir DIR] [--protocol https|ssh] [--json] [--host H]");
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
        "create" | "add" => cmd_workspace_create(flags, &pos[1..]),
        "archive" => cmd_workspace_archive(flags, &pos[1..]),
        "setup" => cmd_workspace_setup(flags, &pos[1..]),
        "rm" | "remove" => cmd_workspace_rm(flags, &pos[1..]),
        _ => usage_error("usage: roamway workspace ls|create <name> --path DIR|archive <name>|setup <name>|rm <name>"),
    }
}

fn cmd_workspace_ls(flags: &HashMap<String, String>) {
    if let Some(h) = resolve_host(flags) {
        match hosts::dispatch(&h, "GET", "/v1/workspaces", None) {
            Ok((code, text)) if (200..300).contains(&code) => {
                if format_is_json(flags) {
                    println!("{}", text);
                } else if format_is_yaml(flags) {
                    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
                    print!("{}", to_yaml(&v));
                } else {
                    print_workspaces_table(&text);
                }
            }
            Ok((code, text)) => {
                eprintln!("workspace ls failed: HTTP {}: {}", code, text);
                fail(1);
            }
            Err(e) => {
                eprintln!("workspace ls failed: {}", e);
                fail(1);
            }
        }
        return;
    }
    let reg = workspaces::Registry::load(&data_dir_for(flags));
    let v = json!({"workspaces": reg.workspaces});
    if format_is_json(flags) || format_is_yaml(flags) {
        print_formatted(flags, &v);
        return;
    }
    if flags.contains_key("quiet") {
        for w in &reg.workspaces {
            println!("{}", w.id);
        }
        return;
    }
    if !flags.contains_key("no-headers") {
        println!(
            "{:<10} {:<16} {:<26} {:<8} {}",
            "ID", "NAME", "PATH", "HOST", "CREATED"
        );
    }
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

fn cmd_workspace_create(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) if !n.trim().is_empty() => n.clone(),
        _ => return usage_error("usage: roamway workspace create <name> --path DIR"),
    };
    let path = match flags.get("path").filter(|p| !p.trim().is_empty() && p.as_str() != "true") {
        Some(p) => p.clone(),
        None => return usage_error("--path is required"),
    };
    let body = json!({
        "name": name,
        "path": path,
        "remote": flags.get("remote"),
    });
    match request(flags, "POST", "/v1/workspaces", Some(body)) {
        Ok(v) => {
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v);
            } else {
                let ws = &v["workspace"];
                println!(
                    "created {} at {}",
                    ws["name"].as_str().unwrap_or(""),
                    ws["path"].as_str().unwrap_or("")
                );
            }
        }
        Err(e) => eprintln!("workspace create failed: {}", e),
    }
}

fn cmd_workspace_archive(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) if !n.trim().is_empty() => n.clone(),
        _ => return usage_error("usage: roamway workspace archive <name>"),
    };
    match request(flags, "DELETE", &format!("/v1/workspaces/{}", name), None) {
        Ok(_) => println!("archived {}", name),
        Err(e) => eprintln!("workspace archive failed: {}", e),
    }
}

fn cmd_workspace_setup(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) if !n.trim().is_empty() => n.clone(),
        _ => return usage_error("usage: roamway workspace setup <name>"),
    };
    let dir = data_dir_for(flags);
    let reg = workspaces::Registry::load(&dir);
    let ws = match reg.get(&name) {
        Some(w) => w.clone(),
        None => {
            eprintln!("no such workspace: {}", name);
            fail(2);
            return;
        }
    };
    let store = terminals::TerminalStore::load(&dir);
    let script = match store.scripts.iter().find(|s| s.name == name || s.id == name) {
        Some(s) => s.clone(),
        None => {
            eprintln!("no setup script for {}", name);
            fail(2);
            return;
        }
    };
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd.exe");
        c.args(["/Q", "/C", &script.cmd]);
        c
    } else {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &script.cmd]);
        c
    };
    cmd.current_dir(&ws.path);
    cmd.stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    match cmd.status() {
        Ok(st) if st.success() => {}
        Ok(st) => {
            eprintln!(
                "setup {} exited {}",
                name,
                st.code().map(|c| c.to_string()).unwrap_or_else(|| "-".into())
            );
            fail(1);
        }
        Err(e) => {
            eprintln!("setup {} failed: {}", name, e);
            fail(1);
        }
    }
}

fn cmd_workspace_rm(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) if !n.trim().is_empty() => n,
        _ => return usage_error("usage: roamway workspace rm <name>"),
    };
    if let Some(h) = resolve_host(flags) {
        let path = format!("/v1/workspaces/{}", name);
        match hosts::dispatch(&h, "DELETE", &path, None) {
            Ok((code, text)) if (200..300).contains(&code) => println!("{}", text),
            Ok((code, text)) => {
                eprintln!("workspace rm failed: HTTP {}: {}", code, text);
                fail(1);
            }
            Err(e) => {
                eprintln!("workspace rm failed: {}", e);
                fail(1);
            }
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

fn probe_run_status(flags: &HashMap<String, String>, id: &str) -> Option<String> {
    let (base, token) = if let Some(h) = resolve_host(flags) {
        (
            hosts::endpoint_normalize(&h.endpoint),
            if h.token.is_empty() { None } else { Some(h.token.clone()) },
        )
    } else {
        let c = local_client(flags);
        (c.base.clone(), c.token.clone())
    };
    let url = format!("{}{}", base.trim_end_matches('/'), format!("/v1/runs/{}", id));
    let mut r = ureq::get(&url).timeout(Duration::from_secs(10));
    if let Some(t) = &token {
        r = r.set("x-roamway-token", t);
    }
    match r.call() {
        Ok(resp) => {
            let v: Value = resp.into_json().ok()?;
            Some(v["run"]["status"].as_str().unwrap_or("").to_string())
        }
        Err(ureq::Error::Status(code, _)) if code == 404 => None,
        Err(_) => None,
    }
}

fn cmd_send(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) if !i.trim().is_empty() => i.clone(),
        _ => return usage_error("usage: roamway send <agent-id> <prompt...>"),
    };
    let prompt = pos[1..].join(" ");
    if prompt.trim().is_empty() {
        usage_error("prompt is required");
        return;
    }
    let body = json!({
        "prompt": prompt,
        "timeout_s": flags.get("timeout").and_then(|v| v.parse::<u64>().ok()),
    });
    match request(flags, "POST", &format!("/v1/agents/{}/run", id), Some(body)) {
        Ok(v) => {
            if flags.contains_key("quiet") {
                println!("{}", v["run"]["id"].as_str().unwrap_or(""));
            } else if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v);
            } else {
                println!("{}", v["run"]["id"].as_str().unwrap_or(""));
            }
        }
        Err(e) => eprintln!("send failed: {}", e),
    }
}

fn cmd_delete(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) if !i.trim().is_empty() => i.clone(),
        _ => return usage_error("usage: roamway delete <id> [--yes]"),
    };
    let running = probe_run_status(flags, &id)
        .map(|s| matches!(s.as_str(), "running" | "queued"))
        .unwrap_or(false);
    for (method, path) in delete_plan(&id, running) {
        if let Err(e) = request(flags, &method, &path, None) {
            eprintln!("delete failed: {}", e);
            return;
        }
    }
    if flags.contains_key("quiet") {
        println!("{}", id);
    } else {
        println!("deleted {}", id);
    }
}

fn cmd_permit(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("ls") {
        "ls" | "list" => cmd_approvals(flags),
        "approve" => {
            if pos.len() < 2 {
                usage_error("usage: roamway permit approve <id>");
            } else {
                cmd_approve(flags, &pos[1..]);
            }
        }
        "deny" => {
            if pos.len() < 2 {
                usage_error("usage: roamway permit deny <id>");
            } else {
                cmd_deny(flags, &pos[1..]);
            }
        }
        _ => usage_error("usage: roamway permit ls | approve <id> | deny <id>"),
    }
}

fn cmd_heartbeat(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("ls") {
        "ls" | "list" => cmd_heartbeat_ls(flags),
        "create" => cmd_heartbeat_create(flags, &pos[1..]),
        "update" => cmd_heartbeat_update(flags, &pos[1..]),
        "delete" | "rm" | "remove" => cmd_heartbeat_delete(flags, &pos[1..]),
        _ => usage_error("usage: roamway heartbeat ls|create|update|delete ..."),
    }
}

fn cmd_heartbeat_ls(flags: &HashMap<String, String>) {
    match request(flags, "GET", "/v1/schedules", None) {
        Ok(v) => {
            let agent = flags.get("agent").cloned().filter(|a| !a.is_empty() && a != "true");
            let mut out = v.clone();
            if let Some(ag) = &agent {
                let filtered: Vec<Value> = out
                    .get("schedules")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|s| s.get("agent").and_then(|a| a.as_str()) == Some(ag.as_str()))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                out["schedules"] = json!(filtered);
            }
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &out);
                return;
            }
            let empty = vec![];
            let scheds = out["schedules"].as_array().unwrap_or(&empty);
            if flags.contains_key("quiet") {
                for s in scheds {
                    println!("{}", s["id"].as_str().unwrap_or(""));
                }
                return;
            }
            if !flags.contains_key("no-headers") {
                println!(
                    "{:<13} {:<20} {:<26} {:<6} {:<26} {}",
                    "ID", "NAME", "NEXT", "RUNS", "LAST", "CRON"
                );
            }
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
        Err(e) => eprintln!("heartbeat ls failed: {}", e),
    }
}

fn cmd_heartbeat_create(flags: &HashMap<String, String>, pos: &[String]) {
    let name = flags.get("name").cloned().unwrap_or_default();
    if name.trim().is_empty() {
        usage_error("--name is required");
        return;
    }
    let agent = flags.get("agent").cloned().filter(|a| !a.is_empty() && a != "true").unwrap_or_default();
    if agent.is_empty() {
        usage_error("--agent is required");
        return;
    }
    let prompt = pos.join(" ");
    let prompt = if prompt.trim().is_empty() {
        flags.get("prompt").cloned().unwrap_or_default()
    } else {
        prompt
    };
    if prompt.trim().is_empty() {
        usage_error("prompt is required");
        return;
    }
    let cron = flags.get("cron").cloned().filter(|v| !v.is_empty() && v != "true");
    let every = flags.get("every").cloned().filter(|v| !v.is_empty() && v != "true");
    if cron.is_none() && every.is_none() {
        usage_error("--cron or --every is required");
        return;
    }
    let body = json!({
        "name": name,
        "prompt": prompt,
        "agent": agent,
        "model": flags.get("model"),
        "workspace": flags.get("workspace"),
        "cron": cron,
        "every": every,
        "timezone": flags.get("tz"),
        "max_runs": flags.get("max-runs").and_then(|v| v.parse::<u64>().ok()),
        "expires_in": flags.get("expires-in").and_then(|v| v.parse::<i64>().ok()),
        "run_now": flags.contains_key("run-now"),
        "overlap": flags.contains_key("overlap"),
    });
    match request(flags, "POST", "/v1/schedules", Some(body)) {
        Ok(v) => {
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v["schedule"]);
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
        Err(e) => eprintln!("heartbeat create failed: {}", e),
    }
}

fn cmd_heartbeat_update(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) if !k.trim().is_empty() => k.clone(),
        _ => return usage_error("usage: roamway heartbeat update <id> [--name --agent --cron|--every --prompt]"),
    };
    let mut body = json!({
        "name": flags.get("name"),
        "agent": flags.get("agent"),
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
        Ok(v) => {
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v["schedule"]);
            } else {
                println!("{}", serde_json::to_string_pretty(&v["schedule"]).unwrap_or_default());
            }
        }
        Err(e) => eprintln!("heartbeat update failed: {}", e),
    }
}

fn cmd_heartbeat_delete(flags: &HashMap<String, String>, pos: &[String]) {
    let key = match pos.first() {
        Some(k) if !k.trim().is_empty() => k.clone(),
        _ => return usage_error("usage: roamway heartbeat delete <id|name>"),
    };
    match request(flags, "DELETE", &format!("/v1/schedules/{}", key), None) {
        Ok(v) => println!("{}", serde_json::to_string(&v).unwrap_or_default()),
        Err(e) => eprintln!("heartbeat delete failed: {}", e),
    }
}

fn cmd_project(_flags: &HashMap<String, String>, _pos: &[String]) {
    eprintln!("project: not supported (no project model; use workspace ls)");
    fail(2);
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
                None => return usage_error("usage: roamway terminal capture <id> [--since N] [--tail N]"),
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
                None => return usage_error("usage: roamway terminal send-keys <id> <keys...> [--enter]"),
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
                None => return usage_error("usage: roamway terminal kill <id>"),
            };
            match c.req("DELETE", &format!("/v1/terminals/{}", id), None) {
                Ok(_) => println!("killed {}", id),
                Err(e) => eprintln!("terminal kill failed: {}", e),
            }
        }
        _ => usage_error(
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
                return usage_error("usage: roamway script add --name N --cmd \"...\" [--workspace DIR]");
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
                None => return usage_error("usage: roamway script rm <name>"),
            };
            match c.req("DELETE", &format!("/v1/scripts/{}", name), None) {
                Ok(_) => println!("removed {}", name),
                Err(e) => eprintln!("script rm failed: {}", e),
            }
        }
        "start" | "stop" => {
            let name = match pos.get(1).filter(|s| !s.trim().is_empty()) {
                Some(v) => v,
                None => return usage_error("usage: roamway script start|stop <name>"),
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
        _ => usage_error("usage: roamway script ls|add|rm|start|stop"),
    }
}

fn cmd_run_remote(h: &hosts::Host, flags: &HashMap<String, String>, pos: &[String], plan: &RunPlan) {
    let mut prompt = pos.join(" ");
    if prompt.trim().is_empty() {
        prompt = flags.get("prompt").cloned().unwrap_or_default();
    }
    if prompt.trim().is_empty() {
        usage_error("prompt required");
        return;
    }
    let body = run_body(plan, &prompt);
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let (code, text) = match hosts::dispatch(h, "POST", "/v1/agents", Some(&body_str)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("dispatch failed: {}", e);
            return;
        }
    };
    if !(200..300).contains(&code) {
        let msg = server_error_message(&text);
        if is_run_usage_error(&msg) {
            usage_error(&msg);
        } else {
            eprintln!("dispatch failed: HTTP {}: {}", code, text);
        }
        return;
    }
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    let id = v["run"]["id"].as_str().unwrap_or("").to_string();
    if flags.contains_key("no-wait") {
        println!("{}", id);
        return;
    }
    let mut fetch_status = |rid: &str| dispatch_json(h, "GET", &format!("/v1/runs/{}", rid));
    let mut fetch_log = |rid: &str| dispatch_json(h, "GET", &format!("/v1/runs/{}/log?tail=40", rid));
    wait_run(flags, &id, plan.wait_timeout, &mut fetch_status, &mut fetch_log);
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
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &json!({"runs": runs}));
            } else {
                print_runs_table(&runs);
            }
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
    if format_is_json(flags) || format_is_yaml(flags) {
        print_formatted(flags, &json!({"runs": all}));
    } else {
        print_runs_table(&all);
    }
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
        _ => usage_error("usage: roamway host add|ls|rm|enable|disable|probe ..."),
    }
}

fn cmd_host_add(flags: &HashMap<String, String>, pos: &[String]) {
    let name = match pos.first() {
        Some(n) => n,
        None => return usage_error("usage: roamway host add <name> --endpoint <ep> [--token <t>]"),
    };
    let endpoint = match flags.get("endpoint") {
        Some(e) => e,
        None => return usage_error("--endpoint is required"),
    };
    let env_name = format!("ROAMWAY_TOKEN_{}", name.to_uppercase().replace('-', "_"));
    let token = flags
        .get("token")
        .cloned()
        .or_else(|| std::env::var(&env_name).ok())
        .filter(|s| !s.is_empty());
    let token = match token {
        Some(t) => t,
        None => {
                eprintln!("token required: --token or environment {}", env_name);
                fail(2);
                return;
            }
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
        None => return usage_error("usage: roamway host rm <name>"),
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
        None => return usage_error("usage: roamway host enable|disable <name>"),
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
        None => return usage_error("usage: roamway host probe <name>"),
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
        _ => usage_error(
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
        usage_error("prompt required");
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
        None => return usage_error("usage: roamway inspect <id>"),
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
            let resumable = v["resumable"].as_bool().unwrap_or(false);
            println!("resumable:  {}", if resumable { "yes" } else { "no" });
            let session = a["session_id"].as_str().unwrap_or("");
            println!("session:    {}", if session.is_empty() { "-" } else { session });
            println!("state:      {}", a["state"].as_str().unwrap_or(""));
            println!("parent:     {}", a["parent"].as_str().unwrap_or(""));
            println!("history:    {} msgs", a["history"].as_array().map(|h| h.len()).unwrap_or(0));
            println!("created_at: {}", a["created_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()));
            println!("updated_at: {}", a["updated_at"].as_i64().map(iso).unwrap_or_else(|| "-".into()));
            println!("last_run:   {}", a["last_run_id"].as_str().unwrap_or(""));
            let last_error = a["last_error"].as_str().unwrap_or("");
            println!("last_error: {}", if last_error.is_empty() { "-" } else { last_error });
            let empty = vec![];
            let runs = v["runs"].as_array().unwrap_or(&empty);
            if let Some(run) = runs.first() {
                if let Some(wt) = run["worktree"].as_object() {
                    let path = wt["path"].as_str().unwrap_or("");
                    let branch = wt["branch"].as_str().unwrap_or("");
                    let base = wt["base"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| "HEAD".into());
                    println!("worktree:   {} ({}, base={})", path, branch, base);
                }
            }
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
        None => return usage_error("usage: roamway agent logs <id> [--tail 80]"),
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
        None => return usage_error("usage: roamway wait <id> [--timeout 60]"),
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
        None => return usage_error("usage: roamway archive <id>"),
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
        None => return usage_error("usage: roamway agent reload <id>"),
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
        None => return usage_error("usage: roamway agent update <id> [--title --model --workspace]"),
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
        None => return usage_error("usage: roamway agent open <id>"),
    };
    let base = if let Some(h) = resolve_host(flags) {
        h.endpoint.clone()
    } else {
        local_base(flags)
    };
    let url = format!("{}/ui/#/agents/{}", base.trim_end_matches('/'), id);
    println!("{}", url);
    if !flags.contains_key("no-browser") {
        open_browser(&url);
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = Command::new("cmd").args(["/C", "start", "", url]).status() {
            eprintln!("roamway: opening browser failed: {}", e);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = Command::new("open").arg(url).status() {
            eprintln!("roamway: opening browser failed: {}", e);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Err(e) = Command::new("xdg-open").arg(url).status() {
            eprintln!("roamway: opening browser failed: {}", e);
        }
    }
}

fn ui_target(flags: &HashMap<String, String>) -> &'static str {
    if flags.contains_key("board") {
        "/ui/board"
    } else {
        "/ui/"
    }
}

fn cmd_ui(flags: &HashMap<String, String>, pos: &[String]) {
    const UI_FLAGS: &[&str] = &[
        "print", "board", "host", "data", "home", "token", "format", "json", "quiet",
        "no-color", "no-headers", "bind",
    ];
    for key in flags.keys() {
        if !UI_FLAGS.contains(&key.as_str()) {
            usage_error(&format!("ui: unknown flag --{}", key));
        }
    }
    if !pos.is_empty() {
        usage_error("usage: roamway ui [--print] [--board]");
    }
    if FAIL_CODE.load(Ordering::SeqCst) != 0 {
        return;
    }
    let base = local_base(flags);
    let url = format!("{}{}", base.trim_end_matches('/'), ui_target(flags));
    if flags.contains_key("print") {
        println!("{}", url);
        return;
    }
    println!("UI: {}", url);
    match local_token(flags) {
        Some(_) => println!("token required: yes (browser login will prompt)"),
        None => println!("token required: no"),
    }
}

fn cmd_agent_detach(flags: &HashMap<String, String>, pos: &[String]) {
    let id = match pos.first() {
        Some(i) => i,
        None => return usage_error("usage: roamway agent detach <id>"),
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
        fail(2);
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
        None => return usage_error("usage: roamway attach <id>"),
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

fn cmd_daemon_run(flags: &HashMap<String, String>) {
    cmd_serve(flags);
}

fn cmd_daemon(flags: &HashMap<String, String>, pos: &[String]) {
    match pos.first().map(String::as_str).unwrap_or("help") {
        "status" => cmd_daemon_status(flags),
        "config" => cmd_daemon_config(flags, pos),
        "start" => cmd_daemon_start(flags),
        "run" => cmd_daemon_run(flags),
        "stop" => cmd_daemon_stop(flags),
        "restart" => cmd_daemon_restart(flags),
        "reload" => cmd_daemon_reload(flags),
        "pair" => cmd_daemon_pair(flags),
        "set-password" => cmd_daemon_set_password(flags),
        _ => usage_error("usage: roamway daemon status|config|start|stop|restart|reload|pair|set-password|run"),
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
    let config_file = data.join("config.json");
    let effective_bind = resolved_config(flags)
        .map(|c| c.bind)
        .unwrap_or_else(|_| "127.0.0.1:8796".to_string());
    let mut status = json!({
        "ok": false,
        "error": "not running",
        "version": "",
        "uptime_s": 0,
        "pid": Value::Null,
        "data_dir": data.to_string_lossy(),
        "config_file": config_file.to_string_lossy(),
        "effective_bind": effective_bind.clone(),
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
        println!("config {}  effective_bind {}", config_file.display(), effective_bind);
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
    println!("config {}  effective_bind {}", config_file.display(), effective_bind);
}

fn cmd_daemon_config(flags: &HashMap<String, String>, pos: &[String]) {
    let data = data_dir_for(flags);
    let sub = pos.get(1).map(String::as_str).unwrap_or("show");
    match sub {
        "get" => {
            let key = match pos.get(2) {
                Some(k) => k.as_str(),
                None => {
                    usage_error("usage: roamway daemon config get <key>");
                    return;
                }
            };
            if !config::config_key_allowed(key) {
                usage_error(&format!("unknown config key '{}'", key));
                return;
            }
            let cfg = match resolved_config(flags) {
                Ok(c) => c,
                Err(e) => {
                    usage_error(&e);
                    return;
                }
            };
            if format_is_json(flags) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "key": key,
                        "value": cfg.display_value(key),
                        "source": cfg.source_of(key),
                    }))
                    .unwrap_or_default()
                );
            } else {
                println!("{}", cfg.display_value(key));
            }
        }
        "set" => {
            let (key, value) = match (pos.get(2), pos.get(3)) {
                (Some(k), Some(v)) => (k.as_str(), v.as_str()),
                _ => {
                    usage_error("usage: roamway daemon config set <key> <value>");
                    return;
                }
            };
            let mut cfg = match config::load(&data) {
                Ok(c) => c,
                Err(e) => {
                    usage_error(&e);
                    return;
                }
            };
            if let Err(e) = config::apply_set(&mut cfg, key, value) {
                usage_error(&e);
                return;
            }
            if let Err(e) = config::save(&cfg, &data) {
                usage_error(&e);
                return;
            }
            println!("set {} = {}", key, value);
        }
        "unset" => {
            let key = match pos.get(2) {
                Some(k) => k.as_str(),
                None => {
                    usage_error("usage: roamway daemon config unset <key>");
                    return;
                }
            };
            let mut cfg = match config::load(&data) {
                Ok(c) => c,
                Err(e) => {
                    usage_error(&e);
                    return;
                }
            };
            if let Err(e) = config::apply_unset(&mut cfg, key) {
                usage_error(&e);
                return;
            }
            if let Err(e) = config::save(&cfg, &data) {
                usage_error(&e);
                return;
            }
            println!("unset {}", key);
        }
        _ => {
            let cfg = match resolved_config(flags) {
                Ok(c) => c,
                Err(e) => {
                    usage_error(&e);
                    return;
                }
            };
            if format_is_json(flags) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "config": cfg.config_json(),
                        "sources": cfg.sources_json(),
                    }))
                    .unwrap_or_default()
                );
            } else {
                for key in config::CONFIG_KEYS {
                    println!(
                        "{:<18}{:<18}{}",
                        key,
                        cfg.display_value(key),
                        cfg.source_of(key)
                    );
                }
            }
        }
    }
}

fn cmd_daemon_start(flags: &HashMap<String, String>) {
    let cfg = match resolved_config(flags) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            fail(2);
            return;
        }
    };
    let bind = cfg.bind.clone();
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
        .arg("--tz")
        .arg(cfg.tz_offset_hours.to_string())
        .arg("--max-concurrent")
        .arg(cfg.max_concurrent.to_string())
        .arg("--max-conn")
        .arg(cfg.max_conn.to_string());
    for host in &cfg.allow_hosts {
        cmd.arg("--allow-host").arg(host);
    }
    for origin in &cfg.allow_origins {
        cmd.arg("--allow-origin").arg(origin);
    }
    for scope in &cfg.require_approval {
        cmd.arg("--require-approval").arg(scope);
    }
    if let Some(s) = cfg.unix_socket.as_ref() {
        cmd.arg("--unix-socket").arg(s);
    }
    if let Some(s) = cfg.tls_cert.as_ref() {
        cmd.arg("--tls-cert").arg(s);
    }
    if let Some(s) = cfg.tls_key.as_ref() {
        cmd.arg("--tls-key").arg(s);
    }
    cmd.stdin(Stdio::null())
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
            println!(
                "daemon started pid={} bind={} config={}",
                pid,
                bind,
                data.join("config.json").display()
            );
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
        None => {
                eprintln!("token required: --token, ROAMWAY_TOKEN, or <data>/token");
                fail(2);
                return;
            }
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
                usage_error("usage: roamway token revoke <id>");
                return;
            }
            let mut store = auth::TokenStore::load(&data);
            if store.revoke(&id) {
                println!("revoked {}", id);
            } else {
                eprintln!("no such token: {}", id);
            }
        }
        _ => usage_error("usage: roamway token add|create --name N [--scope read|run|admin] [--ttl 24h] | ls|list | revoke <id>"),
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
        usage_error("usage: roamway approve <id>");
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
        usage_error("usage: roamway deny <id>");
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
            if format_is_json(flags) || format_is_yaml(flags) {
                print_formatted(flags, &v);
                return;
            }
            let arr = v.get("approvals").and_then(|a| a.as_array()).cloned().unwrap_or_default();
            if flags.contains_key("quiet") {
                for a in &arr {
                    println!("{}", a["id"].as_str().unwrap_or("-"));
                }
                return;
            }
            if arr.is_empty() {
                println!("no approvals");
                return;
            }
            if !flags.contains_key("no-headers") {
                println!(
                    "{:<34} {:<8} {:<6} {} {}",
                    "ID", "STATUS", "METHOD", "PATH", "TS"
                );
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
                    usage_error("usage: roamway provider diagnostic <id>");
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
        _ => usage_error("usage: roamway provider [ls|models [<id>] [--refresh]|diagnostic <id>]"),
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
                    usage_error("usage: invalid --since (expected 7d, 24h or ISO timestamp)");
                    fail(2);
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
                    usage_error("usage: roamway usage inspect <run_id>");
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
        _ => usage_error("usage: roamway usage [ls [--since 7d|24h|ISO] [--provider id]|inspect <run_id>] [--json]"),
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
                    usage_error("usage: roamway plugin init <name>");
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
                    usage_error("usage: roamway plugin logs <name>");
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
                    usage_error("usage: roamway plugin reload <name>");
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
                    usage_error("usage: roamway plugin enable|disable <name> [--trust]");
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
                    usage_error("usage: roamway plugin remove <name> [--keep-files]");
                    return;
                }
            };
            match plugins::remove(&dir, name, flags.contains_key("keep-files")) {
                Ok(()) => println!("plugin '{}' removed", name),
                Err(e) => eprintln!("plugin remove failed: {}", e),
            }
        }
        _ => usage_error("usage: roamway plugin ls|init|logs|reload|enable|disable|remove ..."),
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
                    usage_error("usage: roamway speech transcribe <file>");
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
                usage_error("usage: roamway speech say <text> [--out file]");
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
        _ => usage_error("usage: roamway speech [transcribe <file>|say <text> [--out file]]"),
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
            keep_runs: 500,
            keep_days: 0,
            max_log_bytes: 4_194_304,
            max_conn: 64,
            conns: AtomicUsize::new(0),
            req_total: AtomicU64::new(0),
            req_failed: AtomicU64::new(0),

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
    fn ui_target_switches_board_and_index() {
        let mut flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        assert_eq!(ui_target(&flags), "/ui/");
        flags.insert("board".to_string(), "true".to_string());
        assert_eq!(ui_target(&flags), "/ui/board");
    }

    #[test]
    fn serve_path_fallbacks_append_without_displacing_caller_entries() {
        // 调用方显式设置的目录必须保持优先，fallback 只追加在末尾且不重复。
        let got = path_with_fallbacks("/tmp/shim:/usr/bin", "/home/u");
        assert_eq!(
            got,
            "/tmp/shim:/usr/bin:/home/u/.npm-global/bin:/home/u/.local/bin:/usr/local/bin"
        );
        // 已在 PATH 里的 fallback 不重复添加；空段被丢掉。
        let again = path_with_fallbacks("/usr/local/bin::/opt/x", "/home/u");
        assert_eq!(
            again,
            "/usr/local/bin:/opt/x:/home/u/.npm-global/bin:/home/u/.local/bin"
        );
        // 空 PATH 也能得到完整 fallback。
        assert_eq!(
            path_with_fallbacks("", "/home/u"),
            "/home/u/.npm-global/bin:/home/u/.local/bin:/usr/local/bin"
        );
    }

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
    fn local_base_wildcard_bind_becomes_loopback() {
        let dir = std::env::temp_dir().join(format!("roamway-bind-wild-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("bind"), "0.0.0.0:8796\n").unwrap();
        let mut flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        flags.insert("data".to_string(), dir.to_string_lossy().to_string());
        assert_eq!(local_base(&flags), "http://127.0.0.1:8796");
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

    #[test]
    fn parse_flags_short_o_and_q() {
        let (flags, pos) = parse_flags(&["-o".to_string(), "json".to_string(), "ls".to_string()]);
        assert_eq!(flags.get("format").map(String::as_str), Some("json"));
        assert_eq!(pos, vec!["ls".to_string()]);
        let (flags2, pos2) = parse_flags(&["-q".to_string(), "ls".to_string()]);
        assert_eq!(flags2.get("quiet").map(String::as_str), Some("true"));
        assert_eq!(pos2, vec!["ls".to_string()]);
    }

    #[test]
    fn parse_args_unknown_short_flags_are_collected() {
        let p = parse_args(&["-xvf".to_string()]);
        assert_eq!(p.unknown, vec!["-xvf".to_string()]);
        assert!(p.pos.is_empty());

        let p = parse_args(&[
            "run".to_string(),
            "--agent".to_string(),
            "shell".to_string(),
            "-x".to_string(),
            "echo".to_string(),
        ]);
        assert_eq!(p.unknown, vec!["-x".to_string()]);
        assert_eq!(p.pos, vec!["run".to_string(), "echo".to_string()]);
    }

    #[test]
    fn parse_args_supports_dash_i_as_image_alias() {
        let p = parse_args(&[
            "run".to_string(),
            "-i".to_string(),
            "/tmp/x.png".to_string(),
            "echo".to_string(),
        ]);
        assert_eq!(p.flags.get("image").map(String::as_str), Some("/tmp/x.png"));
        assert_eq!(p.pos, vec!["run".to_string(), "echo".to_string()]);
        assert!(p.unknown.is_empty());
    }

    #[test]
    fn parse_args_passes_everything_after_double_dash_through() {
        let p = parse_args(&["run".to_string(), "--".to_string(), "-x".to_string(), "y".to_string()]);
        assert_eq!(p.pos, vec!["run".to_string(), "-x".to_string(), "y".to_string()]);
        assert!(p.unknown.is_empty());

        let (flags, pos) = parse_flags(&[
            "run".to_string(),
            "--agent".to_string(),
            "shell".to_string(),
            "echo -n hi".to_string(),
        ]);
        assert_eq!(pos, vec!["run".to_string(), "echo -n hi".to_string()]);
        assert!(!flags.contains_key("image"));
    }

    #[test]
    fn parse_args_keeps_bare_dash_as_positional() {
        let p = parse_args(&["-".to_string(), "-x".to_string()]);
        assert_eq!(p.pos, vec!["-".to_string()]);
        assert_eq!(p.unknown, vec!["-x".to_string()]);
    }

    #[test]
    fn json_flag_and_format_flag_match() {
        let mut f1: HashMap<String, String> = HashMap::new();
        f1.insert("json".to_string(), "true".to_string());
        let mut f2: HashMap<String, String> = HashMap::new();
        f2.insert("format".to_string(), "json".to_string());
        let v = json!({"runs": [{"id": "a", "status": "done"}]});
        assert!(format_is_json(&f1));
        assert!(format_is_json(&f2));
        assert_eq!(format_value(&f1, &v), format_value(&f2, &v));
    }

    #[test]
    fn yaml_serializer_emits_nested_maps_and_sequences() {
        let y = to_yaml(&json!({"a": {"b": [1, "x"]}}));
        assert!(y.contains("a:"));
        assert!(y.contains("b:"));
        assert!(y.contains("- 1"));
        assert!(y.contains("- x"));
    }

    #[test]
    fn dispatch_returns_2_for_unknown_short_flag() {
        assert_eq!(dispatch(&["-x".to_string()]), 2);
        assert_eq!(dispatch(&["run".to_string(), "-x".to_string(), "echo".to_string()]), 2);
    }

    #[test]
    fn dispatch_returns_2_for_dash_i_without_value() {
        assert_eq!(dispatch(&["-i".to_string()]), 2);
        assert_eq!(dispatch(&["run".to_string(), "-i".to_string()]), 2);
    }

    #[test]
    fn dispatch_returns_usage_exit_code() {
        assert_eq!(dispatch(&["workspace".to_string(), "setup".to_string()]), 2);
        assert_eq!(dispatch(&["roamway-bogus-command".to_string()]), 2);
    }

    #[test]
    fn prune_cli_rejects_bad_keep() {
        assert_eq!(
            dispatch(&["prune".to_string(), "--keep".to_string(), "-1".to_string()]),
            2
        );
        assert_eq!(
            dispatch(&["prune".to_string(), "--keep".to_string(), "abc".to_string()]),
            2
        );
        assert_eq!(
            dispatch(&["prune".to_string(), "--bogus".to_string()]),
            2
        );
    }

    #[test]
    fn resume_target_parses_agent_and_prompt_and_rejects_conflicts() {
        let mut flags = HashMap::new();
        flags.insert("resume".to_string(), "true".to_string());
        let pos = vec!["ag_1".to_string(), "do".to_string()];
        assert_eq!(
            resume_target(&flags, &pos).unwrap(),
            ("ag_1".to_string(), "do".to_string())
        );

        let no_prompt = vec!["ag_1".to_string()];
        assert_eq!(resume_target(&flags, &no_prompt).unwrap_err(), "prompt required");

        flags.insert("continue".to_string(), "true".to_string());
        assert!(resume_target(&flags, &pos).is_err());
    }

    #[test]
    fn delete_plan_stops_running_runs_before_delete() {
        assert_eq!(
            delete_plan("r1", true),
            vec![
                ("POST".to_string(), "/v1/runs/r1/stop".to_string()),
                ("DELETE".to_string(), "/v1/runs/r1".to_string()),
            ]
        );
        assert_eq!(
            delete_plan("r1", false),
            vec![("DELETE".to_string(), "/v1/runs/r1".to_string())]
        );
    }

    #[test]
    fn parse_labels_accepts_multi_and_rejects_missing_equals() {
        let m = parse_labels(&["a=1".to_string(), "b=2".to_string()]).unwrap();
        assert_eq!(m.get("a").map(String::as_str), Some("1"));
        assert_eq!(m.get("b").map(String::as_str), Some("2"));
        assert!(parse_labels(&["a".to_string()]).is_err());
        assert!(parse_labels(&["=x".to_string()]).is_err());
        let m2 = parse_labels(&["a=b=c".to_string()]).unwrap();
        assert_eq!(m2.get("a").map(String::as_str), Some("b=c"));
    }

    #[test]
    fn run_plan_worktree_mode_and_unsupported_validation() {
        let mut f1: HashMap<String, String> = HashMap::new();
        f1.insert("new-workspace".to_string(), "worktree".to_string());
        assert!(run_plan(&f1).unwrap().worktree);

        let mut f2: HashMap<String, String> = HashMap::new();
        f2.insert("worktree-mode".to_string(), "checkout-pr".to_string());
        assert!(run_plan(&f2).unwrap_err().contains("checkout-pr requires --pr-number"));

        let mut f3: HashMap<String, String> = HashMap::new();
        f3.insert("worktree-slug".to_string(), "slug".to_string());
        assert!(run_plan(&f3).unwrap_err().contains("requires --new-workspace worktree"));

        let mut f4: HashMap<String, String> = HashMap::new();
        f4.insert("env".to_string(), "1BAD=x".to_string());
        assert!(run_plan(&f4).unwrap_err().contains("invalid env key"));
    }

    #[test]
    fn run_plan_rejects_slug_without_worktree_workspace() {
        let mut f: HashMap<String, String> = HashMap::new();
        f.insert("worktree-slug".to_string(), "p16-a".to_string());
        assert_eq!(
            run_plan(&f).unwrap_err(),
            "run --worktree-slug requires --new-workspace worktree"
        );

        f.insert("new-workspace".to_string(), "worktree".to_string());
        let plan = run_plan(&f).unwrap();
        assert_eq!(plan.spec.slug.as_deref(), Some("p16-a"));
    }

    #[test]
    fn ls_plan_switches_runs_and_global_limit_defaults() {
        let mut f1: HashMap<String, String> = HashMap::new();
        f1.insert("runs".to_string(), "true".to_string());
        assert!(ls_plan(&f1).unwrap().runs);

        let mut f2: HashMap<String, String> = HashMap::new();
        f2.insert("global".to_string(), "true".to_string());
        f2.insert("host".to_string(), "nas".to_string());
        assert!(ls_plan(&f2).unwrap().global);

        let mut f3: HashMap<String, String> = HashMap::new();
        f3.insert("limit".to_string(), "abc".to_string());
        assert_eq!(ls_plan(&f3).unwrap().limit, 50);

        let mut f4: HashMap<String, String> = HashMap::new();
        f4.insert("limit".to_string(), "10000".to_string());
        assert_eq!(ls_plan(&f4).unwrap().limit, 500);
    }

    /// R1：`ls --label team` 曾被静默忽略（rc=0 忽略过滤器，返回全部 agent）
    #[test]
    fn ls_plan_rejects_label_without_equals() {
        let mut f: HashMap<String, String> = HashMap::new();
        f.insert("label".to_string(), "team".to_string());
        let err = ls_plan(&f).unwrap_err();
        assert!(err.contains("label must be k=v"), "unexpected: {}", err);

        let mut f2: HashMap<String, String> = HashMap::new();
        f2.insert("label".to_string(), "=x".to_string());
        assert!(ls_plan(&f2).unwrap_err().contains("label key must not be empty"));

        let mut f3: HashMap<String, String> = HashMap::new();
        f3.insert("label".to_string(), "team=p12".to_string());
        assert_eq!(ls_plan(&f3).unwrap().labels, vec!["team=p12".to_string()]);

        // 值里含 `=` 也合法，且原样保留（不重排、不去重）
        let mut f4: HashMap<String, String> = HashMap::new();
        f4.insert("label".to_string(), "team=a=b".to_string());
        assert_eq!(ls_plan(&f4).unwrap().labels, vec!["team=a=b".to_string()]);
    }

    #[test]
    fn agent_list_url_encodes_labels_and_order() {
        let plan = LsPlan {
            runs: false,
            global: false,
            all: true,
            labels: vec!["k=1".to_string(), "a=b".to_string()],
            limit: 10,
            kind: Some("codex".to_string()),
        };
        assert_eq!(
            agent_list_url(&plan),
            "/v1/agents?all=1&label=k%3D1&label=a%3Db&limit=10&kind=codex"
        );
    }

    #[test]
    fn parse_flags_short_a_and_g() {
        let (flags, _) = parse_flags(&["-a".to_string(), "-g".to_string(), "ls".to_string()]);
        assert_eq!(flags.get("all").map(String::as_str), Some("true"));
        assert_eq!(flags.get("global").map(String::as_str), Some("true"));
    }

    #[test]
    fn validate_run_options_rejects_unknown_key_and_bad_values() {
        let ok: BTreeMap<String, String> = BTreeMap::new();
        assert_eq!(validate_run_options(&ok).unwrap(), ok);

        let mut unknown: BTreeMap<String, String> = BTreeMap::new();
        unknown.insert("bogus".to_string(), "1".to_string());
        assert_eq!(
            validate_run_options(&unknown).unwrap_err(),
            "unknown run option: bogus"
        );

        let mut thinking: BTreeMap<String, String> = BTreeMap::new();
        thinking.insert("thinking".to_string(), "bogus".to_string());
        assert!(validate_run_options(&thinking).unwrap_err().contains("none|low|medium|high"));

        let mut mode: BTreeMap<String, String> = BTreeMap::new();
        mode.insert("mode".to_string(), "x".to_string());
        assert!(validate_run_options(&mode).unwrap_err().contains("read-only|workspace-write|danger-full-access"));

        let mut schema: BTreeMap<String, String> = BTreeMap::new();
        schema.insert("output_schema".to_string(), "/nope".to_string());
        assert!(validate_run_options(&schema).unwrap_err().contains("not a JSON object with type"));
    }

    #[test]
    fn run_plan_no_longer_rejects_provider_flags() {
        let mut f: HashMap<String, String> = HashMap::new();
        f.insert("thinking".to_string(), "high".to_string());
        f.insert("mode".to_string(), "read-only".to_string());
        f.insert("provider".to_string(), "ollama".to_string());
        let plan = run_plan(&f).unwrap();
        assert_eq!(plan.options.get("thinking").map(String::as_str), Some("high"));
        assert_eq!(plan.options.get("mode").map(String::as_str), Some("read-only"));
        assert_eq!(plan.options.get("provider").map(String::as_str), Some("ollama"));

        let mut w: HashMap<String, String> = HashMap::new();
        w.insert("worktree-slug".to_string(), "slug".to_string());
        assert!(run_plan(&w).unwrap_err().contains("requires --new-workspace worktree"));
    }

    #[test]
    fn serve_pid_file_written_and_cleared() {
        let dir = std::env::temp_dir().join(format!("roamway-serve-pid-{}", now()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_path = dir.join("roamway.pid");
        assert!(!pid_path.exists());
        let pid = crate::backup::write_pid_file(&dir).unwrap();
        assert_eq!(pid, std::process::id());
        assert!(pid_path.is_file());
        assert_eq!(crate::backup::pid_file_owner(&dir), Some(std::process::id()));
        crate::backup::clear_pid_file(&dir);
        assert!(!pid_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_restore_reject_remote_host_and_bad_flags() {
        FAIL_CODE.store(0, std::sync::atomic::Ordering::SeqCst);
        let host_flags = HashMap::from([("host".to_string(), "http://remote".to_string())]);
        cmd_backup(&host_flags, &[]);
        assert_eq!(
            FAIL_CODE.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "backup --host must be rejected with exit 2"
        );

        FAIL_CODE.store(0, std::sync::atomic::Ordering::SeqCst);
        cmd_restore(&host_flags, &[]);
        assert_eq!(
            FAIL_CODE.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "restore --host must be rejected with exit 2"
        );

        FAIL_CODE.store(0, std::sync::atomic::Ordering::SeqCst);
        let bad_keep = HashMap::from([("keep".to_string(), "-1".to_string())]);
        cmd_backup(&bad_keep, &[]);
        assert_eq!(
            FAIL_CODE.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "backup --keep -1 must be rejected with exit 2"
        );
        FAIL_CODE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}
