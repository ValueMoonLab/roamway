use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Provider {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub base_url: String,
    pub env_key: String,
    pub models: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub fn builtins() -> Vec<Provider> {
    vec![
        Provider {
            id: "api:deepseek".into(),
            kind: "api".into(),
            label: "DeepSeek".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            env_key: "ROAMWAY_DEEPSEEK_KEY".into(),
            models: vec!["deepseek-chat".into(), "deepseek-reasoner".into()],
            enabled: true,
        },
        Provider {
            id: "api:qwen".into(),
            kind: "api".into(),
            label: "Qwen (DashScope)".into(),
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
            env_key: "ROAMWAY_QWEN_KEY".into(),
            models: vec!["qwen-plus".into(), "qwen-max".into(), "qwen-turbo".into()],
            enabled: true,
        },
        Provider {
            id: "agent:codex".into(),
            kind: "agent".into(),
            label: "Codex CLI".into(),
            base_url: String::new(),
            env_key: String::new(),
            models: vec![],
            enabled: true,
        },
        Provider {
            id: "agent:claude".into(),
            kind: "agent".into(),
            label: "Claude Code".into(),
            base_url: String::new(),
            env_key: String::new(),
            models: vec![],
            enabled: true,
        },
    ]
}

pub fn binary_name(provider: &Provider) -> String {
    if provider.kind == "agent" {
        provider
            .id
            .rsplit_once(':')
            .map(|(_, name)| name.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.id.clone())
    } else {
        String::new()
    }
}

pub fn load(dir: &Path) -> Vec<Provider> {
    let mut providers = builtins();
    let path = dir.join("providers.json");
    let raw = match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return providers,
    };
    let user: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("roamway: providers.json parse failed: {}", e);
            return providers;
        }
    };
    let arr = user
        .get("providers")
        .and_then(|v| v.as_array())
        .cloned()
        .or_else(|| user.as_array().cloned())
        .unwrap_or_default();
    for item in arr {
        let id = match item.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };
        if let Some(existing) = providers.iter_mut().find(|p| p.id == id) {
            merge_into(existing, &item);
        } else if let Ok(p) = serde_json::from_value::<Provider>(item) {
            providers.push(p);
        }
    }
    providers
}

fn merge_into(target: &mut Provider, item: &serde_json::Value) {
    if let Some(v) = item.get("kind").and_then(|v| v.as_str()) {
        target.kind = v.to_string();
    }
    if let Some(v) = item.get("label").and_then(|v| v.as_str()) {
        target.label = v.to_string();
    }
    if let Some(v) = item.get("base_url").and_then(|v| v.as_str()) {
        target.base_url = v.to_string();
    }
    if let Some(v) = item.get("env_key").and_then(|v| v.as_str()) {
        target.env_key = v.to_string();
    }
    if let Some(v) = item.get("models").and_then(|v| v.as_array()) {
        target.models = v
            .iter()
            .filter_map(|m| m.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(v) = item.get("enabled").and_then(|v| v.as_bool()) {
        target.enabled = v;
    }
}

pub fn get<'a>(providers: &'a [Provider], id: &str) -> Option<&'a Provider> {
    providers.iter().find(|p| p.id == id)
}

pub fn state(p: &Provider) -> &'static str {
    if !p.enabled {
        return "disabled";
    }
    if p.kind == "agent" {
        let bin = binary_name(p);
        if bin.is_empty() || !which_in_path(&bin) {
            return "no-binary";
        }
        return "ready";
    }
    if p.kind == "api" {
        if p.env_key.is_empty() {
            return "no-key";
        }
        match std::env::var(&p.env_key) {
            Ok(v) if !v.trim().is_empty() => "ready",
            _ => "no-key",
        }
    } else {
        "disabled"
    }
}

pub fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

pub fn fetch_models(base_url: &str) -> Result<Vec<String>, String> {
    let url = models_url(base_url);
    match ureq::get(&url).timeout(Duration::from_secs(10)).call() {
        Ok(resp) => {
            let v: serde_json::Value = resp
                .into_json()
                .map_err(|e| format!("bad json: {}", e))?;
            let data = v
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| "no data array in response".to_string())?;
            let models: Vec<String> = data
                .iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                .collect();
            if models.is_empty() {
                Err("empty model list".into())
            } else {
                Ok(models)
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(format!("HTTP {}: {}", code, body))
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn endpoint_status(base_url: &str) -> Result<u16, String> {
    let url = models_url(base_url);
    match ureq::get(&url).timeout(Duration::from_secs(10)).call() {
        Ok(resp) => Ok(resp.status()),
        Err(ureq::Error::Status(code, resp)) => {
            let _ = resp.into_string();
            Ok(code)
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn which_in_path(bin: &str) -> bool {
    if bin.is_empty() {
        return false;
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path.split(sep) {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(bin);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&candidate) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return true;
                    }
                }
            }
            #[cfg(not(unix))]
            return true;
        }
    }
    false
}

pub fn diagnostic_lines(dir: &Path, p: &Provider) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("id: {}", p.id));
    out.push(format!("kind: {}", p.kind));
    out.push(format!("label: {}", p.label));
    out.push(format!("enabled: {}", p.enabled));
    if p.kind == "api" {
        let present = !p.env_key.is_empty()
            && std::env::var(&p.env_key)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        out.push(format!(
            "env key: {} -> {}",
            p.env_key,
            if present { "[set]" } else { "[missing]" }
        ));
    } else {
        let bin = binary_name(p);
        out.push(format!(
            "binary: {} -> {}",
            bin,
            if which_in_path(&bin) {
                "in PATH"
            } else {
                "not in PATH"
            }
        ));
    }
    if !p.base_url.is_empty() {
        match endpoint_status(&p.base_url) {
            Ok(code) => out.push(format!("endpoint: {} -> HTTP {}", p.base_url, code)),
            Err(e) => out.push(format!("endpoint: {} -> {}", p.base_url, e)),
        }
    } else {
        out.push("endpoint: (none)".into());
    }
    let cfg = dir.join("providers.json");
    out.push(format!("config: {}", cfg.display()));
    if cfg.exists() {
        let readable = fs::read(&cfg).is_ok();
        out.push(format!(
            "permissions: {}",
            if readable { "readable" } else { "unreadable" }
        ));
    } else {
        out.push("permissions: n/a (no providers.json, using builtins)".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_providers_present() {
        let b = builtins();
        assert!(b.iter().any(|p| p.id == "api:deepseek"));
        assert!(b.iter().any(|p| p.id == "api:qwen"));
        assert!(b.iter().any(|p| p.id == "agent:codex"));
        assert!(b.iter().any(|p| p.id == "agent:claude"));
    }

    #[test]
    fn merge_overrides_only_present_fields() {
        let dir = std::env::temp_dir().join(format!("roamway-prov-merge-{}", crate::types::now()));
        let _ = fs::create_dir_all(&dir);
        fs::write(
            dir.join("providers.json"),
            r#"{ "providers": [ { "id": "api:deepseek", "label": "DeepSeek V4", "models": ["deepseek-v4-pro"] } ] }"#,
        )
        .unwrap();
        let providers = load(&dir);
        let p = providers.iter().find(|p| p.id == "api:deepseek").unwrap();
        assert_eq!(p.label, "DeepSeek V4");
        assert_eq!(p.models, vec!["deepseek-v4-pro".to_string()]);
        assert_eq!(p.base_url, "https://api.deepseek.com/v1");
        assert_eq!(p.kind, "api");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn diagnostic_lines_agent_reports_binary() {
        let dir = std::env::temp_dir().join(format!("roamway-prov-diag-{}", crate::types::now()));
        let _ = fs::create_dir_all(&dir);
        let p = builtins().into_iter().find(|p| p.id == "agent:codex").unwrap();
        let lines = diagnostic_lines(&dir, &p);
        assert!(lines.iter().any(|l| l.starts_with("binary: codex")));
        assert!(lines.iter().any(|l| l.starts_with("id: agent:codex")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_tolerates_corrupt_file() {
        let dir = std::env::temp_dir().join(format!("roamway-prov-corrupt-{}", crate::types::now()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("providers.json"), "{ bad json").unwrap();
        let providers = load(&dir);
        assert_eq!(providers.len(), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_reflects_missing_key() {
        let p = Provider {
            id: "api:deepseek".into(),
            kind: "api".into(),
            label: "DeepSeek".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            env_key: "ROAMWAY_P8_NO_SUCH_KEY_987".into(),
            models: vec![],
            enabled: true,
        };
        assert_eq!(state(&p), "no-key");
        let mut ready = p.clone();
        ready.env_key = "PATH".into();
        assert_eq!(state(&ready), "ready");
        ready.enabled = false;
        assert_eq!(state(&ready), "disabled");
    }
}