// 本地状态快照与回滚：零依赖 SHA-256 + manifest 校验 + daemon 存活探测。
// 备份只复制 state.json / config.json / schedules.json，绝不触碰 token/pairing/passwords。

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

const BACKUP_FORMAT: &str = "roamway-backup/1";
const BACKUP_FILES: [&str; 3] = ["state.json", "config.json", "schedules.json"];
const RESTORE_ORDER: [&str; 3] = ["config.json", "schedules.json", "state.json"];

#[derive(Debug, Clone)]
pub struct BackupError {
    code: i32,
    msg: String,
}

impl BackupError {
    pub fn message(&self) -> String {
        self.msg.clone()
    }

    pub fn exit_code(&self) -> i32 {
        self.code
    }
}

fn err(code: i32, msg: impl Into<String>) -> BackupError {
    BackupError {
        code,
        msg: msg.into(),
    }
}

#[derive(Debug)]
pub struct BackupResult {
    pub dir: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub pruned: usize,
}

#[derive(Debug)]
pub struct RestoreResult {
    pub files: Vec<String>,
    pub from: String,
}

#[derive(Serialize, Deserialize)]
struct ManifestFile {
    name: String,
    bytes: u64,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    created_at: i64,
    version: String,
    files: Vec<ManifestFile>,
}

// SHA-256（零依赖自实现，RFC 6234）。
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

fn sha256(data: &[u8]) -> [u8; 32] {
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

    let mut out = [0u8; 32];
    for (i, v) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(64);
    for b in sha256(bytes) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 备份目录时间戳：固定时区偏移下的本地时间（与既有时间渲染一致）。
fn backup_stamp(ts: i64, tz_offset_hours: i64) -> String {
    let local = ts + tz_offset_hours * 3600;
    let days = local.div_euclid(86400);
    let secs = local.rem_euclid(86400);
    let (y, m, d) = crate::types::civil_from_days(days);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        y,
        m,
        d,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn unique_backup_dir(out: &Path, ts: i64, tz_offset_hours: i64) -> PathBuf {
    let stamp = backup_stamp(ts, tz_offset_hours);
    let first = out.join(format!("roamway-backup-{}", stamp));
    if !first.exists() {
        return first;
    }
    let mut n = 2u64;
    loop {
        let alt = out.join(format!("roamway-backup-{}-{}", stamp, n));
        if !alt.exists() {
            return alt;
        }
        n += 1;
    }
}

fn prune_backup_dirs(out: &Path, keep: usize) -> Result<usize, BackupError> {
    if keep == 0 {
        return Ok(0);
    }
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    let entries = fs::read_dir(out)
        .map_err(|e| err(1, format!("backup: cannot list {}: {}", out.display(), e)))?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("roamway-backup-") && e.path().is_dir() {
            dirs.push((name, e.path()));
        }
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    let excess = dirs.len().saturating_sub(keep);
    let mut pruned = 0usize;
    for (_, p) in dirs.into_iter().take(excess) {
        fs::remove_dir_all(&p)
            .map_err(|e| err(1, format!("backup: cannot prune {}: {}", p.display(), e)))?;
        pruned += 1;
    }
    Ok(pruned)
}

/// 生成备份：只复制三件套并写 manifest；失败时删除半成品目录并返回 exit 1 语义。
pub fn backup(
    data: &Path,
    out: &Path,
    keep: usize,
    tz_offset_hours: i64,
) -> Result<BackupResult, BackupError> {
    fs::create_dir_all(out)
        .map_err(|e| err(1, format!("backup: cannot create {}: {}", out.display(), e)))?;
    let ts = crate::types::now();
    let dir = unique_backup_dir(out, ts, tz_offset_hours);
    if let Err(e) = fs::create_dir_all(&dir) {
        return Err(err(
            1,
            format!("backup: cannot create {}: {}", dir.display(), e),
        ));
    }
    let result = build_backup_dir(data, &dir, ts);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    let (files, bytes, manifest) = result.unwrap();
    let manifest_bytes = match serde_json::to_vec_pretty(&manifest) {
        Ok(b) => b,
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            return Err(err(1, format!("backup: bad manifest: {}", e)));
        }
    };
    if let Err(e) = fs::write(dir.join("manifest.json"), &manifest_bytes) {
        let _ = fs::remove_dir_all(&dir);
        return Err(err(1, format!("backup: cannot write manifest: {}", e)));
    }
    let pruned = prune_backup_dirs(out, keep)?;
    Ok(BackupResult {
        dir,
        files,
        bytes,
        pruned,
    })
}

fn build_backup_dir(
    data: &Path,
    dir: &Path,
    ts: i64,
) -> Result<(usize, u64, Manifest), BackupError> {
    let mut files = Vec::new();
    let mut total = 0u64;
    for name in BACKUP_FILES {
        let src = data.join(name);
        if !src.is_file() {
            continue;
        }
        let bytes = fs::read(&src)
            .map_err(|e| err(1, format!("backup: cannot read {}: {}", src.display(), e)))?;
        let dst = dir.join(name);
        fs::write(&dst, &bytes)
            .map_err(|e| err(1, format!("backup: cannot write {}: {}", dst.display(), e)))?;
        total += bytes.len() as u64;
        files.push(ManifestFile {
            name: name.to_string(),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }
    let manifest = Manifest {
        format: BACKUP_FORMAT.to_string(),
        created_at: ts,
        version: env!("CARGO_PKG_VERSION").to_string(),
        files,
    };
    Ok((manifest.files.len(), total, manifest))
}

/// daemon 活体证据：pid / heartbeat / bind 端口，restore 按此区分报错文案。
#[derive(Debug, Clone)]
pub enum Alive {
    Pid(u32),
    Heartbeat(u64),
    Bind(String),
}

/// 读取 `<data>/roamway.pid` 的内容（仅数字与换行），返回所属 pid。
pub fn pid_file_owner(data: &Path) -> Option<u32> {
    let raw = fs::read_to_string(data.join("roamway.pid")).ok()?;
    raw.trim().parse::<u32>().ok().filter(|p| *p > 0)
}

/// 写 `<pid>\n` 到 `<data>/roamway.pid`：tmp + rename，返回写入的 pid。
pub fn write_pid_file(data: &Path) -> std::io::Result<u32> {
    let pid = std::process::id();
    let p = data.join("roamway.pid");
    let tmp = data.join("roamway.pid.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(format!("{}\n", pid).as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &p)?;
    let _ = sync_parent_dir(&p);
    Ok(pid)
}

/// 仅当 pid 文件内容仍是本进程 pid 时才删除，避免误删别人（新 daemon）写的文件。
pub fn clear_pid_file(data: &Path) {
    if pid_file_owner(data) == Some(std::process::id()) {
        let _ = fs::remove_file(data.join("roamway.pid"));
    }
}

const WRITER_LOCK_FILE: &str = "roamway.lock";

fn writer_lock_path(data: &Path) -> PathBuf {
    data.join(WRITER_LOCK_FILE)
}

/// 解析 `<pid> <bind> <iso>`；解析不出来（0 字节/乱码）视为 stale。
fn read_writer_lock(lock_path: &Path) -> Option<(u32, String)> {
    let raw = fs::read_to_string(lock_path).ok()?;
    let mut fields = raw.splitn(3, ' ');
    let owner_pid: u32 = fields.next()?.trim().parse().ok()?;
    let owner_bind = fields.next()?.trim().to_string();
    if owner_bind.is_empty() {
        return None;
    }
    Some((owner_pid, owner_bind))
}

/// 只读查看当前锁 owner：`(pid, bind)`，无锁/坏文件返回 `None`。
pub fn writer_lock_owner(data: &Path) -> Option<(u32, String)> {
    read_writer_lock(&writer_lock_path(data))
}

pub fn try_acquire_writer_with(
    data: &Path,
    bind: &str,
    self_pid: u32,
    alive: &dyn Fn(u32) -> bool,
) -> Result<u32, (u32, String)> {
    let lock_path = writer_lock_path(data);
    for _ in 0..2 {
        let mut lock_file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(_) => match read_writer_lock(&lock_path) {
                Some((owner_pid, owner_bind)) if alive(owner_pid) && owner_pid != self_pid => {
                    return Err((owner_pid, owner_bind));
                }
                _ => {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
            },
        };
        let content = format!(
            "{} {} {}\n",
            self_pid,
            bind,
            crate::types::iso(crate::types::now())
        );
        if let Err(e) = lock_file.write_all(content.as_bytes()) {
            eprintln!("roamway: write writer lock failed: {}", e);
            let _ = fs::remove_file(&lock_path);
            continue;
        }
        if let Err(e) = lock_file.sync_all() {
            eprintln!("roamway: write writer lock failed: {}", e);
            let _ = fs::remove_file(&lock_path);
            continue;
        }
        return Ok(self_pid);
    }
    match read_writer_lock(&lock_path) {
        Some((owner_pid, owner_bind)) => Err((owner_pid, owner_bind)),
        None => Err((0, String::new())),
    }
}

pub fn acquire_writer(data: &Path, bind: &str) -> Result<u32, (u32, String)> {
    let alive: &dyn Fn(u32) -> bool = &(process_alive as fn(u32) -> bool);
    try_acquire_writer_with(data, bind, std::process::id(), alive)
}

pub fn force_acquire_writer(data: &Path, bind: &str) -> std::io::Result<u32> {
    let self_pid = std::process::id();
    let lock_path = writer_lock_path(data);
    let mut lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&lock_path)?;
    lock_file.write_all(
        format!(
            "{} {} {}\n",
            self_pid,
            bind,
            crate::types::iso(crate::types::now())
        )
        .as_bytes(),
    )?;
    lock_file.sync_all()?;
    Ok(self_pid)
}

pub fn release_writer(data: &Path) {
    let self_pid = std::process::id();
    if writer_lock_owner(data).map(|(owner_pid, _)| owner_pid) == Some(self_pid) {
        let _ = fs::remove_file(writer_lock_path(data));
    }
}

pub fn writer_busy_message(data: &Path, owner_pid: u32, owner_bind: &str) -> String {
    format!(
        "roamway: data dir {} is already served by pid {} (bind {}); stop it first or pass --force",
        data.display(),
        owner_pid,
        owner_bind
    )
}

pub fn writer_takeover_message(data: &Path, owner_pid: u32) -> String {
    format!(
        "roamway: --force taking over data dir {} from pid {}; two writers will corrupt state.json",
        data.display(),
        owner_pid
    )
}

#[cfg(target_os = "linux")]
pub fn process_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn process_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 在 `tasklist /FO CSV /NH` 输出里精确匹配 PID 字段。
///
/// 独立成纯函数是刻意的：解析坑来自本地化 INFO 行，而 NAS 门禁跑在 Linux 上看不见
/// `#[cfg(windows)]` 分支——只有纯函数才能在任意平台被单测盯住。
#[cfg(any(windows, test))]
fn pid_field_matches(csv: &str, pid: u32) -> bool {
    let want = pid.to_string();
    csv.lines().any(|line| match line.split(',').nth(1) {
        Some(field) => field.trim().trim_matches('"').trim() == want,
        None => false,
    })
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    // 必须用 CSV + 字段精确比较：`tasklist /NH` 在无匹配时会打印一条本地化 INFO 行
    // （zh-CN：`信息: 没有运行的任务匹配指定标准。`），判 "No tasks" 会让任何合法但已死的 pid
    // 都被当成活体 —— 中文 Windows 上 restore 因此永远要求 --force。CSV 的 INFO 行只有一个字段。
    let out = match std::process::Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {}", pid))
        .arg("/FO")
        .arg("CSV")
        .arg("/NH")
        .output()
    {
        Ok(o) => o,
        Err(_) => return false,
    };
    pid_field_matches(&String::from_utf8_lossy(&out.stdout), pid)
}

/// `<data>/heartbeat` 距今秒数；缺失或时钟异常时无证据。
fn heartbeat_age_secs(data: &Path) -> Option<u64> {
    let modified = fs::metadata(data.join("heartbeat")).ok()?.modified().ok()?;
    let now = std::time::SystemTime::now();
    Some(
        now.duration_since(modified)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
}

/// 解析 `host:port`；支持 `[::1]:8796`。空 host / wildcard 不在此处归一化。
fn parse_host_port(bind: &str) -> Option<(String, u16)> {
    if let Some(rest) = bind.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host.to_string(), port.parse::<u16>().ok()?));
    }
    let (host, port) = bind.rsplit_once(':')?;
    Some((host.to_string(), port.parse::<u16>().ok()?))
}

/// `<data>/bind` 格式合法且 150ms 内可连接时返回实际探测地址；无证据则 None。
fn open_bind_probe(data: &Path) -> Option<String> {
    let raw = fs::read_to_string(data.join("bind")).ok()?;
    let (host, port) = parse_host_port(raw.trim())?;
    let host = if matches!(host.as_str(), "0.0.0.0" | "::" | "") {
        "127.0.0.1".to_string()
    } else {
        host
    };
    let addr = format!("{}:{}", host, port);
    let socket = (host.as_str(), port).to_socket_addrs().ok()?.next()?;
    match TcpStream::connect_timeout(&socket, Duration::from_millis(150)) {
        Ok(_) => Some(addr),
        Err(_) => None,
    }
}

/// 唯一判定实现：pid 存活 → 新鲜 heartbeat → bind 端口可连，短路返回首个证据。
pub fn daemon_probe(data: &Path) -> Option<Alive> {
    if let Some(pid) = pid_file_owner(data) {
        if process_alive(pid) {
            return Some(Alive::Pid(pid));
        }
    }
    if let Some(age) = heartbeat_age_secs(data) {
        if age <= 60 {
            return Some(Alive::Heartbeat(age));
        }
    }
    if let Some(addr) = open_bind_probe(data) {
        return Some(Alive::Bind(addr));
    }
    None
}

/// 兼容薄封装：只有 pid 证据时才返回 pid，行为与旧 `daemon_alive` 一致。
pub fn daemon_alive(data: &Path) -> Option<u32> {
    match daemon_probe(data) {
        Some(Alive::Pid(pid)) => Some(pid),
        _ => None,
    }
}

fn alive_error_message(alive: &Alive) -> String {
    match alive {
        Alive::Pid(pid) => {
            format!(
                "restore: daemon looks alive (pid {}); stop it or pass --force",
                pid
            )
        }
        Alive::Heartbeat(age) => {
            format!(
                "restore: daemon looks alive (heartbeat {}s ago); stop it or pass --force",
                age
            )
        }
        Alive::Bind(addr) => {
            let port = parse_host_port(addr)
                .map(|(_, p)| p.to_string())
                .unwrap_or_else(|| addr.clone());
            format!("restore: daemon looks alive (port {} accepts connections); stop it or pass --force", port)
        }
    }
}

/// 恢复备份：逐文件校验通过前不写任何字节；daemon 存活且无 --force 时 exit 3 语义。
pub fn restore(data: &Path, from: &Path, force: bool) -> Result<RestoreResult, BackupError> {
    let manifest_path = from.join("manifest.json");
    let raw = fs::read(&manifest_path).map_err(|e| {
        err(
            1,
            format!(
                "restore: missing manifest.json in {}: {}",
                from.display(),
                e
            ),
        )
    })?;
    let manifest: Manifest = serde_json::from_slice(&raw)
        .map_err(|e| err(1, format!("restore: bad manifest: {}", e)))?;
    if manifest.format != BACKUP_FORMAT {
        return Err(err(
            1,
            format!("restore: unsupported manifest format '{}'", manifest.format),
        ));
    }

    // 逐文件校验：存在、bytes、sha256 三项必须全部匹配，任何一项不符都不写。
    let mut validated: Vec<(String, Vec<u8>)> = Vec::new();
    for f in &manifest.files {
        let p = from.join(&f.name);
        let bytes = fs::read(&p).map_err(|e| {
            err(
                1,
                format!(
                    "restore: {}: missing or unreadable ({}): {}",
                    f.name,
                    p.display(),
                    e
                ),
            )
        })?;
        if bytes.len() as u64 != f.bytes {
            return Err(err(
                1,
                format!(
                    "restore: {}: bytes mismatch (manifest {} vs actual {})",
                    f.name,
                    f.bytes,
                    bytes.len()
                ),
            ));
        }
        let hex = sha256_hex(&bytes);
        if hex != f.sha256 {
            return Err(err(1, format!("restore: {}: sha256 mismatch", f.name)));
        }
        validated.push((f.name.clone(), bytes));
    }

    if !force {
        if let Some(alive) = daemon_probe(data) {
            return Err(err(3, alive_error_message(&alive)));
        }
    }

    let ts = crate::types::now();
    let mut restored = Vec::new();
    for name in RESTORE_ORDER {
        if let Some((_, bytes)) = validated.iter().find(|(n, _)| n.as_str() == name) {
            write_restore_file(data, name, bytes, ts)?;
            restored.push(name.to_string());
        }
    }
    Ok(RestoreResult {
        files: restored,
        from: from.display().to_string(),
    })
}

fn unique_pre_restore_path(dir: &Path, name: &str, ts: i64) -> PathBuf {
    let stamp = backup_stamp(ts, 0);
    let first = dir.join(format!("{}.pre-restore-{}", name, stamp));
    if !first.exists() {
        return first;
    }
    let mut n = 2u64;
    loop {
        let alt = dir.join(format!("{}.pre-restore-{}-{}", name, stamp, n));
        if !alt.exists() {
            return alt;
        }
        n += 1;
    }
}

fn write_restore_file(dir: &Path, name: &str, bytes: &[u8], ts: i64) -> Result<(), BackupError> {
    let p = dir.join(name);
    let tmp = dir.join(format!("{}.tmp", name));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| {
            err(
                1,
                format!("restore: cannot create {}: {}", tmp.display(), e),
            )
        })?;
        f.write_all(bytes)
            .map_err(|e| err(1, format!("restore: cannot write {}: {}", tmp.display(), e)))?;
        f.sync_all()
            .map_err(|e| err(1, format!("restore: cannot sync {}: {}", tmp.display(), e)))?;
    }
    if p.exists() {
        let rot = unique_pre_restore_path(dir, name, ts);
        fs::rename(&p, &rot)
            .map_err(|e| err(1, format!("restore: cannot rotate {}: {}", p.display(), e)))?;
    }
    fs::rename(&tmp, &p)
        .map_err(|e| err(1, format!("restore: cannot install {}: {}", p.display(), e)))?;
    let _ = sync_parent_dir(&p);
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("roamway-{}-{}", name, crate::types::now()))
    }

    fn dead_pid_file(data: &Path) {
        fs::write(data.join("roamway.pid"), b"4294967295\n").unwrap();
    }

    fn heartbeat_with_age(data: &Path, age_secs: u64) {
        let p = data.join("heartbeat");
        fs::write(&p, b"2000-01-01T00:00:00Z\n").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
        f.set_modified(t).unwrap();
    }

    #[test]
    fn pid_file_roundtrip_and_owner() {
        let data = tmp("pid-roundtrip");
        fs::create_dir_all(&data).unwrap();
        assert!(pid_file_owner(&data).is_none());
        let pid = write_pid_file(&data).unwrap();
        assert_eq!(pid, std::process::id());
        assert_eq!(pid_file_owner(&data), Some(std::process::id()));
        let raw = fs::read_to_string(data.join("roamway.pid")).unwrap();
        assert_eq!(raw.trim().parse::<u32>().ok(), Some(std::process::id()));
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn clear_pid_file_only_when_owner() {
        let data = tmp("pid-clear");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("roamway.pid"), b"4294967295\n").unwrap();
        clear_pid_file(&data);
        assert!(data.join("roamway.pid").exists(), "别的 pid 写的文件不能删");
        write_pid_file(&data).unwrap();
        clear_pid_file(&data);
        assert!(!data.join("roamway.pid").exists());
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn daemon_probe_prefers_live_pid() {
        let data = tmp("probe-live-pid");
        fs::create_dir_all(&data).unwrap();
        write_pid_file(&data).unwrap();
        match daemon_probe(&data) {
            Some(Alive::Pid(pid)) => assert_eq!(pid, std::process::id()),
            other => panic!("expected live-pid evidence, got {:?}", other),
        }
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn tasklist_csv_pid_parsing() {
        // 命中行：真实 zh-CN tasklist /FO CSV /NH 输出（内存字段自带逗号）。
        assert!(pid_field_matches(
            "\"pythonw.exe\",\"4242\",\"Console\",\"1\",\"18,068 K\"\r\n",
            4242
        ));
        // 未命中行：zh-CN INFO 只有一段文本、没有第二字段 —— 旧实现就是被这一行骗成 alive。
        assert!(!pid_field_matches(
            "信息: 没有运行的任务匹配指定标准。\r\n",
            999999
        ));
        // 未命中行：en-US INFO。
        assert!(!pid_field_matches(
            "INFO: No tasks are running which match the specified criteria.\r\n",
            999999
        ));
        // pid 数字出现在别的字段（内存 4242 K）不得误命中。
        assert!(!pid_field_matches(
            "\"a.exe\",\"11\",\"Console\",\"1\",\"4242 K\"\r\n",
            4242
        ));
        // 空输出（tasklist 报错）判死。
        assert!(!pid_field_matches("", 4242));
    }

    #[cfg(windows)]
    #[test]
    fn process_alive_false_for_dead_pid_windows() {
        // 回归：旧实现按 tasklist 输出里是否含 "No tasks" 判定，而 zh-CN Windows 的无匹配 INFO 行是
        // `信息: 没有运行的任务匹配指定标准。` —— 任何合法但已死的 pid 都会被判成 alive。
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit", "0"])
            .spawn()
            .expect("spawn cmd /C exit");
        let pid = child.id();
        let _ = child.wait();
        assert!(!process_alive(pid), "已退出的 pid {} 必须判死", pid);
    }

    #[test]
    fn daemon_probe_falls_back_to_fresh_heartbeat() {
        let data = tmp("probe-heartbeat");
        fs::create_dir_all(&data).unwrap();
        dead_pid_file(&data);
        fs::write(data.join("heartbeat"), format!("{}\n", crate::types::now())).unwrap();
        match daemon_probe(&data) {
            Some(Alive::Heartbeat(age)) => assert!(age <= 60, "heartbeat age {} > 60", age),
            other => panic!("expected heartbeat evidence, got {:?}", other),
        }
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn daemon_probe_falls_back_to_open_bind_port() {
        let data = tmp("probe-bind");
        fs::create_dir_all(&data).unwrap();
        dead_pid_file(&data);
        heartbeat_with_age(&data, 3600);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        fs::write(data.join("bind"), format!("127.0.0.1:{}\n", port)).unwrap();
        match daemon_probe(&data) {
            Some(Alive::Bind(addr)) => assert!(addr.ends_with(&format!(":{}", port)), "{}", addr),
            other => panic!("expected bind evidence, got {:?}", other),
        }
        drop(listener);
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn daemon_probe_ignores_stale_pid_and_old_heartbeat() {
        let data = tmp("probe-none");
        fs::create_dir_all(&data).unwrap();
        dead_pid_file(&data);
        heartbeat_with_age(&data, 120);
        assert!(
            daemon_probe(&data).is_none(),
            "三样都陈旧时必须判定为无证据"
        );
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn restore_refuses_when_only_heartbeat_is_fresh() {
        let data = tmp("restore-heartbeat-data");
        let from = tmp("restore-heartbeat-from");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&from).unwrap();
        fs::write(data.join("state.json"), b"original").unwrap();
        fs::write(from.join("state.json"), b"backup").unwrap();
        let manifest = Manifest {
            format: BACKUP_FORMAT.into(),
            created_at: crate::types::now(),
            version: env!("CARGO_PKG_VERSION").into(),
            files: vec![ManifestFile {
                name: "state.json".into(),
                bytes: 6,
                sha256: sha256_hex(b"backup"),
            }],
        };
        fs::write(
            from.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        dead_pid_file(&data);
        fs::write(data.join("heartbeat"), format!("{}\n", crate::types::now())).unwrap();

        let before = fs::read(data.join("state.json")).unwrap();
        let e = restore(&data, &from, false).unwrap_err();
        assert_eq!(e.exit_code(), 3, "{}", e.message());
        assert!(e.message().contains("heartbeat"), "{}", e.message());
        let after = fs::read(data.join("state.json")).unwrap();
        assert_eq!(before, after, "活体拒绝时不得改写 state.json");
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&from);
    }

    #[test]
    fn sha256_matches_rfc6234_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn backup_writes_manifest_with_matching_hashes() {
        let data = tmp("backup-data");
        let out = tmp("backup-out");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("state.json"), b"state-body").unwrap();
        fs::write(data.join("config.json"), b"config-body").unwrap();
        fs::write(data.join("schedules.json"), b"schedules-body").unwrap();

        let res = backup(&data, &out, 0, 8).unwrap();
        assert_eq!(res.files, 3);
        // "state-body"(10) + "config-body"(11) + "schedules-body"(14)
        assert_eq!(res.bytes, 10 + 11 + 14);
        assert_eq!(res.pruned, 0);

        let manifest: Manifest =
            serde_json::from_slice(&fs::read(res.dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.format, "roamway-backup/1");
        assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
        assert!(manifest.created_at > 0);
        assert_eq!(manifest.files.len(), 3);
        for f in &manifest.files {
            let bytes = fs::read(res.dir.join(&f.name)).unwrap();
            assert_eq!(bytes.len() as u64, f.bytes);
            assert_eq!(sha256_hex(&bytes), f.sha256);
        }
        for name in ["state.json", "config.json", "schedules.json"] {
            assert!(manifest.files.iter().any(|f| f.name == name));
        }
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn backup_keep_prunes_old_dirs() {
        let data = tmp("backup-keep-data");
        let out = tmp("backup-keep-out");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("state.json"), b"x").unwrap();
        for name in [
            "roamway-backup-20200101-000000",
            "roamway-backup-20200102-000000",
            "roamway-backup-20200103-000000",
        ] {
            let d = out.join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("manifest.json"), b"{}").unwrap();
        }
        let res = backup(&data, &out, 1, 8).unwrap();
        assert_eq!(res.pruned, 3);
        let names: Vec<String> = fs::read_dir(&out)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("roamway-backup-"))
            .collect();
        assert_eq!(names.len(), 1);
        assert_eq!(
            names[0],
            res.dir.file_name().unwrap().to_string_lossy().to_string()
        );
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn restore_rejects_tampered_hash_and_writes_nothing() {
        let data = tmp("restore-tamper-data");
        let from = tmp("restore-tamper-from");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("config.json"), b"config").unwrap();
        fs::write(from.join("state.json"), b"state").unwrap();
        let files = vec![
            ManifestFile {
                name: "config.json".into(),
                bytes: 6,
                sha256: sha256_hex(b"config"),
            },
            ManifestFile {
                name: "state.json".into(),
                bytes: 5,
                sha256: sha256_hex(b"tampered"),
            },
        ];
        let manifest = Manifest {
            format: BACKUP_FORMAT.into(),
            created_at: crate::types::now(),
            version: env!("CARGO_PKG_VERSION").into(),
            files,
        };
        fs::write(
            from.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let e = restore(&data, &from, false).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.message().contains("state.json"), "{}", e.message());
        assert!(
            !data.join("config.json").exists(),
            "校验失败前不得写任何字节"
        );
        assert!(!data.join("state.json").exists());
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&from);
    }

    #[test]
    fn restore_refuses_alive_daemon_without_force() {
        let data = tmp("restore-alive-data");
        let from = tmp("restore-alive-from");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("state.json"), b"state").unwrap();
        let manifest = Manifest {
            format: BACKUP_FORMAT.into(),
            created_at: crate::types::now(),
            version: env!("CARGO_PKG_VERSION").into(),
            files: vec![ManifestFile {
                name: "state.json".into(),
                bytes: 5,
                sha256: sha256_hex(b"state"),
            }],
        };
        fs::write(
            from.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(data.join("roamway.pid"), format!("{}", std::process::id())).unwrap();

        let e = restore(&data, &from, false).unwrap_err();
        assert_eq!(e.exit_code(), 3, "{}", e.message());
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&from);
    }

    #[test]
    fn acquire_writer_creates_lock_with_pid_and_bind() {
        let data = tmp("writer-create");
        fs::create_dir_all(&data).unwrap();
        let owner_pid = 4242;
        let bind = "127.0.0.1:8796";
        let got = try_acquire_writer_with(&data, bind, owner_pid, &|_| false).unwrap();
        assert_eq!(got, owner_pid);
        let raw = fs::read_to_string(data.join("roamway.lock")).unwrap();
        let fields: Vec<&str> = raw.split_whitespace().collect();
        assert_eq!(fields[0], "4242");
        assert_eq!(fields[1], bind);
        assert!(fields.len() >= 3, "lock must include an iso timestamp");
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn second_writer_refused_when_owner_alive() {
        let data = tmp("writer-busy");
        fs::create_dir_all(&data).unwrap();
        let lock = data.join("roamway.lock");
        fs::write(&lock, "111 10.0.0.1:9000 2020-01-01T00:00:00Z\n").unwrap();
        let before = fs::read(&lock).unwrap();
        let err = try_acquire_writer_with(&data, "127.0.0.1:9001", 4242, &|_| true).unwrap_err();
        assert_eq!(err, (111, "10.0.0.1:9000".to_string()));
        assert_eq!(
            fs::read(&lock).unwrap(),
            before,
            "lock file must be left untouched"
        );
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn stale_lock_taken_over_when_owner_dead() {
        let data = tmp("writer-stale");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("roamway.lock"),
            "111 10.0.0.1:9000 2020-01-01T00:00:00Z\n",
        )
        .unwrap();
        let got = try_acquire_writer_with(&data, "127.0.0.1:8796", 4242, &|_| false).unwrap();
        assert_eq!(got, 4242);
        let raw = fs::read_to_string(data.join("roamway.lock")).unwrap();
        let fields: Vec<&str> = raw.split_whitespace().collect();
        assert_eq!(fields[0], "4242");
        assert_eq!(fields[1], "127.0.0.1:8796");
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn stale_lock_taken_over_when_owner_is_self() {
        let data = tmp("writer-self");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("roamway.lock"),
            "4242 bind-a 2020-01-01T00:00:00Z\n",
        )
        .unwrap();
        let got = try_acquire_writer_with(&data, "bind-b", 4242, &|_| true).unwrap();
        assert_eq!(got, 4242);
        let raw = fs::read_to_string(data.join("roamway.lock")).unwrap();
        let fields: Vec<&str> = raw.split_whitespace().collect();
        assert_eq!(fields[0], "4242");
        assert_eq!(fields[1], "bind-b");
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn garbage_lock_file_treated_as_stale() {
        let data = tmp("writer-garbage");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("roamway.lock"), b"not-a-lock").unwrap();
        let got = try_acquire_writer_with(&data, "127.0.0.1:8796", 4242, &|_| true).unwrap();
        assert_eq!(got, 4242);
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn release_writer_only_when_owner() {
        let data = tmp("writer-release");
        fs::create_dir_all(&data).unwrap();
        let lock = data.join("roamway.lock");
        fs::write(&lock, "4294967295 some-bind 2020-01-01T00:00:00Z\n").unwrap();
        release_writer(&data);
        assert!(lock.exists(), "another writer's lock must not be removed");
        fs::write(
            &lock,
            format!("{} bind 2020-01-01T00:00:00Z\n", std::process::id()),
        )
        .unwrap();
        release_writer(&data);
        assert!(!lock.exists());
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn force_takeover_overwrites_live_lock() {
        let data = tmp("writer-force");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("roamway.lock"),
            "111 10.0.0.1:9000 2020-01-01T00:00:00Z\n",
        )
        .unwrap();
        let got = force_acquire_writer(&data, "127.0.0.1:8796").unwrap();
        assert_eq!(got, std::process::id());
        let raw = fs::read_to_string(data.join("roamway.lock")).unwrap();
        let fields: Vec<&str> = raw.split_whitespace().collect();
        assert_eq!(fields[0], std::process::id().to_string());
        assert_eq!(fields[1], "127.0.0.1:8796");
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn busy_message_mentions_pid_and_bind() {
        let message = writer_busy_message(Path::new("E:/data"), 4242, "[::1]:8796");
        assert!(message.contains("4242"));
        assert!(message.contains("[::1]:8796"));
    }
}
