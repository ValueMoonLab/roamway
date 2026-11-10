use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::cron::{parse_cron, timezone_offset_secs};
use crate::types::*;

/// 固定超时：P5 的 Schedule 不携带 timeout，调度触发的 run 统一用这个值。
pub const SCHEDULE_TIMEOUT_S: u64 = 3600;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Schedule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub agent_kind: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub every_secs: u64,
    #[serde(default)]
    pub timezone: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub max_runs: u64,
    #[serde(default)]
    pub runs_count: u64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub last_status: String,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub overlap: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HookRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub triggered_at: i64,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ScheduleStore {
    #[serde(skip)]
    pub dir: PathBuf,
    #[serde(default)]
    pub schedules: Vec<Schedule>,
    #[serde(default)]
    pub hooks: Vec<HookRecord>,
}

static ID_SEQ: AtomicU64 = AtomicU64::new(0);

fn gen_id(prefix: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    let rs = std::collections::hash_map::RandomState::new();
    let mut h = rs.build_hasher();
    h.write_i64(now());
    h.write_u64(ID_SEQ.fetch_add(1, Ordering::Relaxed));
    format!("{}_{:08x}", prefix, (h.finish() & 0xffff_ffff) as u32)
}

pub fn gen_schedule_id() -> String {
    gen_id("sch")
}

pub fn gen_hook_id() -> String {
    gen_id("hook")
}

/// 把 P1 的 tz_offset_hours 转成固定偏移字符串（P5 timezone 字段）。
pub fn offset_to_tz(hours: i64) -> String {
    let total_min = hours * 60;
    let sign = if total_min < 0 { '-' } else { '+' };
    let abs = total_min.abs();
    format!("{}{:02}:{:02}", sign, abs / 60, abs % 60)
}

/// 解析 "30s" | "5m" | "2h" | "1d"（无后缀按秒）。
pub fn parse_every(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, "s"),
    };
    let n: u64 = num.trim().parse().ok()?;
    if n == 0 {
        return None;
    }
    let secs = match unit.trim() {
        "" | "s" | "sec" | "secs" => n,
        "m" | "min" | "mins" => n.checked_mul(60)?,
        "h" | "hr" | "hrs" => n.checked_mul(3600)?,
        "d" | "day" | "days" => n.checked_mul(86400)?,
        _ => return None,
    };
    Some(secs)
}

/// 连续失败 n 次时的退避秒数：min(60 * 2^(n-1), 3600)。
pub fn failure_backoff(n: u32) -> i64 {
    let n = n.clamp(1, 8);
    (60i64 * (1i64 << (n - 1))).min(3600)
}

/// 计算下一次触发时间：every 优先，否则按 cron 在时区里求值。
pub fn schedule_next_after(s: &Schedule, t: i64) -> Option<i64> {
    if s.every_secs > 0 {
        return Some(t + s.every_secs as i64);
    }
    let tz = timezone_offset_secs(&s.timezone).unwrap_or(0);
    parse_cron(&s.cron).ok().and_then(|c| c.next_after(t, tz))
}

pub fn exhausted(s: &Schedule) -> bool {
    s.max_runs > 0 && s.runs_count >= s.max_runs
}

pub fn should_skip_overlap(s: &Schedule, active: bool) -> bool {
    !s.overlap && active
}

#[derive(Debug, PartialEq, Eq)]
pub enum TickAction {
    Idle,
    SkipOverlap,
    DisableMaxRuns,
    DisableExpired,
    Run,
}

/// 单个 schedule 的 tick 判定（纯函数，便于单测）。
pub fn decide_tick(s: &Schedule, t: i64, active: bool) -> TickAction {
    if !s.enabled || s.paused {
        return TickAction::Idle;
    }
    match s.next_run_at {
        None => TickAction::Idle,
        Some(n) if n > t => TickAction::Idle,
        Some(_) => {
            if exhausted(s) {
                return TickAction::DisableMaxRuns;
            }
            if s.expires_at.map(|e| e <= t).unwrap_or(false) {
                return TickAction::DisableExpired;
            }
            if should_skip_overlap(s, active) {
                return TickAction::SkipOverlap;
            }
            TickAction::Run
        }
    }
}

impl ScheduleStore {
    pub fn load(dir: &Path) -> ScheduleStore {
        let _ = fs::create_dir_all(dir);
        let p = dir.join("schedules.json");
        let mut migrated = false;
        let mut store = match fs::read(&p) {
            Ok(bytes) if bytes.is_empty() => ScheduleStore {
                dir: dir.to_path_buf(),
                ..Default::default()
            },
            Ok(bytes) => match serde_json::from_slice::<ScheduleStore>(&bytes) {
                Ok(mut s) => {
                    s.dir = dir.to_path_buf();
                    s
                }
                Err(e) => {
                    eprintln!("roamway: schedules.json parse failed: {}", e);
                    let ts = now();
                    let corrupt = p.with_file_name(format!("schedules.json.corrupt-{}", ts));
                    let _ = fs::rename(&p, &corrupt);
                    ScheduleStore {
                        dir: dir.to_path_buf(),
                        ..Default::default()
                    }
                }
            },
            Err(_) => {
                let (schedules, m) = migrate_jobs_file(dir);
                migrated = m;
                if migrated {
                    eprintln!("roamway: migrated jobs.json into schedules.json");
                }
                ScheduleStore {
                    dir: dir.to_path_buf(),
                    schedules,
                    hooks: Vec::new(),
                }
            }
        };
        let t = now();
        let mut dirty = false;
        for s in store.schedules.iter_mut() {
            if s.enabled && !s.paused && s.next_run_at.is_none() {
                s.next_run_at = schedule_next_after(s, t);
                dirty = true;
            }
        }
        if migrated || dirty {
            store.save();
        }
        store
    }

    pub fn save(&self) {
        if self.dir.as_os_str().is_empty() {
            return;
        }
        let p = self.dir.join("schedules.json");
        let bak = self.dir.join("schedules.json.bak");
        let tmp = self.dir.join("schedules.json.tmp");
        let s = match serde_json::to_string_pretty(self) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("roamway: serialize schedules failed: {}", e);
                return;
            }
        };
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(s.as_bytes())?;
            f.sync_all()?;
            drop(f);
            if p.exists() {
                fs::rename(&p, &bak)?;
            }
            fs::rename(&tmp, &p)?;
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("roamway: save schedules failed: {}", e);
        }
    }
}

pub fn find<'a>(store: &'a ScheduleStore, key: &str) -> Option<&'a Schedule> {
    store
        .schedules
        .iter()
        .find(|s| s.id == key || s.name == key)
}

pub fn find_mut<'a>(store: &'a mut ScheduleStore, key: &str) -> Option<&'a mut Schedule> {
    store
        .schedules
        .iter_mut()
        .find(|s| s.id == key || s.name == key)
}

#[derive(Deserialize)]
struct JobFile {
    #[serde(default)]
    jobs: Vec<Job>,
}

fn migrate_jobs_file(dir: &Path) -> (Vec<Schedule>, bool) {
    let src = dir.join("jobs.json");
    if !src.exists() {
        return (Vec::new(), false);
    }
    let bytes = match fs::read(&src) {
        Ok(b) => b,
        Err(_) => return (Vec::new(), false),
    };
    let jobs = serde_json::from_slice::<Vec<Job>>(&bytes)
        .or_else(|_| serde_json::from_slice::<JobFile>(&bytes).map(|f| f.jobs))
        .unwrap_or_default();
    let schedules = migrate_jobs(jobs, now());
    let _ = fs::rename(&src, dir.join("jobs.json.migrated"));
    (schedules, true)
}

/// 旧 {every|daily} 记录转 Schedule。
///
/// 迁移源按规范是 `jobs.json`（本仓库 P1 实际把 jobs 存在 state.json，二者互不影响）。
/// 转换策略保留原语义：every -> every_secs；daily HH:MM -> cron "M H * * *"。
pub fn migrate_jobs(jobs: Vec<Job>, now_ts: i64) -> Vec<Schedule> {
    let mut used: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for j in jobs {
        if j.every_secs.is_none() && j.daily.is_none() {
            continue;
        }
        let (cron, every_secs) = if let Some(d) = j.daily.as_deref() {
            match parse_hhmm(d) {
                Some((h, m)) => (format!("{} {} * * *", m, h), 0),
                None => ("0 0 * * *".to_string(), 86400),
            }
        } else if let Some(e) = j.every_secs {
            if e > 0 {
                (String::new(), e)
            } else {
                (String::new(), 86400)
            }
        } else {
            (String::new(), 86400)
        };
        let tz = offset_to_tz(j.tz_offset_hours);
        let next_run_at = if every_secs > 0 {
            Some(now_ts + every_secs as i64)
        } else {
            timezone_offset_secs(&tz).ok().and_then(|off| {
                parse_cron(&cron)
                    .ok()
                    .and_then(|c| c.next_after(now_ts, off))
            })
        };
        let mut name = if j.title.trim().is_empty() {
            j.id.clone()
        } else {
            j.title.clone()
        };
        if name.trim().is_empty() {
            name = format!("schedule-{}", out.len() + 1);
        }
        let mut unique = name.clone();
        let mut k = 2;
        while used.contains(&unique) {
            unique = format!("{}-{}", name, k);
            k += 1;
        }
        used.insert(unique.clone());
        out.push(Schedule {
            id: gen_schedule_id(),
            name: unique,
            agent_kind: j.agent.clone(),
            prompt: j.prompt.clone(),
            workspace: j.workspace.clone(),
            model: j.model.clone().unwrap_or_default(),
            cron,
            every_secs,
            timezone: tz,
            enabled: j.enabled,
            paused: false,
            max_runs: 0,
            runs_count: 0,
            expires_at: None,
            created_at: now_ts,
            next_run_at,
            last_run_at: None,
            last_status: String::new(),
            consecutive_failures: 0,
            overlap: j.allow_overlap,
        });
    }
    out
}

pub fn start_scheduler(app: Arc<App>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        tick(&app);
    });
}

fn tick(app: &Arc<App>) {
    let due: Vec<String> = {
        let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        let active: Vec<String> = {
            let st = app.store.lock().unwrap_or_else(|e| e.into_inner());
            st.state
                .runs
                .iter()
                .filter(|r| !r.status.terminal())
                .filter_map(|r| r.job_id.clone())
                .collect()
        };
        let mut dirty = false;
        let mut due = Vec::new();
        for s in ss.schedules.iter_mut() {
            if !s.enabled || s.paused {
                continue;
            }
            if s.next_run_at.is_none() {
                s.next_run_at = schedule_next_after(s, t);
                dirty = true;
                continue;
            }
            match decide_tick(s, t, active.contains(&s.id)) {
                TickAction::Idle => {}
                TickAction::SkipOverlap => {
                    s.last_status = "skipped(overlap)".into();
                    s.next_run_at = schedule_next_after(s, t);
                    dirty = true;
                }
                TickAction::DisableMaxRuns => {
                    s.enabled = false;
                    s.last_status = "disabled(max_runs)".into();
                    dirty = true;
                }
                TickAction::DisableExpired => {
                    s.enabled = false;
                    s.last_status = "disabled(expired)".into();
                    dirty = true;
                }
                TickAction::Run => {
                    s.runs_count += 1;
                    s.last_run_at = Some(t);
                    s.last_status = "running".into();
                    s.next_run_at = schedule_next_after(s, t);
                    due.push(s.id.clone());
                    dirty = true;
                }
            }
        }
        if dirty {
            ss.save();
        }
        due
    };
    for id in due {
        create_run_for_schedule(app, &id);
    }
}

pub fn create_run_for_schedule(app: &Arc<App>, schedule_id: &str) {
    let schedule = {
        let ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
        ss.schedules.iter().find(|s| s.id == schedule_id).cloned()
    };
    let Some(s) = schedule else {
        return;
    };
    let run_id = {
        let mut st = app.store.lock().unwrap_or_else(|e| e.into_inner());
        let id = st.next_id("run");
        st.state.runs.push(Run {
            id: id.clone(),
            title: if s.name.is_empty() {
                format!("schedule:{}", s.id)
            } else {
                s.name.clone()
            },
            agent: s.agent_kind.clone(),
            model: if s.model.is_empty() {
                None
            } else {
                Some(s.model.clone())
            },
            workspace: s.workspace.clone(),
            prompt: s.prompt.clone(),
            status: RunStatus::Queued,
            created_at: now(),
            started_at: None,
            ended_at: None,
            exit_code: None,
            error: None,
            resumed: false,
            resume_note: None,
            timeout_s: SCHEDULE_TIMEOUT_S,
            job_id: Some(s.id.clone()),
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
        });
        crate::jobs::prune(&mut st, app.keep_runs, app.keep_days);
        st.save();
        id
    };
    record_hook(app, &s, &run_id, "queued");
    crate::jobs::drain_queued(app);
}

fn record_hook(app: &Arc<App>, s: &Schedule, run_id: &str, status: &str) {
    let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    ss.hooks.push(HookRecord {
        id: gen_hook_id(),
        name: s.name.clone(),
        event: "schedule.run".into(),
        triggered_at: now(),
        run_id: Some(run_id.to_string()),
        status: status.to_string(),
    });
    if ss.hooks.len() > 200 {
        let excess = ss.hooks.len() - 200;
        ss.hooks.drain(..excess);
    }
    ss.save();
}

pub fn run_once(app: &Arc<App>, key: &str) -> Result<String, String> {
    let id = {
        let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = find_mut(&mut ss, key) else {
            return Err("no such schedule".into());
        };
        s.runs_count += 1;
        s.last_run_at = Some(now());
        s.last_status = "running".into();
        let id = s.id.clone();
        ss.save();
        id
    };
    create_run_for_schedule(app, &id);
    Ok(id)
}

pub fn pause(app: &Arc<App>, key: &str) -> Result<Schedule, String> {
    let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = find_mut(&mut ss, key) else {
        return Err("no such schedule".into());
    };
    s.paused = true;
    let out = s.clone();
    ss.save();
    Ok(out)
}

pub fn resume(app: &Arc<App>, key: &str) -> Result<Schedule, String> {
    let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = find_mut(&mut ss, key) else {
        return Err("no such schedule".into());
    };
    s.paused = false;
    s.next_run_at = schedule_next_after(s, now());
    let out = s.clone();
    ss.save();
    Ok(out)
}

pub fn delete(app: &Arc<App>, key: &str) -> Result<Schedule, String> {
    let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    let Some(i) = ss
        .schedules
        .iter()
        .position(|s| s.id == key || s.name == key)
    else {
        return Err("no such schedule".into());
    };
    let out = ss.schedules.remove(i);
    ss.save();
    Ok(out)
}

/// runner 完成一个调度触发的 run 后回调；调用方必须已释放 store 锁。
pub fn record_result(app: &Arc<App>, schedule_id: &str, status: RunStatus, err: Option<String>) {
    let mut ss = app.schedules.lock().unwrap_or_else(|e| e.into_inner());
    let t = now();
    let mut dirty = false;
    if let Some(s) = ss.schedules.iter_mut().find(|s| s.id == schedule_id) {
        let bad = matches!(status, RunStatus::Failed | RunStatus::Timeout);
        s.last_run_at = Some(t);
        if bad {
            s.consecutive_failures += 1;
            s.last_status = if status == RunStatus::Timeout {
                "timeout".into()
            } else {
                "failed".into()
            };
            let backoff = failure_backoff(s.consecutive_failures);
            let cron_next = schedule_next_after(s, t);
            let next = match cron_next {
                Some(cn) => cn.max(t + backoff),
                None => t + backoff,
            };
            s.next_run_at = Some(next);
        } else {
            s.consecutive_failures = 0;
            s.last_status = match status {
                RunStatus::Done => "ok".into(),
                other => format!("{:?}", other).to_lowercase(),
            };
        }
        dirty = true;
    }
    if dirty {
        ss.save();
    }
    let _ = err;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_schedule(every: u64, max_runs: u64) -> Schedule {
        Schedule {
            id: "sch_00000001".into(),
            name: "t1".into(),
            agent_kind: "shell".into(),
            prompt: "echo hi".into(),
            workspace: String::new(),
            model: String::new(),
            cron: String::new(),
            every_secs: every,
            timezone: "UTC".into(),
            enabled: true,
            paused: false,
            max_runs,
            runs_count: 0,
            expires_at: None,
            created_at: 0,
            next_run_at: Some(1000),
            last_run_at: None,
            last_status: String::new(),
            consecutive_failures: 0,
            overlap: false,
        }
    }

    #[test]
    fn parse_every_units() {
        assert_eq!(parse_every("30s"), Some(30));
        assert_eq!(parse_every("5m"), Some(300));
        assert_eq!(parse_every("2h"), Some(7200));
        assert_eq!(parse_every("1d"), Some(86400));
        assert_eq!(parse_every("90"), Some(90));
        assert_eq!(parse_every("0s"), None);
        assert_eq!(parse_every("1w"), None);
    }

    #[test]
    fn failure_backoff_caps_at_one_hour() {
        assert_eq!(failure_backoff(1), 60);
        assert_eq!(failure_backoff(2), 120);
        assert_eq!(failure_backoff(3), 240);
        assert_eq!(failure_backoff(6), 1920);
        assert_eq!(failure_backoff(7), 3600);
        assert_eq!(failure_backoff(9), 3600);
    }

    #[test]
    fn overlap_skips_when_active_run_present() {
        let mut s = sample_schedule(5, 0);
        assert_eq!(decide_tick(&s, 2000, true), TickAction::SkipOverlap);
        s.overlap = true;
        assert_eq!(decide_tick(&s, 2000, true), TickAction::Run);
    }

    #[test]
    fn max_runs_disables_after_exhaustion() {
        let s = Schedule {
            runs_count: 3,
            max_runs: 3,
            ..sample_schedule(5, 3)
        };
        assert!(exhausted(&s));
        assert_eq!(decide_tick(&s, 2000, false), TickAction::DisableMaxRuns);
    }

    #[test]
    fn paused_schedule_never_triggers() {
        let s = Schedule {
            paused: true,
            ..sample_schedule(5, 0)
        };
        assert_eq!(decide_tick(&s, 2000, false), TickAction::Idle);
    }

    #[test]
    fn expired_schedule_disables() {
        let s = Schedule {
            expires_at: Some(1500),
            ..sample_schedule(5, 0)
        };
        assert_eq!(decide_tick(&s, 2000, false), TickAction::DisableExpired);
    }

    #[test]
    fn migrate_old_jobs_preserves_semantics() {
        let jobs = vec![
            Job {
                id: "job_000001".into(),
                title: "daily job".into(),
                agent: "shell".into(),
                model: None,
                workspace: "/tmp".into(),
                prompt: "echo d".into(),
                every_secs: None,
                daily: Some("08:30".into()),
                tz_offset_hours: 8,
                timeout_s: 60,
                max_retries: 2,
                enabled: true,
                allow_overlap: false,
                next_at: 0,
                last_run: None,
                last_error: None,
                attempts: 0,
                depends_on: Vec::new(),
                last_skip: None,
            },
            Job {
                id: "job_000002".into(),
                title: String::new(),
                agent: "shell".into(),
                model: None,
                workspace: "/tmp".into(),
                prompt: "echo e".into(),
                every_secs: Some(3600),
                daily: None,
                tz_offset_hours: -5,
                timeout_s: 60,
                max_retries: 2,
                enabled: true,
                allow_overlap: true,
                next_at: 0,
                last_run: None,
                last_error: None,
                attempts: 0,
                depends_on: Vec::new(),
                last_skip: None,
            },
        ];
        let out = migrate_jobs(jobs, 1_000_000);
        assert_eq!(out.len(), 2);
        let daily = out.iter().find(|s| s.name == "daily job").unwrap();
        assert_eq!(daily.cron, "30 8 * * *");
        assert_eq!(daily.every_secs, 0);
        assert_eq!(daily.timezone, "+08:00");
        let every = out.iter().find(|s| s.name == "job_000002").unwrap();
        assert_eq!(every.every_secs, 3600);
        assert_eq!(every.timezone, "-05:00");
        assert!(every.overlap);
    }

    #[test]
    fn offset_to_tz_formats_sign_and_minutes() {
        assert_eq!(offset_to_tz(8), "+08:00");
        assert_eq!(offset_to_tz(-5), "-05:00");
        assert_eq!(offset_to_tz(0), "+00:00");
    }
}
