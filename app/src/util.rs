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

// ---------------- Dateien absturzsicher schreiben ----------------
// Erst in eine temporäre Datei, auf die Platte zwingen, dann umbenennen: nach einem Stromausfall liegt
// entweder die alte oder die neue Datei da – nie eine halbe.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() { std::fs::create_dir_all(d)?; }
    let tmp = path.with_extension(format!("{}.tmp", random_hex(3)));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    // Virenscanner/Indexer halten Dateien manchmal kurz fest
    for i in 0..10 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) if i < 9 => { let _ = e; std::thread::sleep(std::time::Duration::from_millis(40 * (i + 1))); }
            Err(e) => { let _ = std::fs::remove_file(&tmp); return Err(e); }
        }
    }
    Ok(())
}

pub fn write_json(path: &Path, v: &Value) {
    if let Err(e) = write_atomic(path, serde_json::to_string_pretty(v).unwrap_or_default().as_bytes()) {
        log(format!("{} konnte nicht gespeichert werden: {e}", path.display()));
    }
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

// Zeile anhängen und sofort auf die Platte schreiben (Journal)
pub fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() { std::fs::create_dir_all(d)?; }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_data()
}

// ---------------- Einladungscodes ----------------
// lesbar und ohne verwechselbare Zeichen (kein 0/O, 1/I/L), angezeigt als K7M-2QX-9PA
pub const CODE_LEN: usize = 9;
const CODE_ABC: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

pub fn new_code() -> String {
    let mut b = [0u8; CODE_LEN];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| CODE_ABC[*x as usize % CODE_ABC.len()] as char).collect()
}

pub fn norm_code(s: &str) -> String {
    s.to_uppercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

pub fn valid_code(s: &str) -> bool {
    let c = norm_code(s);
    c.len() == CODE_LEN && c.bytes().all(|b| CODE_ABC.contains(&b))
}

pub fn fmt_code(s: &str) -> String {
    let c: Vec<char> = norm_code(s).chars().collect();
    c.chunks(3).map(|p| p.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

// ---------------- Geräte-Kennung ----------------
// Früher stand die Kennung in einstellungen.txt: wer den Ordner kopiert hat, hatte dieselbe Kennung wie der
// Absender, und die Programme hielten die Änderungen des anderen für ihre eigenen. Jetzt liegt sie im
// Benutzerprofil (nicht im Programmordner) und ist an diesen PC gebunden – kopierte Ordner bekommen eine eigene.
pub fn local_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    base.join("ForzaDashboard")
}

fn machine_fingerprint() -> String {
    let mut m = String::new();
    if let Ok(out) = no_window(std::process::Command::new("reg").args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])).output() {
        let t = String::from_utf8_lossy(&out.stdout);
        if let Some(l) = t.lines().find(|l| l.contains("MachineGuid")) { m = l.split_whitespace().last().unwrap_or("").to_string(); }
    }
    if m.is_empty() { m = format!("{}|{}", std::env::var("COMPUTERNAME").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default()); }
    sha_hex(format!("fd-geraet:{m}").as_bytes())[..16].to_string()
}

pub fn device_id() -> String {
    if let Ok(id) = std::env::var("FD_DEVICE_ID") { return id; }
    let file = local_dir().join("geraet.json");
    let fp = machine_fingerprint();
    if let Some(o) = read_json(&file) {
        let id = o["id"].as_str().unwrap_or("");
        if id.len() == 10 && id.bytes().all(|b| b.is_ascii_hexdigit()) && o["machine"].as_str() == Some(&fp) { return id.to_string(); }
    }
    let id = random_hex(5);
    write_json(&file, &serde_json::json!({"id": id, "machine": fp, "created": now_ms()}));
    log(format!("Neue Geräte-Kennung für diesen PC: {id}"));
    id
}

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

pub fn log_init(root: &Path) {
    let p = root.join("ForzaDashboard-log.txt");
    // das Protokoll des letzten Laufs bleibt als ForzaDashboard-log-vorher.txt (z. B. nach einem Absturz)
    let _ = std::fs::rename(&p, root.join("ForzaDashboard-log-vorher.txt"));
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

pub fn gzip(b: &[u8]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    let mut e = GzEncoder::new(Vec::new(), Compression::new(9));
    let _ = e.write_all(b);
    e.finish().unwrap_or_default()
}
