use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub token: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub created_at: i64,
    #[serde(default)]
    pub last_probe: Option<Probe>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub next_retry_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Probe {
    pub at: i64,
    pub ok: bool,
    pub latency_ms: u64,
    pub version: String,
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Registry {
    #[serde(skip)]
    pub path: PathBuf,
    pub hosts: Vec<Host>,
}

fn default_true() -> bool {
    true
}

/// `192.0.2.10:8796` / `tcp://192.0.2.10:8796` / `http://192.0.2.10:8796/`
/// 都归一化为 `http://192.0.2.10:8796`。
pub fn endpoint_normalize(s: &str) -> String {
    let s = s.trim();
    let s = if let Some(rest) = s.strip_prefix("tcp://") {
        format!("http://{}", rest)
    } else if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("http://{}", s)
    };
    s.trim_end_matches('/').to_string()
}

fn split_authority(auth: &str) -> (String, String) {
    // IPv6 literal in brackets keeps its colons: [::]:8796
    if let Some(rest) = auth.strip_prefix('[') {
        if let Some(idx) = rest.find(']') {
            let host = format!("[{}]", &rest[..idx]);
            let after = &rest[idx + 1..];
            let port = match after.strip_prefix(':') {
                Some(p) => format!(":{}", p),
                None => String::new(),
            };
            return (host, port);
        }
    }
    match auth.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.to_string(), format!(":{}", p))
        }
        _ => (auth.to_string(), String::new()),
    }
}

/// A listen address is never a reachable authority: `0.0.0.0`, `::`, `[::]`, `*`, empty.
fn wildcard_host(host: &str) -> bool {
    host.is_empty()
        || host == "*"
        || host == "0.0.0.0"
        || host == "::"
        || host == "[::]"
        || host == "::0"
        || host == "[::0]"
}

/// True when the endpoint's host part is a listen-only wildcard address.
/// `--host name` 用 registry 里的 endpoint 原样访问（不偷偷改写远端地址），
/// 但 wildcard 永远连不上：调用方（如 `host probe`）要据此给出明确提示。
pub fn endpoint_is_wildcard(raw: &str) -> bool {
    let norm = endpoint_normalize(raw);
    let rest = match norm.split_once("://") {
        Some((_, r)) => r.to_string(),
        None => norm,
    };
    let authority = rest.split('/').next().unwrap_or("");
    let (host, _) = split_authority(authority);
    wildcard_host(&host)
}

/// Client-facing base URL derived from a persisted server `bind` value.
///
/// A wildcard bind is a listen address, not a reachable Host header: sending it makes
/// the server answer 421 misdirected request because the rewritten authority is not in
/// `--allow-host`. Map the wildcard to loopback while keeping the port and any path.
pub fn client_base_from_bind(raw: &str) -> String {
    let norm = endpoint_normalize(raw);
    let (scheme, rest) = match norm.split_once("://") {
        Some((s, r)) => (s.to_string(), r.to_string()),
        None => ("http".to_string(), norm.clone()),
    };
    let (authority, tail) = match rest.split_once('/') {
        Some((a, t)) => (a.to_string(), format!("/{}", t)),
        None => (rest.clone(), String::new()),
    };
    let (host, port) = split_authority(&authority);
    if wildcard_host(&host) {
        format!("{}://127.0.0.1{}{}", scheme, port, tail)
    } else {
        format!("{}://{}{}", scheme, authority, tail)
    }
}

pub fn mask(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 8 {
        return "****".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{}...{}", head, tail)
}

pub fn backoff(failures: u32) -> i64 {
    if failures == 0 {
        return 0;
    }
    let n = failures.clamp(1, 8);
    (30i64 << (n - 1)).min(3600)
}

pub fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 32 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn gen_host_id() -> String {
    format!("host_{}", crate::auth::random_hex(8))
}

impl Registry {
    pub fn load(dir: &Path) -> Self {
        let _ = fs::create_dir_all(dir);
        let path = dir.join("hosts.json");
        match fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => Registry {
                path,
                hosts: Vec::new(),
            },
            Ok(bytes) => match serde_json::from_slice::<Vec<Host>>(&bytes) {
                Ok(hosts) => Registry { path, hosts },
                Err(e) => {
                    eprintln!("roamway: hosts.json parse failed: {}", e);
                    let ts = crate::types::now();
                    let corrupt = dir.join(format!("hosts.json.corrupt-{}", ts));
                    let _ = fs::rename(&path, &corrupt);
                    Registry {
                        path,
                        hosts: Vec::new(),
                    }
                }
            },
            Err(_) => Registry {
                path,
                hosts: Vec::new(),
            },
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_file_name("hosts.json.tmp");
        let data = serde_json::to_vec_pretty(&self.hosts).map_err(std::io::Error::other)?;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&data)?;
        f.sync_all()?;
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(windows)]
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn add(&mut self, name: &str, endpoint: &str, token: &str) -> Result<&Host, String> {
        if !valid_name(name) {
            return Err("name must match [a-z0-9-]{1,32}".into());
        }
        if self.hosts.iter().any(|h| h.name == name) {
            return Err(format!("host '{}' already exists", name));
        }
        let ep = endpoint_normalize(endpoint);
        if !ep.starts_with("http://") && !ep.starts_with("https://") {
            return Err(format!("invalid endpoint: {}", endpoint));
        }
        self.hosts.push(Host {
            id: gen_host_id(),
            name: name.to_string(),
            endpoint: ep,
            token: token.to_string(),
            enabled: true,
            created_at: crate::types::now(),
            last_probe: None,
            failures: 0,
            next_retry_at: None,
        });
        Ok(self.hosts.last().unwrap())
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.hosts.len();
        self.hosts.retain(|h| h.name != name);
        self.hosts.len() != before
    }

    pub fn get(&self, name_or_endpoint: &str) -> Option<&Host> {
        if let Some(h) = self.hosts.iter().find(|h| h.name == name_or_endpoint) {
            return Some(h);
        }
        let ep = endpoint_normalize(name_or_endpoint);
        self.hosts.iter().find(|h| h.endpoint == ep)
    }
}

pub fn probe(h: &Host) -> Probe {
    let at = crate::types::now();
    if let Some(nrt) = h.next_retry_at {
        if nrt > at {
            if let Some(last) = &h.last_probe {
                return Probe {
                    at,
                    ok: last.ok,
                    latency_ms: last.latency_ms,
                    version: last.version.clone(),
                    error: "skipped".into(),
                };
            }
            return Probe {
                at,
                ok: false,
                latency_ms: 0,
                version: String::new(),
                error: "skipped".into(),
            };
        }
    }
    let url = format!("{}/health", h.endpoint.trim_end_matches('/'));
    let start = Instant::now();
    let mut req = ureq::get(&url).timeout(Duration::from_secs(5));
    if !h.token.is_empty() {
        req = req.set("x-roamway-token", &h.token);
    }
    match req.call() {
        Ok(resp) => {
            let body = resp.into_string().unwrap_or_default();
            let version = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_default();
            Probe {
                at,
                ok: true,
                latency_ms: start.elapsed().as_millis() as u64,
                version,
                error: String::new(),
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let _ = resp.into_string();
            Probe {
                at,
                ok: false,
                latency_ms: start.elapsed().as_millis() as u64,
                version: String::new(),
                error: format!("HTTP {}", code),
            }
        }
        Err(e) => Probe {
            at,
            ok: false,
            latency_ms: start.elapsed().as_millis() as u64,
            version: String::new(),
            error: e.to_string(),
        },
    }
}

pub fn dispatch(
    h: &Host,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    let url = format!("{}{}", h.endpoint.trim_end_matches('/'), path);
    let req = match method.to_ascii_uppercase().as_str() {
        "GET" => ureq::get(&url),
        "POST" => ureq::post(&url),
        "DELETE" => ureq::delete(&url),
        "PATCH" => ureq::patch(&url),
        _ => return Err(format!("unsupported method: {}", method)),
    };
    let req = req
        .timeout(Duration::from_secs(60))
        .set("x-roamway-token", &h.token);
    let res = match body {
        Some(b) => req.send_string(b),
        None => req.call(),
    };
    match res {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            Ok((status, text))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Ok((code, text))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_base_maps_wildcard_bind_to_loopback() {
        assert_eq!(
            client_base_from_bind("0.0.0.0:8796"),
            "http://127.0.0.1:8796"
        );
        assert_eq!(
            client_base_from_bind("0.0.0.0:8796\n"),
            "http://127.0.0.1:8796"
        );
        assert_eq!(client_base_from_bind(":8796"), "http://127.0.0.1:8796");
        assert_eq!(client_base_from_bind("[::]:8796"), "http://127.0.0.1:8796");
        assert_eq!(client_base_from_bind("::"), "http://127.0.0.1");
        assert_eq!(client_base_from_bind("*:9100"), "http://127.0.0.1:9100");
        assert_eq!(
            client_base_from_bind("tcp://0.0.0.0:8896"),
            "http://127.0.0.1:8896"
        );
    }

    #[test]
    fn client_base_keeps_concrete_bind() {
        assert_eq!(
            client_base_from_bind("127.0.0.1:8896"),
            "http://127.0.0.1:8896"
        );
        assert_eq!(
            client_base_from_bind("192.0.2.10:8796"),
            "http://192.0.2.10:8796"
        );
        assert_eq!(
            client_base_from_bind("localhost:8796"),
            "http://localhost:8796"
        );
        assert_eq!(client_base_from_bind("[::1]:8796"), "http://[::1]:8796");
        assert_eq!(
            client_base_from_bind("https://nas.example:8443"),
            "https://nas.example:8443"
        );
    }

    #[test]
    fn endpoint_is_wildcard_detects_listen_addresses() {
        for raw in [
            "0.0.0.0:8796",
            "http://0.0.0.0:8796",
            "[::]:8796",
            "::",
            "*:9100",
            "tcp://0.0.0.0:8896",
            ":8796",
        ] {
            assert!(endpoint_is_wildcard(raw), "{} should be wildcard", raw);
        }
        for raw in [
            "127.0.0.1:8896",
            "192.0.2.10:8796",
            "https://nas.example:8443",
            "[::1]:8796",
        ] {
            assert!(!endpoint_is_wildcard(raw), "{} is reachable", raw);
        }
    }

    #[test]
    fn endpoint_normalize_accepts_three_forms() {
        assert_eq!(
            endpoint_normalize("192.0.2.10:8796"),
            "http://192.0.2.10:8796"
        );
        assert_eq!(
            endpoint_normalize("tcp://192.0.2.10:8796"),
            "http://192.0.2.10:8796"
        );
        assert_eq!(
            endpoint_normalize("http://192.0.2.10:8796/"),
            "http://192.0.2.10:8796"
        );
    }

    #[test]
    fn registry_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("roamway-hosts-roundtrip-{}", crate::types::now()));
        let mut reg = Registry::load(&dir);
        let h = reg
            .add("nas", "192.0.2.10:8796", "secret-token")
            .unwrap()
            .clone();
        assert_eq!(h.endpoint, "http://192.0.2.10:8796");
        reg.save().unwrap();
        let loaded = Registry::load(&dir);
        assert_eq!(loaded.hosts.len(), 1);
        let got = &loaded.hosts[0];
        assert_eq!(got.name, "nas");
        assert_eq!(got.endpoint, "http://192.0.2.10:8796");
        assert_eq!(got.token, "secret-token");
        assert!(got.enabled);
        assert_eq!(got.failures, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registry_corrupt_recovers() {
        let dir =
            std::env::temp_dir().join(format!("roamway-hosts-corrupt-{}", crate::types::now()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("hosts.json"), "{ bad json").unwrap();
        let reg = Registry::load(&dir);
        assert!(reg.hosts.is_empty());
        let corrupt_exists = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("hosts.json.corrupt-")
            });
        assert!(corrupt_exists);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_validation() {
        assert!(valid_name("nas"));
        assert!(valid_name("nas-01"));
        assert!(!valid_name("NAS"));
        assert!(!valid_name("has space"));
        assert!(!valid_name(&"x".repeat(33)));
    }

    #[test]
    fn backoff_caps_at_3600() {
        assert_eq!(backoff(1), 30);
        assert_eq!(backoff(2), 60);
        assert_eq!(backoff(8), 3600);
        assert_eq!(backoff(9), 3600);
    }

    #[test]
    fn mask_short_and_long() {
        assert_eq!(mask("abc"), "****");
        assert_eq!(mask("abcdefgh"), "abcd...efgh");
        assert_eq!(mask("0123456789abcdef"), "0123...cdef");
    }

    #[test]
    fn get_by_name_or_endpoint() {
        let mut reg = Registry {
            path: PathBuf::from("unused"),
            hosts: Vec::new(),
        };
        reg.add("nas", "192.0.2.10:8796", "tok").unwrap();
        assert!(reg.get("nas").is_some());
        assert!(reg.get("http://192.0.2.10:8796").is_some());
        assert!(reg.get("192.0.2.10:8796").is_some());
        assert!(reg.get("nope").is_none());
    }
}
