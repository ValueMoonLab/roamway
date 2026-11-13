use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::types;

/// `hub.json` 数据模型（P9）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HubPermission {
    pub subject: String,
    pub scope: String,
    pub granted_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HubAgentSpec {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HubTrigger {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub secret_env: String,
    pub agent_spec: HubAgentSpec,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Hub {
    pub hub_id: String,
    pub name: String,
    pub created_at: String,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<HubPermission>,
    #[serde(default)]
    pub triggers: Vec<HubTrigger>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct OrgFile {
    #[serde(default)]
    pub triggers: Vec<HubTrigger>,
}

#[derive(Serialize, Clone, Debug)]
pub struct HubActivity {
    pub ts: i64,
    pub source: String,
    pub result: String,
}

/// 守护进程内存态：delivery 去重 + 最近触发记录。hub.json 本体按需读盘。
#[derive(Default)]
pub struct HubRuntime {
    deliveries: VecDeque<String>,
    activity: VecDeque<HubActivity>,
}

fn default_true() -> bool {
    true
}

pub fn hub_path(dir: &Path) -> PathBuf {
    dir.join("hub.json")
}

pub fn gen_hub_id() -> String {
    format!("hub_{}", crate::auth::random_hex(8))
}

pub fn gen_trigger_id() -> String {
    format!("trg_{}", crate::auth::random_hex(8))
}

pub fn secret_env_for(id: &str) -> String {
    format!("ROAMWAY_HOOK_{}", id.to_uppercase())
}

pub fn valid_scope(scope: &str) -> bool {
    matches!(scope, "read" | "run" | "admin")
}

pub fn load(dir: &Path) -> Option<Hub> {
    std::fs::read(hub_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<Hub>(&b).ok())
}

pub fn save(dir: &Path, hub: &Hub) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(hub).map_err(|e| e.to_string())?;
    crate::auth::write_private(&hub_path(dir), &bytes).map_err(|e| e.to_string())
}

pub fn init(dir: &Path, name: Option<String>, force: bool) -> Result<Hub, String> {
    let path = hub_path(dir);
    if path.exists() && !force {
        return Err("hub.json already exists (use --force to overwrite)".into());
    }
    let hub = Hub {
        hub_id: gen_hub_id(),
        name: name.unwrap_or_else(|| "hub".into()),
        created_at: types::iso(types::now()),
        projects: vec![],
        permissions: vec![],
        triggers: vec![],
    };
    save(dir, &hub)?;
    Ok(hub)
}

pub fn grant(hub: &mut Hub, subject: &str, scope: &str, granted_at: &str) {
    match hub.permissions.iter_mut().find(|p| p.subject == subject) {
        Some(p) => {
            p.scope = scope.to_string();
            p.granted_at = granted_at.to_string();
        }
        None => hub.permissions.push(HubPermission {
            subject: subject.to_string(),
            scope: scope.to_string(),
            granted_at: granted_at.to_string(),
        }),
    }
}

pub fn revoke(hub: &mut Hub, subject: &str) -> bool {
    let before = hub.permissions.len();
    hub.permissions.retain(|p| p.subject != subject);
    hub.permissions.len() != before
}

pub fn add_project(hub: &mut Hub, path: &str) -> bool {
    if hub.projects.iter().any(|p| p == path) {
        return false;
    }
    hub.projects.push(path.to_string());
    true
}

pub fn remove_project(hub: &mut Hub, path: &str) -> bool {
    let before = hub.projects.len();
    hub.projects.retain(|p| p != path);
    hub.projects.len() != before
}

fn normalize_trigger(mut t: HubTrigger) -> HubTrigger {
    if t.id.is_empty() {
        t.id = gen_trigger_id();
    }
    if t.secret_env.is_empty() {
        t.secret_env = secret_env_for(&t.id);
    }
    if t.source.is_empty() {
        t.source = "generic".into();
    }
    t
}

pub fn trigger_valid(t: &HubTrigger) -> bool {
    matches!(t.source.as_str(), "github" | "generic")
        && crate::types::is_known_agent(&t.agent_spec.provider)
        && !t.agent_spec.prompt.trim().is_empty()
}

pub fn deploy(dir: &Path, org_path: &Path) -> Result<(usize, usize, usize), String> {
    let mut hub = load(dir).ok_or("hub not initialized (run 'roamway hub init')")?;
    let raw = std::fs::read(org_path).map_err(|e| format!("read {}: {}", org_path.display(), e))?;
    let org: OrgFile =
        serde_json::from_slice(&raw).map_err(|e| format!("parse {}: {}", org_path.display(), e))?;
    let mut imported = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;
    for t in org.triggers {
        let t = normalize_trigger(t);
        if !trigger_valid(&t) {
            skipped += 1;
            continue;
        }
        match hub.triggers.iter().position(|x| x.id == t.id) {
            Some(i) => {
                hub.triggers[i] = t;
                updated += 1;
            }
            None => {
                hub.triggers.push(t);
                imported += 1;
            }
        }
    }
    save(dir, &hub)?;
    Ok((imported, updated, skipped))
}

pub fn rotate_token(dir: &Path) -> Result<String, String> {
    let token = crate::auth::random_hex(32);
    let path = dir.join("token");
    crate::auth::write_private(&path, format!("{}\n", token).as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(token)
}

pub fn logout(dir: &Path, keep_data: bool) {
    let _ = std::fs::remove_file(dir.join("token"));
    if !keep_data {
        let _ = std::fs::remove_file(hub_path(dir));
    }
}

/// 校验 `X-Hub-Signature-256: sha256=<hex>`，HMAC-SHA256 常量时间比较。
pub fn verify_signature(secret: &[u8], body: &[u8], header: &str) -> Result<(), &'static str> {
    let value = header.trim();
    let hex = value
        .strip_prefix("sha256=")
        .ok_or("missing sha256= prefix")?;
    let provided = crate::auth::hex_decode(hex).ok_or("bad signature hex")?;
    if provided.len() != 32 {
        return Err("bad signature length");
    }
    let mac = crate::auth::hmac_sha256(secret, body);
    if crate::auth::ct_eq(&mac, &provided) {
        Ok(())
    } else {
        Err("signature mismatch")
    }
}

/// GitHub 事件过滤；generic 源不要求事件头。
pub fn event_matches(trigger: &HubTrigger, github_event: Option<&str>) -> bool {
    if trigger.source != "github" {
        return true;
    }
    if trigger.events.is_empty() {
        return true;
    }
    match github_event {
        Some(ev) => trigger.events.iter().any(|e| e == ev),
        None => false,
    }
}

/// 掩码来源 IP（仅用于日志，不落原文）。
pub fn mask_ip(ip: &str) -> String {
    let ip = ip.trim();
    if ip.contains('.') {
        if let Some((prefix, _)) = ip.rsplit_once('.') {
            return format!("{}.*", prefix);
        }
    }
    if ip.contains(':') {
        let first = ip.split(':').next().unwrap_or("");
        return format!("{}:***", first);
    }
    if ip.is_empty() {
        return "unknown".into();
    }
    "masked".into()
}
impl HubRuntime {
    /// 返回 true 表示该 delivery 已见过（重复投递）。
    pub fn check_delivery(&mut self, delivery: Option<&str>) -> bool {
        let Some(d) = delivery.map(str::trim).filter(|d| !d.is_empty()) else {
            return false;
        };
        if self.deliveries.iter().any(|x| x == d) {
            return true;
        }
        self.deliveries.push_back(d.to_string());
        while self.deliveries.len() > 1000 {
            self.deliveries.pop_front();
        }
        false
    }

    pub fn record_activity(&mut self, source: String, result: String) {
        self.activity.push_back(HubActivity {
            ts: types::now(),
            source,
            result,
        });
        while self.activity.len() > 5 {
            self.activity.pop_front();
        }
    }

    pub fn recent(&self) -> Vec<HubActivity> {
        self.activity.iter().rev().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("roamway-hub-{}-{}", name, types::now()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn sample_trigger(id: &str) -> HubTrigger {
        HubTrigger {
            id: id.into(),
            source: "github".into(),
            repo: Some("owner/name".into()),
            events: vec!["push".into(), "pull_request".into()],
            secret_env: secret_env_for(id),
            agent_spec: HubAgentSpec {
                provider: "shell".into(),
                cwd: ".".into(),
                prompt: "echo hi".into(),
            },
            enabled: true,
        }
    }

    #[test]
    fn hub_lifecycle_init_save_load() {
        let dir = tmp("lifecycle");
        let hub = init(&dir, Some("team".into()), false).unwrap();
        assert!(hub.hub_id.starts_with("hub_"));
        assert_eq!(hub.hub_id.len(), 4 + 16);
        assert_eq!(hub.name, "team");
        let reloaded = load(&dir).unwrap();
        assert_eq!(reloaded.hub_id, hub.hub_id);
        // 已存在则拒绝；--force 覆盖
        assert!(init(&dir, None, false).is_err());
        let hub2 = init(&dir, Some("team2".into()), true).unwrap();
        assert_eq!(load(&dir).unwrap().hub_id, hub2.hub_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scope_validation_and_grant_revoke() {
        assert!(valid_scope("read"));
        assert!(valid_scope("run"));
        assert!(valid_scope("admin"));
        assert!(!valid_scope("write"));
        let dir = tmp("scope");
        let mut hub = init(&dir, None, false).unwrap();
        grant(&mut hub, "user:alice", "read", "t1");
        grant(&mut hub, "user:alice", "admin", "t2");
        assert_eq!(hub.permissions.len(), 1);
        assert_eq!(hub.permissions[0].scope, "admin");
        grant(&mut hub, "token:abc", "run", "t3");
        assert_eq!(hub.permissions.len(), 2);
        assert!(revoke(&mut hub, "user:alice"));
        assert!(!revoke(&mut hub, "user:alice"));
        assert_eq!(hub.permissions.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projects_add_idempotent_and_remove() {
        let dir = tmp("projects");
        let mut hub = init(&dir, None, false).unwrap();
        assert!(add_project(&mut hub, "/tmp/a"));
        assert!(!add_project(&mut hub, "/tmp/a"));
        assert!(add_project(&mut hub, "/tmp/b"));
        assert_eq!(hub.projects.len(), 2);
        assert!(remove_project(&mut hub, "/tmp/a"));
        assert!(!remove_project(&mut hub, "/tmp/a"));
        assert_eq!(hub.projects, vec!["/tmp/b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deploy_import_update_skip() {
        let dir = tmp("deploy");
        let mut hub = init(&dir, None, false).unwrap();
        hub.triggers.push(sample_trigger("trg_00000001"));
        save(&dir, &hub).unwrap();

        let org = OrgFile {
            triggers: vec![
                sample_trigger("trg_00000001"), // update
                sample_trigger("trg_00000002"), // import
                {
                    let mut bad = sample_trigger("trg_00000003");
                    bad.source = "bogus".into(); // skip
                    bad
                },
                {
                    let mut no_id = sample_trigger("");
                    no_id.id.clear();
                    no_id.secret_env.clear();
                    no_id // 自动补 id -> import
                },
            ],
        };
        let org_path = dir.join("organization.json");
        std::fs::write(&org_path, serde_json::to_vec(&org).unwrap()).unwrap();
        let (imported, updated, skipped) = deploy(&dir, &org_path).unwrap();
        assert_eq!(imported, 2);
        assert_eq!(updated, 1);
        assert_eq!(skipped, 1);
        let reloaded = load(&dir).unwrap();
        assert_eq!(reloaded.triggers.len(), 3);
        assert!(reloaded
            .triggers
            .iter()
            .all(|t| !t.id.is_empty() && !t.secret_env.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hmac_verify_correct_wrong_missing() {
        let secret = b"s3cret";
        let body = b"{\"hello\":1}";
        let mac = crate::auth::hmac_sha256(secret, body);
        let header = format!("sha256={}", crate::auth::hex_encode(&mac));
        assert!(verify_signature(secret, body, &header).is_ok());
        assert!(verify_signature(secret, body, "sha256=deadbeef").is_err());
        assert!(verify_signature(secret, body, "not-a-header").is_err());
        assert!(verify_signature(secret, body, "").is_err());
        assert!(verify_signature(secret, b"other", &header).is_err());
    }

    #[test]
    fn duplicate_delivery_detected() {
        let mut rt = HubRuntime::default();
        assert!(!rt.check_delivery(Some("d-1")));
        assert!(rt.check_delivery(Some("d-1")));
        assert!(!rt.check_delivery(Some("d-2")));
        assert!(!rt.check_delivery(None));
        assert!(!rt.check_delivery(Some("")));
    }

    #[test]
    fn activity_keeps_latest_five() {
        let mut rt = HubRuntime::default();
        for i in 0..8 {
            rt.record_activity(format!("ip{}", i), format!("r{}", i));
        }
        let recent = rt.recent();
        assert_eq!(recent.len(), 5);
        assert_eq!(recent[0].result, "r7");
        assert_eq!(recent[4].result, "r3");
    }

    #[test]
    fn github_event_filter() {
        let t = sample_trigger("trg_00000001");
        assert!(event_matches(&t, Some("push")));
        assert!(!event_matches(&t, Some("issue")));
        assert!(!event_matches(&t, None));
        let mut any = sample_trigger("trg_00000001");
        any.events.clear();
        assert!(event_matches(&any, None));
        let mut generic = sample_trigger("trg_00000001");
        generic.source = "generic".into();
        assert!(event_matches(&generic, None));
    }

    #[test]
    fn mask_ip_covers_v4_v6_and_empty() {
        assert_eq!(mask_ip("192.0.2.7"), "192.0.2.*");
        assert_eq!(mask_ip("2001:db8::1"), "2001:***");
        assert_eq!(mask_ip(""), "unknown");
    }

    #[test]
    fn generated_ids_are_unique_in_burst() {
        // 回归：旧实现同一时钟刻内可产生相同 ID（24bit 截断 + 弱熵）。
        let mut ids = std::collections::HashSet::new();
        for _ in 0..2000 {
            assert!(ids.insert(gen_trigger_id()));
            assert!(ids.insert(gen_hub_id()));
        }
    }

    #[test]
    fn rotate_token_writes_and_overwrites() {
        let dir = tmp("rotate");
        let a = rotate_token(&dir).unwrap();
        let b = rotate_token(&dir).unwrap();
        assert_ne!(a, b);
        let on_disk = std::fs::read_to_string(dir.join("token")).unwrap();
        assert_eq!(on_disk.trim(), b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
