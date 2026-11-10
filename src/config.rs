/// P14 持久部署配置：<data_dir>/config.json 的加载、校验、原子写入，以及
/// CLI flag > ROAMWAY_* 环境变量 > config.json > 内置默认 的统一解析。
///
/// 安全约束：config.json 永远不落 token / password / secret / api_key 等密钥，
/// 只允许通过 `token_env` 指定「环境变量名」。发现禁用或未知键时直接报错。
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::types::Run;

/// config.json 允许出现的全部键，顺序即 CLI 表格 / JSON 的展示顺序。
pub const CONFIG_KEYS: &[&str] = &[
    "bind",
    "max_concurrent",
    "max_conn",
    "tz_offset_hours",
    "unix_socket",
    "tls_cert",
    "tls_key",
    "allow_hosts",
    "allow_origins",
    "require_approval",
    "token_env",
    "keep_runs",
    "max_log_bytes",
];

const FORBIDDEN_KEYS: &[&str] = &["token", "password", "secret", "api_key"];

/// 持久配置。全部字段可选：缺省时由代码里的内置默认值接管。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_conn: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz_offset_hours: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_hosts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_origins: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_approval: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_runs: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_log_bytes: Option<usize>,
}

/// 解析后的生效配置：每个 key 都已收敛为具体值，并记录来源。
#[derive(Clone, Debug, Default)]
pub struct EffectiveConfig {
    pub bind: String,
    pub max_concurrent: usize,
    pub max_conn: usize,
    pub tz_offset_hours: i64,
    pub unix_socket: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub allow_hosts: Vec<String>,
    pub allow_origins: Vec<String>,
    pub require_approval: Vec<String>,
    pub token_env: String,
    pub keep_runs: usize,
    pub max_log_bytes: usize,
    pub sources: BTreeMap<String, String>,
}

impl EffectiveConfig {
    pub fn require_approval_enabled(&self) -> bool {
        !self.require_approval.is_empty()
    }

    pub fn source_of(&self, key: &str) -> String {
        self.sources
            .get(key)
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    }

    pub fn config_json(&self) -> Value {
        json!({
            "bind": self.bind.clone(),
            "max_concurrent": self.max_concurrent,
            "max_conn": self.max_conn,
            "tz_offset_hours": self.tz_offset_hours,
            "unix_socket": self.unix_socket.clone(),
            "tls_cert": self.tls_cert.clone(),
            "tls_key": self.tls_key.clone(),
            "allow_hosts": self.allow_hosts.clone(),
            "allow_origins": self.allow_origins.clone(),
            "require_approval": self.require_approval_enabled(),
            "token_env": self.token_env.clone(),
            "keep_runs": self.keep_runs,
            "max_log_bytes": self.max_log_bytes,
        })
    }

    pub fn sources_json(&self) -> Value {
        serde_json::to_value(&self.sources).unwrap_or_else(|_| json!({}))
    }

    pub fn display_value(&self, key: &str) -> String {
        match key {
            "bind" => self.bind.clone(),
            "max_concurrent" => self.max_concurrent.to_string(),
            "max_conn" => self.max_conn.to_string(),
            "tz_offset_hours" => self.tz_offset_hours.to_string(),
            "unix_socket" => self.unix_socket.clone().unwrap_or_else(|| "-".to_string()),
            "tls_cert" => self.tls_cert.clone().unwrap_or_else(|| "-".to_string()),
            "tls_key" => self.tls_key.clone().unwrap_or_else(|| "-".to_string()),
            "allow_hosts" => self.allow_hosts.join(","),
            "allow_origins" => self.allow_origins.join(","),
            "require_approval" => self.require_approval_enabled().to_string(),
            "token_env" => self.token_env.clone(),
            "keep_runs" => self.keep_runs.to_string(),
            "max_log_bytes" => self.max_log_bytes.to_string(),
            _ => String::new(),
        }
    }
}

pub fn config_key_allowed(key: &str) -> bool {
    CONFIG_KEYS.contains(&key)
}

fn is_forbidden_key(key: &str) -> bool {
    FORBIDDEN_KEYS.iter().any(|f| key == *f || key.contains(f))
}

/// 从 `<dir>/config.json` 加载。文件缺失 / 空文件返回默认；语法错误或含
/// 禁用 / 未知键时返回 Err（调用方 stderr + exit 2）。
pub fn load(dir: &Path) -> Result<Config, String> {
    let path = dir.join("config.json");
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Ok(Config::default()),
    };
    if bytes.is_empty() {
        return Ok(Config::default());
    }
    let text = String::from_utf8_lossy(&bytes);
    let value: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            let line = e.line();
            return Err(format!("config.json: bad json at line {}", line));
        }
    };
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Err("config.json: expected a JSON object".to_string()),
    };
    for key in obj.keys() {
        if !config_key_allowed(key) {
            if is_forbidden_key(key) {
                return Err(format!("config.json contains forbidden key '{}'", key));
            }
            return Err(format!("config.json contains unknown key '{}'", key));
        }
    }
    serde_json::from_value::<Config>(value).map_err(|e| format!("config.json: invalid value: {}", e))
}

/// 原子写入 `<dir>/config.json`：tmp + fsync + rename；unix 上 0600。
pub fn save(cfg: &Config, dir: &Path) -> Result<(), String> {
    let _ = fs::create_dir_all(dir);
    let path = dir.join("config.json");
    let tmp = dir.join("config.json.tmp");
    let s = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let result = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(s.as_bytes())?;
        f.sync_all()?;
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    })();
    result.map_err(|e| format!("config.json: save failed: {}", e))
}

fn invalid(key: &str, value: &str) -> String {
    format!("{}: invalid value '{}'", key, value)
}

fn valid_bind(bind: &str) -> bool {
    let b = bind.trim();
    if b.is_empty() {
        return false;
    }
    if let Some(rest) = b.strip_prefix('[') {
        if let Some((host, port)) = rest.split_once("]:") {
            return !host.is_empty() && port.parse::<u16>().is_ok();
        }
        return false;
    }
    // 裸 IPv6（多于一个冒号且没有端口）不支持。
    if b.matches(':').count() > 1 {
        return false;
    }
    match b.rsplit_once(':') {
        Some((host, port)) => !host.is_empty() && port.parse::<u16>().is_ok(),
        None => false,
    }
}

fn parse_usize_positive(key: &str, value: &str) -> Result<usize, String> {
    let v: usize = value
        .trim()
        .parse()
        .map_err(|_| invalid(key, value.trim()))?;
    if v == 0 {
        return Err(invalid(key, value.trim()));
    }
    Ok(v)
}

fn parse_i64(key: &str, value: &str) -> Result<i64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| invalid(key, value.trim()))
}

fn parse_bool(key: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(invalid(key, value.trim())),
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn bool_scopes(enabled: bool) -> Vec<String> {
    if enabled {
        vec!["run".to_string(), "admin".to_string()]
    } else {
        Vec::new()
    }
}

/// `daemon config set <key> <value>`：校验合法后原地修改 cfg。失败不改动 cfg。
pub fn apply_set(cfg: &mut Config, key: &str, value: &str) -> Result<(), String> {
    if !config_key_allowed(key) {
        return Err(format!("unknown config key '{}'", key));
    }
    let trimmed = value.trim();
    match key {
        "bind" => {
            if !valid_bind(trimmed) {
                return Err(invalid(key, trimmed));
            }
            cfg.bind = Some(trimmed.to_string());
        }
        "max_concurrent" => cfg.max_concurrent = Some(parse_usize_positive(key, value)?),
        "max_conn" => cfg.max_conn = Some(parse_usize_positive(key, value)?),
        "tz_offset_hours" => cfg.tz_offset_hours = Some(parse_i64(key, value)?),
        "unix_socket" | "tls_cert" | "tls_key" | "token_env" => {
            if trimmed.is_empty() {
                return Err(invalid(key, trimmed));
            }
            let v = trimmed.to_string();
            match key {
                "unix_socket" => cfg.unix_socket = Some(v),
                "tls_cert" => cfg.tls_cert = Some(v),
                "tls_key" => cfg.tls_key = Some(v),
                _ => cfg.token_env = Some(v),
            }
        }
        "allow_hosts" => cfg.allow_hosts = Some(split_list(value)),
        "allow_origins" => cfg.allow_origins = Some(split_list(value)),
        "require_approval" => cfg.require_approval = Some(parse_bool(key, value)?),
        "keep_runs" => cfg.keep_runs = Some(
            value
                .trim()
                .parse::<usize>()
                .map_err(|_| invalid(key, value.trim()))?,
        ),
        "max_log_bytes" => cfg.max_log_bytes = Some(
            value
                .trim()
                .parse::<usize>()
                .map_err(|_| invalid(key, value.trim()))?,
        ),
        _ => return Err(format!("unknown config key '{}'", key)),
    }
    Ok(())
}

/// `daemon config unset <key>`：把该键从文件里清掉，回到 env / 默认值。
pub fn apply_unset(cfg: &mut Config, key: &str) -> Result<(), String> {
    if !config_key_allowed(key) {
        return Err(format!("unknown config key '{}'", key));
    }
    match key {
        "bind" => cfg.bind = None,
        "max_concurrent" => cfg.max_concurrent = None,
        "max_conn" => cfg.max_conn = None,
        "tz_offset_hours" => cfg.tz_offset_hours = None,
        "unix_socket" => cfg.unix_socket = None,
        "tls_cert" => cfg.tls_cert = None,
        "tls_key" => cfg.tls_key = None,
        "allow_hosts" => cfg.allow_hosts = None,
        "allow_origins" => cfg.allow_origins = None,
        "require_approval" => cfg.require_approval = None,
        "token_env" => cfg.token_env = None,
        "keep_runs" => cfg.keep_runs = None,
        "max_log_bytes" => cfg.max_log_bytes = None,
        _ => return Err(format!("unknown config key '{}'", key)),
    }
    Ok(())
}

/// 把 flags 里同一 key 的多值（以 \u{1f} 分隔）拆成 Vec。
fn flag_values(flags: &HashMap<String, String>, key: &str) -> Vec<String> {
    flags
        .get(key)
        .map(|v| {
            v.split('\u{1f}')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn non_empty(v: &str) -> bool {
    !v.trim().is_empty()
}

/// 统一解析：CLI flag > ROAMWAY_* 环境变量 > config.json > 内置默认。
pub fn resolve_config(
    flags: &HashMap<String, String>,
    env: &HashMap<String, String>,
    file: &Config,
) -> Result<EffectiveConfig, String> {
    let mut sources = BTreeMap::new();
    let mut c = EffectiveConfig::default();

    // bind
    if let Some(v) = flags.get("bind") {
        c.bind = check_bind(v)?;
        sources.insert("bind".into(), "flag".into());
    } else if let Some(v) = env.get("ROAMWAY_BIND").filter(|v| non_empty(v)) {
        c.bind = check_bind(v)?;
        sources.insert("bind".into(), "env".into());
    } else if let Some(v) = file.bind.as_deref().filter(|v| non_empty(v)) {
        c.bind = check_bind(v)?;
        sources.insert("bind".into(), "file".into());
    } else {
        c.bind = "127.0.0.1:8796".into();
        sources.insert("bind".into(), "default".into());
    }

    // max_concurrent
    if let Some(v) = flags.get("max-concurrent") {
        c.max_concurrent = parse_usize_positive("max_concurrent", v)?;
        sources.insert("max_concurrent".into(), "flag".into());
    } else if let Some(v) = env.get("ROAMWAY_MAX_CONCURRENT").filter(|v| non_empty(v)) {
        c.max_concurrent = parse_usize_positive("max_concurrent", v)?;
        sources.insert("max_concurrent".into(), "env".into());
    } else if let Some(v) = file.max_concurrent {
        if v == 0 {
            return Err(invalid("max_concurrent", "0"));
        }
        c.max_concurrent = v;
        sources.insert("max_concurrent".into(), "file".into());
    } else {
        c.max_concurrent = 4;
        sources.insert("max_concurrent".into(), "default".into());
    }

    // max_conn
    if let Some(v) = flags.get("max-conn") {
        c.max_conn = parse_usize_positive("max_conn", v)?;
        sources.insert("max_conn".into(), "flag".into());
    } else if let Some(v) = env.get("ROAMWAY_MAX_CONN").filter(|v| non_empty(v)) {
        c.max_conn = parse_usize_positive("max_conn", v)?;
        sources.insert("max_conn".into(), "env".into());
    } else if let Some(v) = file.max_conn {
        if v == 0 {
            return Err(invalid("max_conn", "0"));
        }
        c.max_conn = v;
        sources.insert("max_conn".into(), "file".into());
    } else {
        c.max_conn = 64;
        sources.insert("max_conn".into(), "default".into());
    }

    // tz_offset_hours
    if let Some(v) = flags.get("tz") {
        c.tz_offset_hours = parse_i64("tz_offset_hours", v)?;
        sources.insert("tz_offset_hours".into(), "flag".into());
    } else if let Some(v) = env.get("ROAMWAY_TZ").filter(|v| non_empty(v)) {
        c.tz_offset_hours = parse_i64("tz_offset_hours", v)?;
        sources.insert("tz_offset_hours".into(), "env".into());
    } else if let Some(v) = file.tz_offset_hours {
        c.tz_offset_hours = v;
        sources.insert("tz_offset_hours".into(), "file".into());
    } else {
        c.tz_offset_hours = 8;
        sources.insert("tz_offset_hours".into(), "default".into());
    }

    // unix_socket / tls_cert / tls_key（无环境变量）
    resolve_optional_string(
        flags,
        file.unix_socket.as_deref(),
        "unix-socket",
        "unix_socket",
        &mut c.unix_socket,
        &mut sources,
    );
    resolve_optional_string(
        flags,
        file.tls_cert.as_deref(),
        "tls-cert",
        "tls_cert",
        &mut c.tls_cert,
        &mut sources,
    );
    resolve_optional_string(
        flags,
        file.tls_key.as_deref(),
        "tls-key",
        "tls_key",
        &mut c.tls_key,
        &mut sources,
    );

    // allow_hosts（flag 多次 / env 逗号分隔 / file 数组）
    let flag_hosts = flag_values(flags, "allow-host");
    if !flag_hosts.is_empty() {
        c.allow_hosts = flag_hosts;
        sources.insert("allow_hosts".into(), "flag".into());
    } else if let Some(v) = env.get("ROAMWAY_ALLOW_HOSTS") {
        c.allow_hosts = split_list(v);
        sources.insert("allow_hosts".into(), "env".into());
    } else if let Some(v) = file.allow_hosts.clone() {
        c.allow_hosts = v;
        sources.insert("allow_hosts".into(), "file".into());
    } else {
        c.allow_hosts = Vec::new();
        sources.insert("allow_hosts".into(), "default".into());
    }

    // allow_origins（无环境变量）
    let flag_origins = flag_values(flags, "allow-origin");
    if !flag_origins.is_empty() {
        c.allow_origins = flag_origins;
        sources.insert("allow_origins".into(), "flag".into());
    } else if let Some(v) = file.allow_origins.clone() {
        c.allow_origins = v;
        sources.insert("allow_origins".into(), "file".into());
    } else {
        c.allow_origins = Vec::new();
        sources.insert("allow_origins".into(), "default".into());
    }

    // require_approval：flag 为 scope 列表（兼容 P6），env/file 为布尔。
    let flag_scopes = flag_values(flags, "require-approval");
    if !flag_scopes.is_empty() {
        c.require_approval = flag_scopes;
        sources.insert("require_approval".into(), "flag".into());
    } else if let Some(v) = env.get("ROAMWAY_REQUIRE_APPROVAL").filter(|v| non_empty(v)) {
        c.require_approval = bool_scopes(parse_bool("require_approval", v)?);
        sources.insert("require_approval".into(), "env".into());
    } else if let Some(b) = file.require_approval {
        c.require_approval = bool_scopes(b);
        sources.insert("require_approval".into(), "file".into());
    } else {
        c.require_approval = Vec::new();
        sources.insert("require_approval".into(), "default".into());
    }

    // token_env（无 flag / env）
    if let Some(v) = file.token_env.as_deref().filter(|v| non_empty(v)) {
        c.token_env = v.trim().to_string();
        sources.insert("token_env".into(), "file".into());
    } else {
        c.token_env = "ROAMWAY_TOKEN".into();
        sources.insert("token_env".into(), "default".into());
    }

    // keep_runs（无 flag / env）
    if let Some(v) = file.keep_runs {
        c.keep_runs = v;
        sources.insert("keep_runs".into(), "file".into());
    } else {
        c.keep_runs = 500;
        sources.insert("keep_runs".into(), "default".into());
    }

    // max_log_bytes（无 flag / env）：0 表示不限。
    if let Some(v) = file.max_log_bytes {
        c.max_log_bytes = v;
        sources.insert("max_log_bytes".into(), "file".into());
    } else {
        c.max_log_bytes = 4_194_304;
        sources.insert("max_log_bytes".into(), "default".into());
    }

    c.sources = sources;
    Ok(c)
}

fn check_bind(v: &str) -> Result<String, String> {
    let v = v.trim();
    if valid_bind(v) {
        Ok(v.to_string())
    } else {
        Err(invalid("bind", v))
    }
}

fn resolve_optional_string(
    flags: &HashMap<String, String>,
    file_value: Option<&str>,
    flag_name: &str,
    key: &str,
    out: &mut Option<String>,
    sources: &mut BTreeMap<String, String>,
) {
    if let Some(v) = flags.get(flag_name).filter(|v| non_empty(v)) {
        *out = Some(v.trim().to_string());
        sources.insert(key.to_string(), "flag".to_string());
    } else if let Some(v) = file_value.filter(|v| non_empty(v)) {
        *out = Some(v.trim().to_string());
        sources.insert(key.to_string(), "file".to_string());
    } else {
        *out = None;
        sources.insert(key.to_string(), "default".to_string());
    }
}

/// 计算本轮需要删除的 terminal run id（只删最旧的 terminal；running/queued 永久保留）。
/// keep_runs=0 或未超出阈值时返回空列表。这是保留策略的唯一真源：
/// 启动期 trim_runs 与运行期 jobs::prune 都走这里，不存在第二套阈值。
pub fn trim_runs_plan(runs: &[Run], keep_runs: usize) -> Vec<String> {
    if keep_runs == 0 || runs.len() <= keep_runs {
        return Vec::new();
    }
    let mut terminal: Vec<usize> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status.terminal())
        .map(|(i, _)| i)
        .collect();
    if terminal.is_empty() {
        return Vec::new();
    }
    let excess = runs.len() - keep_runs;
    let drop_count = excess.min(terminal.len());
    if drop_count == 0 {
        return Vec::new();
    }
    terminal.sort_by(|&a, &b| runs[a].created_at.cmp(&runs[b].created_at));
    terminal.truncate(drop_count);
    terminal.iter().map(|&i| runs[i].id.clone()).collect()
}

/// 修剪历史 run：keep_runs=0 不修剪；只删 terminal（done/failed/...）记录，
/// 绝不误删 running/queued。把总数压到 keep_runs 以内时，优先删除最旧的 terminal。
pub fn trim_runs(runs: &mut Vec<Run>, keep_runs: usize) -> bool {
    let removed = trim_runs_plan(runs, keep_runs);
    if removed.is_empty() {
        return false;
    }
    let removed: std::collections::HashSet<String> = removed.into_iter().collect();
    let before = runs.len();
    runs.retain(|r| !removed.contains(&r.id));
    runs.len() < before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RunStatus;

    fn run(id: &str, status: RunStatus, created_at: i64) -> Run {
        Run {
            id: id.to_string(),
            title: String::new(),
            agent: "shell".to_string(),
            model: None,
            workspace: String::new(),
            prompt: "echo hi".to_string(),
            status,
            created_at,
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
        }
    }

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolve_priority_flag_over_env_over_file_over_default() {
        let file = Config::default();
        let flags = HashMap::from([("bind".to_string(), "127.0.0.1:8797".to_string())]);
        let env = env_map(&[("ROAMWAY_BIND", "127.0.0.1:8798")]);
        let c = resolve_config(&flags, &env, &file).unwrap();
        assert_eq!(c.bind, "127.0.0.1:8797");
        assert_eq!(c.source_of("bind"), "flag");

        let flags = HashMap::new();
        let env = env_map(&[
            ("ROAMWAY_MAX_CONCURRENT", "9"),
            ("ROAMWAY_BIND", "127.0.0.1:8798"),
        ]);
        let mut file = Config::default();
        file.max_concurrent = Some(5);
        let c = resolve_config(&flags, &env, &file).unwrap();
        assert_eq!(c.max_concurrent, 9);
        assert_eq!(c.source_of("max_concurrent"), "env");

        let flags = HashMap::new();
        let env = HashMap::new();
        let mut file = Config::default();
        file.tz_offset_hours = Some(7);
        let c = resolve_config(&flags, &env, &file).unwrap();
        assert_eq!(c.tz_offset_hours, 7);
        assert_eq!(c.source_of("tz_offset_hours"), "file");

        let flags = HashMap::new();
        let env = HashMap::new();
        let c = resolve_config(&flags, &env, &Config::default()).unwrap();
        assert_eq!(c.max_conn, 64);
        assert_eq!(c.source_of("max_conn"), "default");
    }

    #[test]
    fn resolve_env_comma_array_and_invalid_values() {
        let env = env_map(&[("ROAMWAY_ALLOW_HOSTS", "a.example, b.example")]);
        let c = resolve_config(&HashMap::new(), &env, &Config::default()).unwrap();
        assert_eq!(c.allow_hosts, vec!["a.example".to_string(), "b.example".to_string()]);
        assert_eq!(c.source_of("allow_hosts"), "env");

        let flags = HashMap::from([("max-concurrent".to_string(), "0".to_string())]);
        assert!(resolve_config(&flags, &HashMap::new(), &Config::default()).is_err());

        let flags = HashMap::from([("bind".to_string(), "127.0.0.1".to_string())]);
        assert!(resolve_config(&flags, &HashMap::new(), &Config::default()).is_err());

        let flags = HashMap::from([("tz".to_string(), "abc".to_string())]);
        assert!(resolve_config(&flags, &HashMap::new(), &Config::default()).is_err());
    }

    #[test]
    fn config_roundtrip_and_no_token_leak() {
        let dir = std::env::temp_dir().join(format!("roamway-cfg-{}", crate::types::now()));
        let mut cfg = Config::default();
        cfg.bind = Some("127.0.0.1:8899".to_string());
        cfg.max_concurrent = Some(8);
        cfg.max_conn = Some(32);
        cfg.tz_offset_hours = Some(9);
        cfg.allow_hosts = Some(vec!["a".to_string(), "b".to_string()]);
        cfg.allow_origins = Some(vec!["https://x".to_string()]);
        cfg.require_approval = Some(true);
        cfg.token_env = Some("ROAMWAY_TOKEN".to_string());
        cfg.keep_runs = Some(12);
        save(&cfg, &dir).unwrap();
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.bind, cfg.bind);
        assert_eq!(loaded.max_concurrent, cfg.max_concurrent);
        assert_eq!(loaded.max_conn, cfg.max_conn);
        assert_eq!(loaded.tz_offset_hours, cfg.tz_offset_hours);
        assert_eq!(loaded.allow_hosts, cfg.allow_hosts);
        assert_eq!(loaded.allow_origins, cfg.allow_origins);
        assert_eq!(loaded.require_approval, cfg.require_approval);
        assert_eq!(loaded.token_env, cfg.token_env);
        assert_eq!(loaded.keep_runs, cfg.keep_runs);
        let raw = fs::read_to_string(dir.join("config.json")).unwrap();
        assert!(!raw.contains("\"token\""));
        assert!(raw.contains("\"token_env\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_load_bad_json_reports_line() {
        let dir = std::env::temp_dir().join(format!("roamway-badcfg-{}", crate::types::now()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("config.json"), "{ bad\n").unwrap();
        let e = load(&dir).unwrap_err();
        assert!(e.contains("bad json at line"), "{}", e);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_rejects_forbidden_and_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("roamway-forkey-{}", crate::types::now()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("config.json"), r#"{"token": "abc"}"#).unwrap();
        let e = load(&dir).unwrap_err();
        assert!(e.contains("forbidden key 'token'"), "{}", e);

        fs::write(dir.join("config.json"), r#"{"foo": 1}"#).unwrap();
        let e = load(&dir).unwrap_err();
        assert!(e.contains("unknown key 'foo'"), "{}", e);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_key_whitelist() {
        for key in CONFIG_KEYS {
            assert!(config_key_allowed(key), "{}", key);
        }
        for bad in ["token", "password", "secret", "api_key", "foo"] {
            assert!(!config_key_allowed(bad), "{}", bad);
        }
    }

    #[test]
    fn apply_set_validates_and_applies() {
        let mut cfg = Config::default();
        assert!(apply_set(&mut cfg, "max_concurrent", "0").is_err());
        assert!(cfg.max_concurrent.is_none());
        apply_set(&mut cfg, "tz_offset_hours", "9").unwrap();
        assert_eq!(cfg.tz_offset_hours, Some(9));
        apply_set(&mut cfg, "allow_hosts", "a,b").unwrap();
        assert_eq!(cfg.allow_hosts, Some(vec!["a".to_string(), "b".to_string()]));
        apply_set(&mut cfg, "require_approval", "true").unwrap();
        assert_eq!(cfg.require_approval, Some(true));
        apply_set(&mut cfg, "bind", "127.0.0.1:8899").unwrap();
        assert_eq!(cfg.bind.as_deref(), Some("127.0.0.1:8899"));
    }

    #[test]
    fn trim_runs_keeps_newest_and_never_drops_running() {
        let mut runs = vec![
            run("old", RunStatus::Done, 1),
            run("running", RunStatus::Running, 2),
            run("new", RunStatus::Done, 3),
        ];
        assert!(trim_runs(&mut runs, 2));
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().any(|r| r.id == "running"));
        assert!(runs.iter().any(|r| r.id == "new"));
        assert!(!runs.iter().any(|r| r.id == "old"));
    }

    #[test]
    fn config_keys_include_max_log_bytes() {
        assert!(CONFIG_KEYS.contains(&"max_log_bytes"));
    }

    #[test]
    fn max_log_bytes_defaults_to_4mib() {
        let c = resolve_config(&HashMap::new(), &HashMap::new(), &Config::default()).unwrap();
        assert_eq!(c.max_log_bytes, 4_194_304);
        assert_eq!(c.source_of("max_log_bytes"), "default");
    }

    #[test]
    fn max_log_bytes_set_validates_values() {
        let mut cfg = Config::default();
        apply_set(&mut cfg, "max_log_bytes", "0").unwrap();
        assert_eq!(cfg.max_log_bytes, Some(0));
        apply_set(&mut cfg, "max_log_bytes", "8388608").unwrap();
        assert_eq!(cfg.max_log_bytes, Some(8_388_608));
        assert!(apply_set(&mut cfg, "max_log_bytes", "-1").is_err());
        assert!(apply_set(&mut cfg, "max_log_bytes", "lots").is_err());
    }
}
