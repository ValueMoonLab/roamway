use crate::auth;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agents::Agent;
use crate::workspaces::WorktreeInfo;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn iso(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "08:30" -> (8, 30)
pub fn parse_hhmm(s: &str) -> Option<(i64, i64)> {
    let (h, m) = s.split_once(':')?;
    let h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    Some((h, m))
}

/// 允许通过 HTTP 提交的 agent 白名单，其余一律拒绝（防止任意命令执行）
pub const KNOWN_AGENTS: &[&str] = &[
    "shell",
    "codex",
    "hermes",
    "claude",
    "api:deepseek",
    "api:qwen",
];

pub fn is_known_agent(agent: &str) -> bool {
    KNOWN_AGENTS.contains(&agent.trim())
}

/// `--env` 的 key 必须是 `[A-Za-z_][A-Za-z0-9_]*`（ASCII 标识符，避免 shell 注入）
pub fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 下一次「本地钟点」的时间戳（用固定时区偏移，不依赖系统 tz 数据）
pub fn next_daily(after: i64, hhmm: &str, tz_offset_hours: i64) -> Option<i64> {
    let (h, m) = parse_hhmm(hhmm)?;
    let off = tz_offset_hours * 3600;
    let local = after + off;
    let day_start = local.div_euclid(86400) * 86400;
    let mut at = day_start + h * 3600 + m * 60 - off;
    if at <= after {
        at += 86400;
    }
    Some(at)
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Done,
    Failed,
    Timeout,
    Killed,
    Interrupted,
}

impl RunStatus {
    pub fn terminal(self) -> bool {
        !matches!(self, RunStatus::Queued | RunStatus::Running)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum RunUsage {
    Data(Usage),
    Note(String),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Run {
    pub id: String,
    pub title: String,
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    pub workspace: String,
    pub prompt: String,
    pub status: RunStatus,
    pub created_at: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub resumed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_note: Option<String>,
    pub timeout_s: u64,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<RunUsage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub id: String,
    pub title: String,
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    pub workspace: String,
    pub prompt: String,
    #[serde(default)]
    pub every_secs: Option<u64>,
    #[serde(default)]
    pub daily: Option<String>,
    #[serde(default = "default_tz")]
    pub tz_offset_hours: i64,
    pub timeout_s: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow_overlap: bool,
    pub next_at: i64,
    #[serde(default)]
    pub last_run: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    /// 前置任务 id：只有它们各自最近一次运行成功（done）时才允许触发本任务
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// 最近一次因依赖未满足而跳过的原因；成功触发时清空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_skip: Option<String>,
}

fn default_tz() -> i64 {
    8
}
fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct State {
    #[serde(default)]
    pub runs: Vec<Run>,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreHealth {
    Fresh,
    Ok,
    Recovered,
    Unrecoverable,
}

impl StoreHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreHealth::Fresh => "fresh",
            StoreHealth::Ok => "ok",
            StoreHealth::Recovered => "recovered",
            StoreHealth::Unrecoverable => "unrecoverable",
        }
    }
}
pub struct Store {
    pub dir: PathBuf,
    pub state: State,
    pub loaded_ok: bool,
    pub store_health: StoreHealth,
    pub preserve_backup: bool,
    /// 上次成功写入 state.json 的内容 FNV-1a 64 哈希；0 表示尚无写入记录。
    pub last_write_hash: u64,
    /// 成功写入 state.json 的次数（内存态，不序列化到 state.json）。
    pub writes_total: u64,
    /// 因内容与上次完全相同而跳过的写入次数（内存态，不序列化）。
    pub writes_skipped: u64,
}

/// prompt 持久化上限：超过则截断并追加标记，防止 state.json 无界膨胀。
/// 运行期内存中的 `Run.prompt` 仍保留全文，不影响执行；仅落盘时截断。
const PROMPT_PERSIST_LIMIT: usize = 8192;

fn truncate_prompt(p: &str) -> String {
    if p.len() <= PROMPT_PERSIST_LIMIT {
        return p.to_string();
    }
    let dropped = p.len() - PROMPT_PERSIST_LIMIT;
    let marker = format!(" ...[truncated {} bytes]", dropped);
    let mut cut = PROMPT_PERSIST_LIMIT.saturating_sub(marker.len());
    while cut > 0 && !p.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = String::with_capacity(PROMPT_PERSIST_LIMIT);
    out.push_str(&p[..cut]);
    out.push_str(&marker);
    out
}

impl Store {
    pub fn load(dir: &Path) -> Store {
        let _ = fs::create_dir_all(dir);
        let _ = fs::create_dir_all(dir.join("logs"));
        let p = dir.join("state.json");
        let bak = dir.join("state.json.bak");
        let (mut state, store_health) = load_state(&p, &bak);
        let loaded_ok = store_health != StoreHealth::Unrecoverable;
        let preserve_backup = store_health != StoreHealth::Ok;
        // 重启后，上次遗留的 queued/running 一律标记 interrupted，绝不自动重放
        let mut dirty = false;
        for r in state.runs.iter_mut() {
            if !r.status.terminal() {
                // 回收上次遗留的子进程（尽量先校验，降低 PID 复用误杀）
                if let Some(pid) = r.pid {
                    if crate::runner::pid_is_our_run(pid, r.started_at) {
                        crate::runner::kill_process_group(pid);
                    }
                }
                r.status = RunStatus::Interrupted;
                r.ended_at = Some(now());
                r.error = Some("daemon restarted while running".into());
                dirty = true;
            }
        }
        // 重启后，若 agent 仍标 running/queued 但对应 run 已不在活动态，修正为 error
        for a in state.agents.iter_mut() {
            if a.state == "running" || a.state == "queued" {
                let active = state
                    .runs
                    .iter()
                    .any(|r| r.id == a.last_run_id && !r.status.terminal());
                if !active {
                    a.state = "error".into();
                    if a.last_error.is_empty() {
                        a.last_error = "daemon restarted while running".into();
                    }
                    a.updated_at = now();
                    dirty = true;
                }
            }
        }
        let mut store = Store {
            dir: dir.to_path_buf(),
            state,
            loaded_ok,
            store_health,
            preserve_backup,
            // 这三个字段仅内存态、不序列化进 state.json；语义是 "since daemon start"。
            last_write_hash: 0,
            writes_total: 0,
            writes_skipped: 0,
        };
        if dirty {
            store.save();
        }
        prune_corrupt_snapshots(dir, 5);
        store
    }

    pub fn save(&mut self) -> bool {
        let p = self.dir.join("state.json");
        let bak = self.dir.join("state.json.bak");
        let tmp = self.dir.join("state.json.tmp");
        let mut view = self.state.clone();
        for r in &mut view.runs {
            r.prompt = truncate_prompt(&r.prompt);
        }
        let s = match serde_json::to_string_pretty(&view) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("roamway: serialize state failed: {}", e);
                return false;
            }
        };
        let hash = fnv1a64(s.as_bytes());
        if hash == self.last_write_hash && p.exists() {
            self.writes_skipped += 1;
            return false;
        }
        // 先写 tmp 并刷盘；成功后滚旧文件为 .bak，再原子替换正式文件。
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(s.as_bytes())?;
            f.sync_all()?;
            drop(f);
            if p.exists() && !self.preserve_backup {
                fs::rename(&p, &bak)?;
            }
            fs::rename(&tmp, &p)?;
            sync_parent_dir(&p)?;
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("roamway: save state failed: {}", e);
            return false;
        }
        self.writes_total += 1;
        self.last_write_hash = hash;
        self.preserve_backup = false;
        true
    }

    pub fn log_path(&self, id: &str) -> PathBuf {
        // Security: `id` can reach here from user-controlled SSE fallback
        // (server.rs stream_agent_events). Reject anything outside the ID
        // charset so `../` or absolute paths can never escape logs/ and turn
        // the event stream into an arbitrary-file read.
        let safe = !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        let name = if safe { id } else { "invalid" };
        self.dir.join("logs").join(format!("{}.log", name))
    }

    pub fn next_id(&mut self, prefix: &str) -> String {
        self.state.seq += 1;
        format!("{}_{:06}", prefix, self.state.seq)
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn load_state(p: &Path, bak: &Path) -> (State, StoreHealth) {
    match fs::read(p) {
        Ok(bytes) if bytes.is_empty() => (State::default(), StoreHealth::Ok),
        Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
            Ok(st) => (st, StoreHealth::Ok),
            Err(e) => {
                eprintln!("roamway: state.json parse failed: {}", e);
                quarantine_corrupt_state(p);
                recover_from_backup(bak)
            }
        },
        Err(_) => {
            if !p.exists() && !bak.exists() {
                (State::default(), StoreHealth::Fresh)
            } else {
                recover_from_backup(bak)
            }
        }
    }
}

fn quarantine_corrupt_state(p: &Path) {
    let mut ts = now();
    let mut corrupt = p.with_file_name(format!("state.json.corrupt-{}", ts));
    while corrupt.exists() {
        ts += 1;
        corrupt = p.with_file_name(format!("state.json.corrupt-{}", ts));
    }
    if let Err(e) = fs::rename(p, &corrupt) {
        eprintln!("roamway: isolate corrupt state failed: {}", e);
    }
}

fn recover_from_backup(bak: &Path) -> (State, StoreHealth) {
    match fs::read(bak) {
        Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
            Ok(st) => {
                eprintln!("roamway: recovered state from state.json.bak");
                preserve_known_good_backup(bak);
                (st, StoreHealth::Recovered)
            }
            Err(e) => {
                eprintln!("roamway: state.json.bak parse failed: {}", e);
                (State::default(), StoreHealth::Unrecoverable)
            }
        },
        Err(e) => {
            eprintln!("roamway: no usable state ({}); starting empty", e);
            (State::default(), StoreHealth::Unrecoverable)
        }
    }
}

fn preserve_known_good_backup(bak: &Path) {
    let known = bak.with_file_name("state.json.bak.known-good");
    if known.exists() {
        return;
    }
    if let Err(e) = fs::copy(bak, &known) {
        eprintln!("roamway: preserve known-good backup failed: {}", e);
    }
}

/// 只读体检，不落盘、不隔离坏文件、不触发 interrupted 标记。
pub fn inspect_store_health(dir: &Path) -> StoreHealth {
    let p = dir.join("state.json");
    let bak = dir.join("state.json.bak");
    let state_exists = p.is_file();
    let bak_exists = bak.is_file();
    if !state_exists && !bak_exists {
        return StoreHealth::Fresh;
    }
    if let Ok(bytes) = fs::read(&p) {
        if bytes.is_empty() {
            return StoreHealth::Ok;
        }
        if serde_json::from_slice::<State>(&bytes).is_ok() {
            return StoreHealth::Ok;
        }
    }
    if bak_exists {
        if let Ok(bytes) = fs::read(&bak) {
            if serde_json::from_slice::<State>(&bytes).is_ok() {
                return StoreHealth::Recovered;
            }
        }
    }
    StoreHealth::Unrecoverable
}

pub fn prune_corrupt_snapshots(dir: &Path, keep: usize) {
    let mut snaps: Vec<(i64, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(suffix) = name.strip_prefix("state.json.corrupt-") {
                if let Ok(epoch) = suffix.parse::<i64>() {
                    snaps.push((epoch, entry.path()));
                }
            }
        }
    }
    if snaps.len() <= keep {
        return;
    }
    snaps.sort_by_key(|(epoch, _)| *epoch);
    let remove = snaps.len() - keep;
    for (_, path) in snaps.into_iter().take(remove) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(unix)]
fn sync_parent_dir(p: &Path) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        let d = fs::File::open(parent)?;
        d.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_dir(_p: &Path) -> std::io::Result<()> {
    Ok(())
}

pub struct App {
    pub store: Mutex<Store>,
    pub schedules: Mutex<crate::schedules::ScheduleStore>,
    pub terms: Mutex<crate::terminals::TerminalStore>,
    pub term_rt: Mutex<HashMap<String, crate::terminals::Runtime>>,
    pub token: Option<String>,
    pub started: i64,
    pub cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub tz_offset_hours: i64,
    pub max_concurrent: usize,
    pub max_conn: usize,
    pub keep_runs: usize,
    pub keep_days: u64,
    pub max_log_bytes: usize,
    /// queued run 数上限（allow_overlap 积压防护），0 = 不限制。
    pub queued_limit: usize,
    pub conns: AtomicUsize,
    /// `/metrics` 用的 HTTP 计数（纯内存，不参与 state.json 持久化）。
    /// 每次请求 +1；响应码 >= 400 时 failed +1。
    pub req_total: AtomicU64,
    pub req_failed: AtomicU64,
    /// 所有请求累计耗时（微秒）与单请求最大耗时（微秒，纯内存态）。
    pub req_us: AtomicU64,
    pub req_max_us: AtomicU64,
    pub req_4xx: AtomicU64,
    pub req_5xx: AtomicU64,

    pub tokens: Mutex<auth::TokenStore>,
    pub approvals: Mutex<auth::ApprovalStore>,
    pub passwords: Mutex<auth::PasswordStore>,
    pub pairings: Mutex<auth::PairingStore>,
    pub hub_rt: Mutex<crate::hub::HubRuntime>,
    /// hub.json 读-改-写与 webhook 准入的串行锁。
    /// 加锁顺序恒为 hub_lock → store，禁止反向。
    pub hub_lock: Mutex<()>,
    pub allow_hosts: Vec<String>,
    pub allow_origins: Vec<String>,
    pub require_approval: Vec<String>,
    pub bind_host: String,
    pub unix_socket: Option<PathBuf>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
}

impl App {
    /// 仅作为 `Store::save` 的薄封装：计数由 `Store` 自己维护，单一真源。
    pub fn save(&self) {
        let mut st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        st.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_rejects_traversal() {
        // 回归：SSE fallback 会把用户可控 id 直接传给 log_path，
        // 路径穿越字符必须被拒，不能逃出 logs/ 目录。
        let dir = std::env::temp_dir().join(format!("roamway-logpath-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let store = Store::load(&dir);
        let logs = dir.join("logs");
        for bad in ["../../etc/passwd", "..", "a/b", "a\\b", "/abs", "", "x\0y"] {
            let p = store.log_path(bad);
            assert!(p.starts_with(&logs), "escaped logs dir for {:?}", bad);
            assert_eq!(p.file_name().unwrap(), "invalid.log");
        }
        assert_eq!(
            store.log_path("run_000001").file_name().unwrap(),
            "run_000001.log"
        );
        assert_eq!(
            store.log_path("ag_1a2b3c4d5e6f7890").file_name().unwrap(),
            "ag_1a2b3c4d5e6f7890.log"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn iso_epoch_and_known() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn hhmm_validation() {
        assert_eq!(parse_hhmm("08:30"), Some((8, 30)));
        assert_eq!(parse_hhmm("23:59"), Some((23, 59)));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("8:70"), None);
        assert_eq!(parse_hhmm("nope"), None);
    }

    #[test]
    fn daily_lands_on_local_0830() {
        let t = now();
        let at = next_daily(t, "08:30", 8).expect("next daily");
        assert!(at > t, "must be in the future");
        assert!(at - t <= 86400, "within one day");
        let local_secs = (at + 8 * 3600).rem_euclid(86400);
        assert_eq!(local_secs, 8 * 3600 + 30 * 60);
    }

    #[test]
    fn status_terminal() {
        assert!(!RunStatus::Queued.terminal());
        assert!(!RunStatus::Running.terminal());
        assert!(RunStatus::Done.terminal());
        assert!(RunStatus::Interrupted.terminal());
    }

    #[test]
    fn agent_whitelist() {
        for a in [
            "shell",
            "codex",
            "hermes",
            "claude",
            "api:deepseek",
            "api:qwen",
        ] {
            assert!(is_known_agent(a));
        }
        assert!(!is_known_agent("rm -rf /"));
        assert!(!is_known_agent(""));
    }

    #[test]
    fn load_state_recovers_from_backup() {
        let dir = std::env::temp_dir().join(format!("roamway-load-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("state.json");
        let bak = dir.join("state.json.bak");
        fs::write(&p, "{ bad json").unwrap();
        fs::write(&bak, r#"{"runs":[],"jobs":[],"seq":0}"#).unwrap();
        let (state, health) = load_state(&p, &bak);
        assert_eq!(health, StoreHealth::Recovered);
        assert!(state.runs.is_empty());
        assert!(!p.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_state_corrupts_and_starts_empty() {
        let dir = std::env::temp_dir().join(format!("roamway-corrupt-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("state.json");
        let bak = dir.join("state.json.bak");
        fs::write(&p, "{ bad json").unwrap();
        let (state, health) = load_state(&p, &bak);
        assert_eq!(health, StoreHealth::Unrecoverable);
        assert!(state.runs.is_empty());
        let corrupt_exists = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("state.json.corrupt-")
        });
        assert!(corrupt_exists);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_rotates_previous_state_into_backup() {
        let dir = std::env::temp_dir().join(format!("roamway-save-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let mut store = Store {
            dir: dir.clone(),
            state: State {
                seq: 7,
                ..Default::default()
            },
            loaded_ok: true,
            store_health: StoreHealth::Ok,
            preserve_backup: false,
            last_write_hash: 0,
            writes_total: 0,
            writes_skipped: 0,
        };
        assert!(store.save());
        assert!(dir.join("state.json").exists());
        let mut store2 = Store {
            dir: dir.clone(),
            state: State {
                seq: 9,
                ..Default::default()
            },
            loaded_ok: true,
            store_health: StoreHealth::Ok,
            preserve_backup: false,
            last_write_hash: 0,
            writes_total: 0,
            writes_skipped: 0,
        };
        assert!(store2.save());
        assert!(dir.join("state.json.bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_truncates_oversized_prompt_to_bounded_state() {
        let dir = std::env::temp_dir().join(format!("roamway-trunc-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let big = "x".repeat(200_000);
        let run: Run = serde_json::from_value(serde_json::json!({
            "id": "run_1",
            "title": "",
            "agent": "shell",
            "workspace": "",
            "prompt": big,
            "status": "done",
            "created_at": 0,
            "timeout_s": 60,
        }))
        .unwrap();
        let mut store = Store {
            dir: dir.clone(),
            state: State {
                runs: vec![run],
                ..Default::default()
            },
            loaded_ok: true,
            store_health: StoreHealth::Ok,
            preserve_backup: false,
            last_write_hash: 0,
            writes_total: 0,
            writes_skipped: 0,
        };
        assert!(store.save());
        let bytes = fs::read(dir.join("state.json")).unwrap();
        assert!(
            bytes.len() < 20_000,
            "state.json 尺寸应有界，实际 {}",
            bytes.len()
        );
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("[truncated"), "应含截断标记");
        // 往返一致：再 load + save 不膨胀。
        let mut store2 = Store::load(&dir);
        assert!(store2.save());
        let bytes2 = fs::read(dir.join("state.json")).unwrap();
        assert!(bytes2.len() < 20_000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_skips_write_when_state_unchanged() {
        let dir = std::env::temp_dir().join(format!("roamway-skip-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let mut store = Store {
            dir: dir.clone(),
            state: State {
                seq: 7,
                ..Default::default()
            },
            loaded_ok: true,
            store_health: StoreHealth::Ok,
            preserve_backup: false,
            last_write_hash: 0,
            writes_total: 0,
            writes_skipped: 0,
        };
        assert!(store.save());
        let meta1 = fs::metadata(dir.join("state.json")).unwrap();
        let bytes1 = fs::read(dir.join("state.json")).unwrap();
        assert!(!store.save());
        assert_eq!(store.writes_total, 1);
        assert_eq!(store.writes_skipped, 1);
        let meta2 = fs::metadata(dir.join("state.json")).unwrap();
        let bytes2 = fs::read(dir.join("state.json")).unwrap();
        assert_eq!(bytes1, bytes2);
        assert_eq!(meta1.modified().unwrap(), meta2.modified().unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_counters_reset_on_load() {
        let dir = std::env::temp_dir().join(format!("roamway-counters-load-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let mut store = Store::load(&dir);
        assert!(store.save());
        assert_eq!(store.writes_total, 1);
        let reloaded = Store::load(&dir);
        assert_eq!(
            reloaded.writes_total, 0,
            "写计数只在内存态，重新 load 后应归零"
        );
        assert_eq!(
            reloaded.writes_skipped, 0,
            "跳过计数同样应在重新 load 后归零"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_marks_stale_runs_interrupted() {
        let dir = std::env::temp_dir().join(format!("roamway-interrupted-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let run = Run {
            id: "run_000001".into(),
            title: String::new(),
            agent: "shell".into(),
            model: None,
            workspace: String::new(),
            prompt: "echo hi".into(),
            status: RunStatus::Queued,
            created_at: now(),
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
        let state = State {
            runs: vec![run],
            jobs: vec![],
            agents: vec![],
            seq: 1,
        };
        fs::write(dir.join("state.json"), serde_json::to_vec(&state).unwrap()).unwrap();
        let store = Store::load(&dir);
        assert_eq!(store.state.runs.len(), 1);
        assert_eq!(store.state.runs[0].status, RunStatus::Interrupted);
        assert!(store.state.runs[0].ended_at.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_health_fresh_on_empty_dir() {
        let dir = std::env::temp_dir().join(format!("roamway-health-fresh-{}", now()));
        let _ = fs::remove_dir_all(&dir);
        let store = Store::load(&dir);
        assert_eq!(store.store_health, StoreHealth::Fresh);
        assert!(store.loaded_ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_health_ok_on_good_state() {
        let dir = std::env::temp_dir().join(format!("roamway-health-ok-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let state = State {
            seq: 7,
            ..Default::default()
        };
        fs::write(dir.join("state.json"), serde_json::to_vec(&state).unwrap()).unwrap();
        let store = Store::load(&dir);
        assert_eq!(store.store_health, StoreHealth::Ok);
        assert!(store.loaded_ok);
        assert_eq!(store.state.seq, 7);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_health_recovered_from_backup() {
        let dir = std::env::temp_dir().join(format!("roamway-health-recovered-{}", now()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("state.json"), b"{ bad json").unwrap();
        let good = State {
            seq: 9,
            ..Default::default()
        };
        fs::write(
            dir.join("state.json.bak"),
            serde_json::to_vec(&good).unwrap(),
        )
        .unwrap();
        let store = Store::load(&dir);
        assert_eq!(store.store_health, StoreHealth::Recovered);
        assert!(store.loaded_ok);
        assert_eq!(store.state.seq, 9);
        assert!(dir.join("state.json.bak.known-good").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_health_unrecoverable_when_both_bad() {
        let dir = std::env::temp_dir().join(format!("roamway-health-unrecoverable-{}", now()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("state.json"), b"{ bad json").unwrap();
        fs::write(dir.join("state.json.bak"), b"{ also bad").unwrap();
        let store = Store::load(&dir);
        assert_eq!(store.store_health, StoreHealth::Unrecoverable);
        assert!(!store.loaded_ok);
        assert!(store.state.runs.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_skips_backup_rotation_while_preserving() {
        let dir = std::env::temp_dir().join(format!("roamway-save-preserve-{}", now()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("state.json.bak"), b"known-good-original").unwrap();
        let mut store = Store {
            dir: dir.clone(),
            state: State {
                seq: 7,
                ..Default::default()
            },
            loaded_ok: true,
            store_health: StoreHealth::Recovered,
            preserve_backup: true,
            last_write_hash: 0,
            writes_total: 0,
            writes_skipped: 0,
        };
        assert!(store.save());
        assert!(dir.join("state.json").exists());
        assert_eq!(
            fs::read(dir.join("state.json.bak")).unwrap(),
            b"known-good-original"
        );
        assert!(!store.preserve_backup);
        store.state.seq += 1;
        assert!(store.save());
        let rotated = fs::read(dir.join("state.json.bak")).unwrap();
        assert_ne!(rotated, b"known-good-original");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_snapshots_capped_at_five() {
        let dir = std::env::temp_dir().join(format!("roamway-snapshots-{}", now()));
        let _ = fs::create_dir_all(&dir);
        for epoch in 1..=7 {
            fs::write(dir.join(format!("state.json.corrupt-{}", epoch)), b"x").unwrap();
        }
        prune_corrupt_snapshots(&dir, 5);
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("state.json.corrupt-"))
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "state.json.corrupt-3",
                "state.json.corrupt-4",
                "state.json.corrupt-5",
                "state.json.corrupt-6",
                "state.json.corrupt-7",
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
