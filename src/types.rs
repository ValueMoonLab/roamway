use serde::{Deserialize, Serialize};
use crate::auth;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agents::Agent;

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
    let doe = (z - era * 146097) as i64;
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
pub const KNOWN_AGENTS: &[&str] = &["shell", "codex", "hermes", "claude", "api:deepseek", "api:qwen"];

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

pub struct Store {
    pub dir: PathBuf,
    pub state: State,
    pub loaded_ok: bool,
}

impl Store {
    pub fn load(dir: &Path) -> Store {
        let _ = fs::create_dir_all(dir);
        let _ = fs::create_dir_all(dir.join("logs"));
        let p = dir.join("state.json");
        let bak = dir.join("state.json.bak");
        let (mut state, loaded_ok) = load_state(&p, &bak);
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
        let store = Store {
            dir: dir.to_path_buf(),
            state,
            loaded_ok,
        };
        if dirty {
            store.save();
        }
        store
    }

    pub fn save(&self) {
        let p = self.dir.join("state.json");
        let bak = self.dir.join("state.json.bak");
        let tmp = self.dir.join("state.json.tmp");
        let s = match serde_json::to_string_pretty(&self.state) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("roamway: serialize state failed: {}", e);
                return;
            }
        };
        // 先写 tmp 并刷盘；成功后滚旧文件为 .bak，再原子替换正式文件。
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(s.as_bytes())?;
            f.sync_all()?;
            drop(f);
            if p.exists() {
                fs::rename(&p, &bak)?;
            }
            fs::rename(&tmp, &p)?;
            sync_parent_dir(&p)?;
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("roamway: save state failed: {}", e);
        }
    }

    pub fn log_path(&self, id: &str) -> PathBuf {
        self.dir.join("logs").join(format!("{}.log", id))
    }

    pub fn next_id(&mut self, prefix: &str) -> String {
        self.state.seq += 1;
        format!("{}_{:06}", prefix, self.state.seq)
    }
}

fn load_state(p: &Path, bak: &Path) -> (State, bool) {
    match fs::read(p) {
        Ok(bytes) if bytes.is_empty() => (State::default(), true),
        Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
            Ok(st) => (st, true),
            Err(e) => {
                eprintln!("roamway: state.json parse failed: {}", e);
                let ts = now();
                let corrupt = p.with_file_name(format!("state.json.corrupt-{}", ts));
                let _ = fs::rename(p, &corrupt);
                recover_from_backup(bak)
            }
        },
        Err(_) => recover_from_backup(bak),
    }
}

fn recover_from_backup(bak: &Path) -> (State, bool) {
    match fs::read(bak) {
        Ok(bytes) => match serde_json::from_slice::<State>(&bytes) {
            Ok(st) => {
                eprintln!("roamway: recovered state from state.json.bak");
                (st, true)
            }
            Err(e) => {
                eprintln!("roamway: state.json.bak parse failed: {}", e);
                (State::default(), false)
            }
        },
        Err(e) => {
            eprintln!("roamway: no usable state ({}); starting empty", e);
            (State::default(), false)
        }
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
    pub conns: AtomicUsize,
    pub tokens: Mutex<auth::TokenStore>,
    pub approvals: Mutex<auth::ApprovalStore>,
    pub passwords: Mutex<auth::PasswordStore>,
    pub pairings: Mutex<auth::PairingStore>,
    pub hub_rt: Mutex<crate::hub::HubRuntime>,
    pub allow_hosts: Vec<String>,
    pub allow_origins: Vec<String>,
    pub require_approval: Vec<String>,
    pub bind_host: String,
    pub unix_socket: Option<PathBuf>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
}

impl App {
    pub fn save(&self) {
        let st = self.store.lock().unwrap_or_else(|e| e.into_inner());
        st.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for a in ["shell", "codex", "hermes", "claude", "api:deepseek", "api:qwen"] {
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
        let (state, ok) = load_state(&p, &bak);
        assert!(ok);
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
        let (state, ok) = load_state(&p, &bak);
        assert!(!ok);
        assert!(state.runs.is_empty());
        let corrupt_exists = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("state.json.corrupt-"));
        assert!(corrupt_exists);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_rotates_previous_state_into_backup() {
        let dir = std::env::temp_dir().join(format!("roamway-save-{}", now()));
        let _ = fs::create_dir_all(&dir);
        let store = Store {
            dir: dir.clone(),
            state: State {
                seq: 7,
                ..Default::default()
            },
            loaded_ok: true,
        };
        store.save();
        assert!(dir.join("state.json").exists());
        let store2 = Store {
            dir: dir.clone(),
            state: State {
                seq: 9,
                ..Default::default()
            },
            loaded_ok: true,
        };
        store2.save();
        assert!(dir.join("state.json.bak").exists());
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
}
