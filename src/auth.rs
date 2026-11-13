// P6 auth primitives and on-disk stores. SHA-256, HMAC-SHA256 and PBKDF2-HMAC-SHA256
// are implemented here (no external crypto crates); OS entropy comes from getrandom,
// which is already in the tree via ureq -> rustls -> ring, keeping builds offline.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const PBKDF2_ITERATIONS: u32 = 200_000;
pub const PBKDF2_ALGO: &str = "pbkdf2-sha256";
pub const LOGIN_TTL_SECS: i64 = 24 * 3600;
pub const PAIR_TTL_SECS: i64 = 60;
pub const APPROVAL_RETENTION_SECS: i64 = 7 * 24 * 3600;
pub const ALL_SCOPES: &[&str] = &["read", "run", "admin"];

pub fn now() -> i64 {
    crate::types::now()
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_block(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (i, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
        w[i] = ((chunk[0] as u32) << 24)
            | ((chunk[1] as u32) << 16)
            | ((chunk[2] as u32) << 8)
            | (chunk[3] as u32);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
        state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
    );
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(((data.len() + 72) / 64 + 1) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    for block in padded.as_chunks::<64>().0 {
        sha256_block(&mut state, block);
    }
    let mut out = [0u8; 32];
    for (i, v) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; 64];
    let key_vec;
    let k = if key.len() > 64 {
        key_vec = sha256(key).to_vec();
        &key_vec[..]
    } else {
        key
    };
    key_block[..k.len()].copy_from_slice(k);
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Vec::with_capacity(64 + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_hash = sha256(&inner);
    let mut outer = Vec::with_capacity(64 + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let blocks = (out.len() as u32).div_ceil(32);
    let mut input = Vec::with_capacity(salt.len() + 4);
    input.extend_from_slice(salt);
    input.extend_from_slice(&[0u8; 4]);
    let mut offset = 0usize;
    for block in 1u32..=blocks {
        let idx = input.len() - 4;
        input[idx..].copy_from_slice(&block.to_be_bytes());
        let mut u = hmac_sha256(password, &input);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for b in 0..32 {
                t[b] ^= u[b];
            }
        }
        let n = out.len().min(offset + 32) - offset;
        out[offset..offset + n].copy_from_slice(&t[..n]);
        offset += n;
    }
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    for i in (0..b.len()).step_by(2) {
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (b[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex_encode(&sha256(data))
}

pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    // 长度差按 usize 全程累加，不能截断成 u8：否则长度差恰为 256 的倍数
    // （且短串以 0 补齐）时 diff 归零，会把不等长输入误判为相等。
    let mut diff = a.len() ^ b.len();
    let n = a.len().max(b.len());
    for i in 0..n {
        diff |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

pub fn ct_eq_str(a: &str, b: &str) -> bool {
    ct_eq(a.as_bytes(), b.as_bytes())
}

pub fn random_hex(nbytes: usize) -> String {
    // OS CSPRNG (getrandom): unpredictable, collision-resistant. Replaces the
    // old SipHash+nanos+counter construction — SipHash is a keyed hash, not a
    // CSPRNG, and its output must not back token plaintext / salts / OTP codes.
    // getrandom is already in the tree via ureq -> rustls -> ring, so this
    // keeps offline builds working with no new downloads.
    let mut buf = vec![0u8; nbytes];
    getrandom::getrandom(&mut buf).unwrap_or_else(|e| panic!("OS CSPRNG unavailable: {e}"));
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(nbytes * 2);
    for b in buf {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn random_digits(len: usize) -> String {
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in random_hex(16).as_bytes() {
            if out.len() == len {
                break;
            }
            if b.is_ascii_digit() {
                out.push(*b as char);
            }
        }
    }
    out
}

pub fn parse_ttl(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(v) = s.strip_suffix('s') {
        return v.trim().parse::<i64>().ok().filter(|n| *n >= 0);
    }
    if let Some(v) = s.strip_suffix('m') {
        return v
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
            .map(|n| n * 60);
    }
    if let Some(v) = s.strip_suffix('h') {
        return v
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
            .map(|n| n * 3600);
    }
    if let Some(v) = s.strip_suffix('d') {
        return v
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
            .map(|n| n * 86400);
    }
    s.parse::<i64>().ok().filter(|n| *n >= 0)
}

pub fn has_scope(scopes: &[String], required: &str) -> bool {
    if scopes.iter().any(|s| s == "admin") {
        return true;
    }
    scopes.iter().any(|s| s == required)
}

pub fn required_scope(method: &str, segs: &[&str]) -> &'static str {
    if method == "GET" || method == "HEAD" || method == "OPTIONS" {
        return match segs.get(1).copied() {
            Some("hosts") | Some("schedules") | Some("approvals") => "admin",
            Some("hub") if segs.get(2) == Some(&"triggers") => "admin",
            _ => "read",
        };
    }
    if method == "POST" {
        match segs {
            ["v1", "runs"] => return "run",
            ["v1", "prune"] => return "run",
            ["v1", "agents"] | ["v1", "agents", "import"] => return "run",
            ["v1", "agents", _, "run"] | ["v1", "agents", _, "detach"] => return "run",
            _ => {}
        }
    }
    "admin"
}

pub fn is_write_method(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

pub fn normalize_host(host: &str) -> String {
    let host = host.trim().to_lowercase();
    if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((h, _)) => h.to_string(),
            None => host,
        }
    } else if host.matches(':').count() == 1 {
        host.rsplit_once(':')
            .map(|(h, _)| h.to_string())
            .unwrap_or(host)
    } else {
        host
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

pub fn host_allowed(allow_hosts: &[String], bind_host: &str, host_header: Option<&str>) -> bool {
    let Some(raw) = host_header else { return true };
    let host = normalize_host(raw);
    if !allow_hosts.is_empty() {
        return allow_hosts.iter().any(|h| normalize_host(h) == host);
    }
    if is_loopback_host(&host) {
        return true;
    }
    normalize_host(bind_host) == host
}

pub fn origin_allowed(allow_origins: &[String], origin: Option<&str>) -> Option<String> {
    let origin = origin?;
    for allowed in allow_origins {
        if allowed == "*" {
            return Some("*".into());
        }
        if allowed == origin {
            return Some(allowed.clone());
        }
    }
    None
}

pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Atomic replace: write a sibling temp file, fsync, then rename over the
    // target. A crash mid-write can no longer leave a truncated tokens.json /
    // approvals.json — readers see either the old or the new file, never a
    // half-written one. The temp name carries pid+seq so concurrent writers
    // (CLI + daemon) never clobber each other's temp file.
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "state".into()),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PasswordRecord {
    pub algo: String,
    pub iterations: u32,
    pub salt_hex: String,
    pub hash_hex: String,
}

impl PasswordRecord {
    pub fn new(password: &str) -> PasswordRecord {
        let salt = random_hex(16);
        PasswordRecord {
            algo: PBKDF2_ALGO.into(),
            iterations: PBKDF2_ITERATIONS,
            salt_hex: salt.clone(),
            hash_hex: Self::derive(password, &salt, PBKDF2_ITERATIONS),
        }
    }

    fn derive(password: &str, salt_hex: &str, iterations: u32) -> String {
        let salt = hex_decode(salt_hex).unwrap_or_default();
        let mut out = [0u8; 32];
        pbkdf2_hmac_sha256(password.as_bytes(), &salt, iterations, &mut out);
        hex_encode(&out)
    }

    pub fn verify(&self, password: &str) -> bool {
        if self.algo != PBKDF2_ALGO {
            return false;
        }
        let Some(salt) = hex_decode(&self.salt_hex) else {
            return false;
        };
        let Some(expected) = hex_decode(&self.hash_hex) else {
            return false;
        };
        let mut out = [0u8; 32];
        pbkdf2_hmac_sha256(password.as_bytes(), &salt, self.iterations, &mut out);
        ct_eq(&out, &expected)
    }
}

#[derive(Default)]
pub struct PasswordStore {
    pub path: PathBuf,
    pub record: Option<PasswordRecord>,
}

impl PasswordStore {
    pub fn load(dir: &Path) -> PasswordStore {
        let path = dir.join("password.json");
        let record = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<PasswordRecord>(&b).ok());
        PasswordStore { path, record }
    }

    pub fn set_password(&mut self, password: &str) -> Result<(), String> {
        if password.is_empty() {
            return Err("password must not be empty".into());
        }
        let record = PasswordRecord::new(password);
        let bytes = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
        write_private(&self.path, &bytes).map_err(|e| e.to_string())?;
        self.record = Some(record);
        Ok(())
    }

    pub fn verify(&self, password: &str) -> bool {
        self.record
            .as_ref()
            .map(|r| r.verify(password))
            .unwrap_or(false)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TokenRecord {
    pub id: String,
    pub name: String,
    pub sha256: String,
    pub created_at: i64,
    pub last_used_at: i64,
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Default)]
pub struct TokenStore {
    pub path: PathBuf,
    pub tokens: Vec<TokenRecord>,
    /// tokens.json 落盘时的 mtime；读取路径检测到磁盘变化即重载，
    /// 使 CLI revoke / 外部改文件无需重启立即生效。
    mtime: Option<std::time::SystemTime>,
}

impl TokenStore {
    pub fn load(dir: &Path) -> TokenStore {
        let path = dir.join("tokens.json");
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let tokens = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<TokenRecord>>(&b).ok())
            .unwrap_or_default();
        TokenStore {
            path,
            tokens,
            mtime,
        }
    }

    /// 磁盘 mtime 与内存不一致时重载。重载后保留内存里更新的 last_used_at，
    /// 不丢未落盘的新鲜度；删除的 token 随磁盘消失（revoke 立即生效）。
    fn reload_if_changed(&mut self) {
        let disk_mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if disk_mtime == self.mtime {
            return;
        }
        let mut disk = std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<TokenRecord>>(&b).ok())
            .unwrap_or_default();
        for t in disk.iter_mut() {
            if let Some(old) = self.tokens.iter().find(|o| o.id == t.id) {
                if old.last_used_at > t.last_used_at {
                    t.last_used_at = old.last_used_at;
                }
            }
        }
        self.tokens = disk;
        self.mtime = disk_mtime;
    }

    pub fn save(&mut self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.tokens).map_err(|e| e.to_string())?;
        write_private(&self.path, &bytes).map_err(|e| e.to_string())?;
        self.mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        Ok(())
    }

    pub fn create(
        &mut self,
        name: Option<String>,
        scopes: Vec<String>,
        ttl_secs: Option<i64>,
    ) -> Result<(String, String), String> {
        let plain = random_hex(32);
        let id = format!("tok_{}_{}", now(), random_hex(8));
        let name = name.unwrap_or_else(|| format!("token-{}", id));
        let record = TokenRecord {
            id: id.clone(),
            name,
            sha256: sha256_hex(plain.as_bytes()),
            created_at: now(),
            last_used_at: now(),
            scopes,
            expires_at: ttl_secs.and_then(|s| if s > 0 { Some(now() + s) } else { None }),
        };
        self.tokens.push(record);
        if let Err(e) = self.save() {
            self.tokens.pop();
            return Err(e);
        }
        Ok((id, plain))
    }

    pub fn revoke(&mut self, id: &str) -> Result<bool, String> {
        let before = self.tokens.len();
        self.tokens.retain(|t| t.id != id);
        let changed = self.tokens.len() != before;
        if changed {
            self.save()?;
        }
        Ok(changed)
    }

    /// 是否存在有效 token（先触发热重载）。daemon 启动时为空、之后 CLI 建 token
    /// 的场景必须靠它刷新，否则 authenticate 的 Open 判定会永久放行。
    pub fn has_tokens(&mut self) -> bool {
        self.reload_if_changed();
        !self.tokens.is_empty()
    }

    pub fn list(&mut self) -> Vec<TokenRecord> {
        self.reload_if_changed();
        self.tokens.clone()
    }

    pub fn resolve(&mut self, plain: &str) -> Option<TokenRecord> {
        self.reload_if_changed();
        let sha = sha256_hex(plain.as_bytes());
        let current = now();
        let mut found: Option<TokenRecord> = None;
        for t in self.tokens.iter_mut() {
            if let Some(exp) = t.expires_at {
                if exp <= current {
                    continue;
                }
            }
            if ct_eq_str(&t.sha256, &sha) {
                t.last_used_at = current;
                found = Some(t.clone());
                break;
            }
        }
        if found.is_some() {
            if let Err(e) = self.save() {
                eprintln!("roamway: token save failed: {}", e);
            }
        }
        found
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PairRecord {
    pub code: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Default)]
pub struct PairingStore {
    pub path: PathBuf,
    pub record: Option<PairRecord>,
}

impl PairingStore {
    pub fn load(dir: &Path) -> PairingStore {
        let path = dir.join("pairing.json");
        let record = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<PairRecord>(&b).ok())
            .filter(|r| r.expires_at > now());
        PairingStore { path, record }
    }

    pub fn save(&self) {
        if let Some(r) = &self.record {
            if let Ok(bytes) = serde_json::to_vec_pretty(r) {
                let _ = write_private(&self.path, &bytes);
            }
        }
    }

    pub fn generate(&mut self, ttl_secs: i64) -> String {
        let code = random_digits(6);
        let created = now();
        self.record = Some(PairRecord {
            code: code.clone(),
            created_at: created,
            expires_at: created + ttl_secs,
        });
        self.save();
        code
    }

    pub fn consume(&mut self, code: &str) -> Result<(), String> {
        let Some(r) = self.record.clone() else {
            return Err("no active pairing code".into());
        };
        if now() > r.expires_at {
            self.record = None;
            let _ = std::fs::remove_file(&self.path);
            return Err("pairing code expired".into());
        }
        if ct_eq_str(&r.code, code.trim()) {
            self.record = None;
            let _ = std::fs::remove_file(&self.path);
            Ok(())
        } else {
            Err("invalid pairing code".into())
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApprovalRecord {
    pub id: String,
    pub ts: i64,
    pub method: String,
    pub path: String,
    pub body_hash: String,
    pub token_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

#[derive(Default)]
pub struct ApprovalStore {
    pub path: PathBuf,
    pub records: Vec<ApprovalRecord>,
}

impl ApprovalStore {
    pub fn load(dir: &Path) -> ApprovalStore {
        let path = dir.join("approvals.json");
        let mut records: Vec<ApprovalRecord> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let before = records.len();
        records.retain(|r| r.ts >= now() - APPROVAL_RETENTION_SECS || r.status == "pending");
        let store = ApprovalStore { path, records };
        if store.records.len() != before {
            store.save();
        }
        store
    }

    pub fn save(&self) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.records) {
            let _ = write_private(&self.path, &bytes);
        }
    }

    pub fn create(
        &mut self,
        method: &str,
        path: &str,
        body: &[u8],
        token_id: &str,
    ) -> ApprovalRecord {
        let record = ApprovalRecord {
            id: format!("apr_{}_{}", now(), random_hex(8)),
            ts: now(),
            method: method.to_string(),
            path: path.to_string(),
            body_hash: sha256_hex(body),
            token_id: token_id.to_string(),
            status: "pending".into(),
            decided_at: None,
            decided_by: None,
        };
        self.records.push(record.clone());
        self.records
            .retain(|r| r.ts >= now() - APPROVAL_RETENTION_SECS || r.status == "pending");
        self.save();
        record
    }

    pub fn get(&self, id: &str) -> Option<&ApprovalRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    pub fn list(&self, all: bool) -> Vec<&ApprovalRecord> {
        let mut out: Vec<&ApprovalRecord> = if all {
            self.records.iter().collect()
        } else {
            self.records
                .iter()
                .filter(|r| r.status == "pending")
                .collect()
        };
        out.sort_by_key(|a| std::cmp::Reverse(a.ts));
        out
    }

    pub fn decide(&mut self, id: &str, status: &str, decided_by: &str) -> bool {
        let Some(r) = self.records.iter_mut().find(|r| r.id == id) else {
            return false;
        };
        if r.status != "pending" {
            return false;
        }
        r.status = match status {
            "approved" | "denied" => status.to_string(),
            _ => return false,
        };
        r.decided_at = Some(now());
        r.decided_by = Some(decided_by.to_string());
        self.save();
        true
    }

    pub fn verify_replay(
        &self,
        id: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(), &'static str> {
        let Some(r) = self.get(id) else {
            return Err("approval not found");
        };
        if r.status != "approved" {
            return Err("approval not approved");
        }
        if r.method != method || r.path != path || r.body_hash != sha256_hex(body) {
            return Err("approval body mismatch");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_encode(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_case2() {
        let h = hmac_sha256("Jefe".as_bytes(), b"what do ya want for nothing?");
        assert_eq!(
            hex_encode(&h),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn pbkdf2_sha256_known_vectors() {
        let mut dk = [0u8; 32];
        pbkdf2_hmac_sha256(b"password", b"salt", 1, &mut dk);
        assert_eq!(
            hex_encode(&dk),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        pbkdf2_hmac_sha256(b"password", b"salt", 2, &mut dk);
        assert_eq!(
            hex_encode(&dk),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }

    #[test]
    fn ct_eq_is_constant_time_and_correct() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"a"));
        assert!(ct_eq(b"", b""));
        assert!(ct_eq_str("tok", "tok"));
        assert!(!ct_eq_str("tok", "tik"));
    }

    /// 回归：长度差恰为 256 的倍数且短串以 0 补齐时，旧实现把长度差截断成
    /// u8 会归零，误判不等长输入为相等。修复后必须判不等。
    #[test]
    fn ct_eq_rejects_length_diff_multiple_of_256() {
        let short = vec![0u8; 0];
        let long = vec![0u8; 256];
        assert!(!ct_eq(&short, &long));
        assert!(!ct_eq(&long, &short));
        // 非零内容同样不能因长度差 256 而误判
        let a = vec![7u8; 16];
        let mut b = vec![7u8; 16];
        b.extend_from_slice(&vec![0u8; 256]);
        assert!(!ct_eq(&a, &b));
        // 等长仍正确
        assert!(ct_eq(&long, &vec![0u8; 256]));
    }

    #[test]
    fn parse_ttl_units() {
        assert_eq!(parse_ttl("24h"), Some(86400));
        assert_eq!(parse_ttl("7d"), Some(604800));
        assert_eq!(parse_ttl("3600"), Some(3600));
        assert_eq!(parse_ttl("60s"), Some(60));
        assert_eq!(parse_ttl("30m"), Some(1800));
        assert_eq!(parse_ttl("0"), Some(0));
        assert_eq!(parse_ttl("abc"), None);
        assert_eq!(parse_ttl("-5h"), None);
    }

    #[test]
    fn scope_matrix_six_groups() {
        assert_eq!(required_scope("GET", &["v1", "runs"]), "read");
        assert_eq!(required_scope("POST", &["v1", "runs"]), "run");
        assert_eq!(required_scope("POST", &["v1", "agents"]), "run");
        assert_eq!(
            required_scope("POST", &["v1", "runs", "x", "stop"]),
            "admin"
        );
        assert_eq!(required_scope("DELETE", &["v1", "schedules", "x"]), "admin");
        assert_eq!(required_scope("POST", &["v1", "agents", "x", "run"]), "run");
    }

    #[test]
    fn scope_hierarchy_admin_includes_all() {
        let scopes = vec!["admin".to_string()];
        assert!(has_scope(&scopes, "read"));
        assert!(has_scope(&scopes, "run"));
        assert!(has_scope(&scopes, "admin"));
        let run_only = vec!["run".to_string()];
        assert!(!has_scope(&run_only, "admin"));
        assert!(has_scope(&run_only, "run"));
    }

    #[test]
    fn admin_read_routes_require_admin_scope() {
        assert_eq!(required_scope("GET", &["v1", "hosts"]), "admin");
        assert_eq!(required_scope("GET", &["v1", "schedules"]), "admin");
        assert_eq!(required_scope("GET", &["v1", "schedules", "x"]), "admin");
        assert_eq!(required_scope("GET", &["v1", "approvals"]), "admin");
        assert_eq!(required_scope("GET", &["v1", "runs"]), "read");
    }

    #[test]
    fn host_allowlist_hit_and_miss() {
        let empty: Vec<String> = vec![];
        assert!(host_allowed(&empty, "127.0.0.1", Some("127.0.0.1:8796")));
        assert!(host_allowed(&empty, "0.0.0.0", Some("localhost:8796")));
        assert!(!host_allowed(&empty, "127.0.0.1", Some("evil.com:8796")));
        let allow = vec!["api.example.com".to_string()];
        assert!(host_allowed(
            &allow,
            "127.0.0.1",
            Some("api.example.com:8796")
        ));
        assert!(!host_allowed(&allow, "127.0.0.1", Some("evil.com:8796")));
        assert!(host_allowed(&empty, "127.0.0.1", None));
    }

    #[test]
    fn origin_cors_only_when_configured_and_matching() {
        let none: Vec<String> = vec![];
        assert_eq!(origin_allowed(&none, Some("http://x.example")), None);
        let some = vec!["http://x.example".to_string()];
        assert_eq!(
            origin_allowed(&some, Some("http://x.example")),
            Some("http://x.example".into())
        );
        assert_eq!(origin_allowed(&some, Some("http://evil.example")), None);
        let wild = vec!["*".to_string()];
        assert_eq!(
            origin_allowed(&wild, Some("http://x.example")),
            Some("*".into())
        );
    }

    #[test]
    fn token_store_roundtrip_and_expiry() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-token-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = TokenStore::load(&dir);
        // TTL 给足 60 秒：create 与 resolve 之间若跨秒（并行测试 + 慢 IO 的 CI runner），
        // 1 秒 TTL 会造成假过期。过期路径下面用手写 expires_at 覆盖，语义不变。
        let (id, plain) = store
            .create(Some("n".into()), vec!["run".into()], Some(60))
            .unwrap();
        assert!(id.starts_with("tok_"));
        assert!(store.resolve(&plain).is_some());
        assert!(store.resolve("wrong").is_none());
        store.tokens[0].expires_at = Some(now() - 1);
        assert!(
            store.resolve(&plain).is_none(),
            "expired token must be rejected"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn random_hex_csprng_format_and_uniqueness() {
        // 回归：random_hex 必须输出精确长度的小写 hex，且批量调用不重复
        // （CSPRNG 替换 SipHash 构造后，格式与唯一性契约不变）。
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            let s = random_hex(16);
            assert_eq!(s.len(), 32);
            assert!(s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert!(seen.insert(s));
        }
        assert_eq!(random_hex(0).len(), 0);
        assert_eq!(random_hex(1).len(), 2);
        assert_eq!(random_hex(32).len(), 64);
    }

    #[test]
    fn token_and_approval_ids_unique_in_burst() {
        // 回归：旧 ID 后缀只有 24bit 且同刻可重复，token 吊销/审批查找会被碰撞破坏。
        let dir = std::env::temp_dir().join(format!("roamway-idburst-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut tokens = TokenStore::load(&dir);
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let (_plain, id) = tokens.create(None, vec!["read".into()], None).unwrap();
            assert!(ids.insert(id));
        }
        let mut approvals = ApprovalStore::load(&dir);
        let mut aids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let rec = approvals.create("POST", "/v1/runs", b"{}", "tok");
            assert!(aids.insert(rec.id));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_store_revoke() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-revoke-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = TokenStore::load(&dir);
        let (id, plain) = store
            .create(Some("n".into()), vec!["read".into()], None)
            .unwrap();
        assert!(store.revoke(&id).unwrap());
        assert!(!store.revoke("missing").unwrap());
        assert!(store.resolve(&plain).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_store_reloads_after_external_change() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-reload-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = TokenStore::load(&dir);
        let (_id, plain) = store
            .create(Some("n".into()), vec!["read".into()], None)
            .unwrap();
        assert!(store.resolve(&plain).is_some());
        // 外部直接改写 tokens.json：删掉该 token（等价 CLI revoke）。
        std::fs::write(dir.join("tokens.json"), "[]").unwrap();
        // 不重启：再次 resolve 必须失败（热重载生效）。
        assert!(store.resolve(&plain).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_tokens_tracks_external_create_and_revoke() {
        // 回归：daemon 以空 token 库启动（Open 模式），之后 CLI 建 token，
        // has_tokens 必须经热重载变 true，否则 authenticate 永久放行任意请求。
        let dir = std::env::temp_dir().join(format!("roamway-auth-hastokens-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut daemon_view = TokenStore::load(&dir);
        assert!(!daemon_view.has_tokens());
        // 另一进程（CLI）建 token 落盘。
        let mut cli = TokenStore::load(&dir);
        let (id, _plain) = cli
            .create(Some("n".into()), vec!["read".into()], None)
            .unwrap();
        assert!(daemon_view.has_tokens());
        // CLI revoke 后，daemon 视图必须回到 false。
        assert!(cli.revoke(&id).unwrap());
        assert!(!daemon_view.has_tokens());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_store_save_fails_on_unwritable_path() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-noparent-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        // 父目录不存在：write_private 无法创建文件，save 必须返回 Err。
        let mut store = TokenStore {
            path: dir.join("missing-subdir").join("tokens.json"),
            tokens: vec![],
            mtime: None,
        };
        assert!(store.save().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairing_code_one_time_use() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-pair-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("pairing.json");
        let record = PairRecord {
            code: "123456".into(),
            created_at: now(),
            expires_at: now() + 60,
        };
        let _ = write_private(&path, &serde_json::to_vec_pretty(&record).unwrap());
        let mut store = PairingStore::load(&dir);
        assert_eq!(store.consume("123456"), Ok(()));
        assert!(store.consume("123456").is_err());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairing_code_expiry_and_mismatch() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-pairx-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = PairingStore::load(&dir);
        store.record = Some(PairRecord {
            code: "000000".into(),
            created_at: now() - 120,
            expires_at: now() - 60,
        });
        assert!(store.consume("000000").is_err());
        store.record = Some(PairRecord {
            code: "111111".into(),
            created_at: now(),
            expires_at: now() + 60,
        });
        assert!(store.consume("999999").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approval_body_hash_mismatch_conflict() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-apr-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = ApprovalStore::load(&dir);
        let rec = store.create("POST", "/v1/runs", b"{\"prompt\":\"a\"}", "tok");
        assert!(store.decide(&rec.id, "approved", "cli"));
        assert_eq!(
            store.verify_replay(&rec.id, "POST", "/v1/runs", b"{\"prompt\":\"a\"}"),
            Ok(())
        );
        assert_eq!(
            store.verify_replay(&rec.id, "POST", "/v1/runs", b"{\"prompt\":\"b\"}"),
            Err("approval body mismatch")
        );
        assert_eq!(
            store.verify_replay("missing", "POST", "/v1/runs", b"{}"),
            Err("approval not found")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approval_retention_cleans_old_records() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-aprc-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("approvals.json");
        let old = ApprovalRecord {
            id: "old".into(),
            ts: now() - APPROVAL_RETENTION_SECS - 10,
            method: "POST".into(),
            path: "/v1/runs".into(),
            body_hash: sha256_hex(b"x"),
            token_id: "t".into(),
            status: "approved".into(),
            decided_at: None,
            decided_by: None,
        };
        let pending_old = ApprovalRecord {
            id: "pending-old".into(),
            ts: now() - APPROVAL_RETENTION_SECS - 10,
            method: "POST".into(),
            path: "/v1/runs".into(),
            body_hash: sha256_hex(b"x"),
            token_id: "t".into(),
            status: "pending".into(),
            decided_at: None,
            decided_by: None,
        };
        let _ = write_private(
            &path,
            &serde_json::to_vec_pretty(&vec![old, pending_old]).unwrap(),
        );
        let store = ApprovalStore::load(&dir);
        assert!(
            store.get("old").is_none(),
            "expired approved records are pruned"
        );
        assert!(
            store.get("pending-old").is_some(),
            "pending records are kept until decided"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn private_files_are_written_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("roamway-auth-mode-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("password.json");
        write_private(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn password_hash_never_stores_plaintext() {
        let dir = std::env::temp_dir().join(format!("roamway-auth-pass-{}", now()));
        let _ = std::fs::create_dir_all(&dir);
        let mut store = PasswordStore::load(&dir);
        store.set_password("s3cret").unwrap();
        let raw = std::fs::read_to_string(&store.path).unwrap();
        assert!(!raw.contains("s3cret"));
        assert!(raw.contains("\"algo\": \"pbkdf2-sha256\""));
        assert!(store.verify("s3cret"));
        assert!(!store.verify("wrong"));
        let _ = std::fs::remove_file(&store.path);
        let reloaded = PasswordStore::load(&dir);
        assert!(!reloaded.verify("s3cret"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
