// Kleine Helfer: Zeit, Hashes, Versionen, Ordner, Protokoll
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn sha_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    let _ = getrandom::fill(&mut b);
    hex::encode(b)
}

// JavaScript-Wahrheitswert (für "x || null" / "x || 0" aus der Node-Version)
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn ver_parts(v: &str) -> Option<[u64; 4]> {
    // Format: JJJJ-MM-TT.N
    let (date, n) = v.split_once('.')?;
    let mut it = date.split('-');
    let p = [it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?, n.parse().ok()?];
    if it.next().is_some() { return None; }
    Some(p)
}

pub fn ver_newer(a: &str, b: &str) -> bool {
    let Some(a) = ver_parts(a) else { return false };
    let Some(b) = ver_parts(b) else { return true };
    a > b
}

pub fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}

pub fn lan_ips() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(list) = if_addrs::get_if_addrs() {
        for a in list {
            if let std::net::IpAddr::V4(ip) = a.ip() {
                if !ip.is_loopback() && !ip.is_link_local() { out.push(ip.to_string()); }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

// Datenordner: neben der exe, wenn dort schon ForzaDashboard-Daten liegen (portabel),
// sonst %APPDATA%\ForzaDashboard. FD_ROOT überschreibt beides.
pub fn find_root() -> PathBuf {
    if let Ok(r) = std::env::var("FD_ROOT") { return PathBuf::from(r); }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let portable = ["einstellungen.txt", "gelernte-strassen.json"].iter().any(|f| dir.join(f).exists());
        if portable && writable(&dir) { return dir; }
    }
    let base = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join("ForzaDashboard");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn writable(dir: &Path) -> bool {
    let p = dir.join(".fd-schreibtest");
    let ok = std::fs::write(&p, b"").is_ok();
    let _ = std::fs::remove_file(&p);
    ok
}

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

pub fn log_init(root: &Path) {
    let p = root.join("ForzaDashboard-log.txt");
    let f = std::fs::File::create(p).ok();
    let _ = LOG.set(Mutex::new(f));
}

pub fn log(msg: impl AsRef<str>) {
    let line = format!("[{}] {}\r\n", now_ms() / 1000, msg.as_ref());
    #[cfg(debug_assertions)]
    eprint!("{line}");
    if let Some(m) = LOG.get() {
        if let Some(f) = m.lock().unwrap().as_mut() { let _ = f.write_all(line.as_bytes()); }
    }
}

pub fn msgbox(text: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("ForzaDashboard"), MB_OK | MB_ICONWARNING); }
}

// Nach einem Update: warten, bis die alte Programmversion beendet ist
pub fn wait_pid(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    if pid == 0 { return; }
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, 15000);
            let _ = CloseHandle(h);
        }
    }
}
