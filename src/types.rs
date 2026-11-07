use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn civil_from_days(z: i64) -> (i64, u32, u32) {
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
    pub timeout_s: u64,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub attempts: u32,
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
    pub seq: u64,
}

pub struct Store {
    pub dir: PathBuf,
    pub state: State,
}

impl Store {
    pub fn load(dir: &Path) -> Store {
        let _ = fs::create_dir_all(dir);
        let _ = fs::create_dir_all(dir.join("logs"));
        let p = dir.join("state.json");
        let mut state: State = fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // 重启后，上次遗留的 queued/running 一律标记 interrupted，绝不自动重放
        let mut dirty = false;
        for r in state.runs.iter_mut() {
            if !r.status.terminal() {
                r.status = RunStatus::Interrupted;
                r.ended_at = Some(now());
                r.error = Some("daemon restarted while running".into());
                dirty = true;
            }
        }
        let store = Store {
            dir: dir.to_path_buf(),
            state,
        };
        if dirty {
            store.save();
        }
        store
    }

    pub fn save(&self) {
        let p = self.dir.join("state.json");
        let tmp = self.dir.join("state.json.tmp");
        if let Ok(s) = serde_json::to_string_pretty(&self.state) {
            if fs::write(&tmp, s).is_ok() {
                let _ = fs::rename(&tmp, &p);
            }
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

pub struct App {
    pub store: Mutex<Store>,
    pub token: Option<String>,
    pub started: i64,
    pub cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub tz_offset_hours: i64,
}

impl App {
    pub fn save(&self) {
        if let Ok(st) = self.store.lock() {
            st.save();
        }
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
}
