// Netz: Freunde, gemeinsame Straßen und Updates über einen öffentlichen MQTT-Vermittler.
// - Freunde: jeder sendet 2× pro Sekunde Name + Position (nur Spielkoordinaten) in seine Gruppe.
// - Gemeinsam lernen: die gelernten Straßen/Vormerkungen werden in der Gruppe geteilt.
// - Updates: neue ForzaDashboard.exe kommt signiert vom Herausgeber-PC und wird automatisch installiert.
// Gruppen-Themen und Datenformate sind dieselben wie in der Node-Version (beide sehen sich gegenseitig).
use crate::state::{Friend, Shared};
use crate::util::{log, now_ms, sha_hex, take_chars, truthy, ver_newer};
use crate::APP_VER;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signer, Verifier};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, timeout};

// falls Port 1883 im Netzwerk gesperrt ist, wird es verschlüsselt über 8883 versucht
const BROKERS: &[(&str, u16, bool)] = &[
    ("broker.hivemq.com", 1883, false),
    ("broker.emqx.io", 1883, false),
    ("broker.hivemq.com", 8883, true),
    ("broker.emqx.io", 8883, true),
    ("test.mosquitto.org", 1883, false),
];

// ---------------- MQTT-Grundlagen ----------------
fn mq_str(s: &[u8]) -> Vec<u8> {
    let mut v = (s.len() as u16).to_be_bytes().to_vec();
    v.extend_from_slice(s);
    v
}

fn mq_pkt(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    let mut n = body.len();
    loop {
        let mut d = (n % 128) as u8;
        n /= 128;
        if n > 0 { d |= 128; }
        out.push(d);
        if n == 0 { break; }
    }
    out.extend_from_slice(body);
    out
}

fn send(s: &Shared, pkt: Vec<u8>) {
    if !s.mq_ok() { return; }
    if let Some(tx) = s.mq.lock().unwrap().as_ref() { let _ = tx.send(pkt); }
}

pub fn publish(s: &Shared, topic: &str, payload: &[u8], retain: bool) {
    let mut body = mq_str(topic.as_bytes());
    body.extend_from_slice(payload);
    send(s, mq_pkt(if retain { 0x31 } else { 0x30 }, &body));
}

fn subscribe(s: &Shared, filters: &[String]) {
    let pid = s.mq_pid.fetch_add(1, Ordering::Relaxed) % 60000 + 1;
    let mut body = pid.to_be_bytes().to_vec();
    for f in filters {
        body.extend(mq_str(f.as_bytes()));
        body.push(0);
    }
    send(s, mq_pkt(0x82, &body));
}

pub fn grp_topic(group: &str) -> String {
    format!("fhdash/v1/{}", &sha_hex(format!("fhdash:{}", group.trim().to_lowercase()).as_bytes())[..24])
}

fn topic(s: &Shared) -> String {
    s.mq_topic.lock().unwrap().clone()
}

// "Ich bin weg" an die Gruppe
pub fn bye(s: &Shared) {
    if s.mq_ok() { publish(s, &format!("{}/{}", topic(s), s.sid), br#"{"o":0}"#, false); }
}

// nach Gruppenwechsel neu verbinden
pub fn restart(s: &Arc<Shared>) {
    bye(s);
    s.friends.lock().unwrap().clear();
    let s = s.clone();
    tauri::async_runtime::spawn(async move {
        sleep(Duration::from_millis(150)).await;
        s.mq_restart.notify_one();
    });
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

enum Outcome {
    Ended(bool),
    Restart,
}

pub async fn run(s: Arc<Shared>) {
    let (mut bi, mut wait) = (0usize, 2000u64);
    loop {
        let (host, port, secure) = BROKERS[bi % BROKERS.len()];
        match session(&s, host, port, secure).await {
            Outcome::Restart => continue,
            Outcome::Ended(connected) => {
                if connected { wait = 2000; }
                bi += 1;
                tokio::select! {
                    _ = sleep(Duration::from_millis(wait)) => {}
                    _ = s.mq_restart.notified() => {}
                }
                wait = (wait as f64 * 1.6).min(30000.0) as u64;
            }
        }
    }
}

async fn session(s: &Arc<Shared>, host: &str, port: u16, secure: bool) -> Outcome {
    let group_topic = grp_topic(&s.cfg.lock().unwrap().fr_group);
    *s.mq_topic.lock().unwrap() = group_topic;
    let Ok(Ok(tcp)) = timeout(Duration::from_secs(10), TcpStream::connect((host, port))).await else { return Outcome::Ended(false) };
    let _ = tcp.set_nodelay(true);
    let io: Box<dyn Io> = if secure {
        let Ok(cx) = native_tls::TlsConnector::new() else { return Outcome::Ended(false) };
        match timeout(Duration::from_secs(10), tokio_native_tls::TlsConnector::from(cx).connect(host, tcp)).await {
            Ok(Ok(t)) => Box::new(t),
            _ => return Outcome::Ended(false),
        }
    } else {
        Box::new(tcp)
    };
    let (mut rd, mut wr) = tokio::io::split(io);
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut connect = mq_str(b"MQTT");
    connect.extend_from_slice(&[4, 0x02, 0, 60]);
    connect.extend(mq_str(format!("fhdash-{}-{}", s.sid, crate::util::random_hex(2)).as_bytes()));
    let _ = tx.send(mq_pkt(0x10, &connect));
    let writer = tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            if wr.write_all(&p).await.is_err() { break; }
        }
    });

    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = vec![0u8; 65536];
    let mut connected = false;
    let mut ping = interval(Duration::from_secs(15));
    ping.tick().await;
    let out = loop {
        tokio::select! {
            r = timeout(Duration::from_secs(45), rd.read(&mut tmp)) => match r {
                Ok(Ok(n)) if n > 0 => {
                    buf.extend_from_slice(&tmp[..n]);
                    if !handle_packets(s, &mut buf, &tx, &mut connected, host) { break Outcome::Ended(connected); }
                }
                _ => break Outcome::Ended(connected),
            },
            _ = ping.tick() => { if connected { let _ = tx.send(vec![0xC0, 0]); } }
            _ = s.mq_restart.notified() => break Outcome::Restart,
        }
    };
    s.mq_ok.store(false, Ordering::Relaxed);
    *s.mq.lock().unwrap() = None;
    drop(tx);
    sleep(Duration::from_millis(50)).await;
    writer.abort();
    out
}

// false = Verbindung beenden
fn handle_packets(s: &Arc<Shared>, buf: &mut Vec<u8>, tx: &mpsc::UnboundedSender<Vec<u8>>, connected: &mut bool, host: &str) -> bool {
    loop {
        if buf.len() < 2 { return true; }
        let (mut mul, mut len, mut i) = (1usize, 0usize, 1usize);
        loop {
            if i >= buf.len() { return true; }
            let b = buf[i];
            i += 1;
            len += (b & 127) as usize * mul;
            mul *= 128;
            if b & 128 == 0 || i >= 5 { break; }
        }
        if buf.len() < i + len { return true; }
        let kind = buf[0] >> 4;
        let qos = (buf[0] >> 1) & 3;
        let body: Vec<u8> = buf[i..i + len].to_vec();
        buf.drain(..i + len);
        match kind {
            2 => {
                // CONNACK
                if body.get(1) != Some(&0) { return false; }
                *connected = true;
                *s.mq.lock().unwrap() = Some(tx.clone());
                s.mq_ok.store(true, Ordering::Relaxed);
                log(format!("Verbunden mit {host}"));
                subscribe(s, &[format!("{}/#", topic(s)), format!("{}/#", updx_topic())]);
                crate::shared::on_connect(s);
                let s2 = s.clone();
                tauri::async_runtime::spawn(async move {
                    sleep(Duration::from_millis(1500)).await;
                    let s3 = s2.clone();
                    let _ = tokio::task::spawn_blocking(move || { lr_publish(&s3, true); upd_publish(&s3, true); }).await;
                });
            }
            3 => {
                // PUBLISH
                if body.len() < 2 { continue; }
                let tl = u16::from_be_bytes([body[0], body[1]]) as usize;
                let start = 2 + tl + if qos > 0 { 2 } else { 0 };
                if body.len() < start { continue; }
                let t = String::from_utf8_lossy(&body[2..2 + tl]).to_string();
                on_msg(s, &t, &body[start..]);
            }
            _ => {}
        }
    }
}

fn on_msg(s: &Arc<Shared>, t: &str, payload: &[u8]) {
    if let Some(sub) = t.strip_prefix(&format!("{}/", updx_topic())) { return upd_on_msg(s, sub, payload); }
    let gt = topic(s);
    let Some(rest) = t.strip_prefix(&format!("{gt}/")) else { return };
    if let Some(sub) = rest.strip_prefix("map/") { return crate::shared::on_mqtt(s, sub, payload); }
    if let Some(id) = rest.strip_prefix("lr/") {
        if id != s.sid && !payload.is_empty() { flr_on_msg(s, id, payload); }
        return;
    }
    if rest.contains('/') || rest == s.sid || !s.cfg.lock().unwrap().fr_on { return; }
    let Ok(m) = serde_json::from_slice::<Value>(payload) else { return };
    if m.get("o").and_then(Value::as_f64) == Some(0.0) {
        s.friends.lock().unwrap().remove(rest);
        return;
    }
    let (Some(x), Some(z)) = (m.get("x").and_then(Value::as_f64), m.get("z").and_then(Value::as_f64)) else { return };
    let num = |k: &str| m.get(k).and_then(Value::as_f64).filter(|f| f.is_finite()).unwrap_or(0.0);
    let n = match m.get("n") { Some(Value::String(n)) if !n.is_empty() => n.clone(), Some(v) if truthy(v) => v.to_string(), _ => "Freund".into() };
    let f = Friend {
        n: take_chars(&n, 24), x, z, h: num("h"), v: num("v"), c: num("c") as i64,
        g: if m.get("g").is_some_and(truthy) { 1 } else { 0 }, seen: now_ms(),
    };
    s.friends.lock().unwrap().insert(rest.to_string(), f);
}

// ---------------- Freunde: eigene Position senden, Liste an die Dashboards ----------------
pub fn spawn_timers(s: Arc<Shared>) {
    let s1 = s.clone();
    tauri::async_runtime::spawn(async move {
        let mut t = interval(Duration::from_millis(500));
        loop {
            t.tick().await;
            pos_tick(&s1);
            friends_tick(&s1);
        }
    });
    let s2 = s.clone();
    tauri::async_runtime::spawn(async move {
        let mut t = interval(Duration::from_secs(30));
        loop {
            t.tick().await;
            let s3 = s2.clone();
            let _ = tokio::task::spawn_blocking(move || lr_publish(&s3, false)).await;
        }
    });
    let s4 = s.clone();
    tauri::async_runtime::spawn(async move {
        let mut t = interval(Duration::from_secs(5));
        let mut n = 0u64;
        loop {
            t.tick().await;
            n += 1;
            upd_request_missing(&s4);
            if n % 360 == 0 {
                let s5 = s4.clone();
                let _ = tokio::task::spawn_blocking(move || upd_publish(&s5, false)).await;
            }
        }
    });
}

fn pos_tick(s: &Shared) {
    let (fr_on, name) = { let c = s.cfg.lock().unwrap(); (c.fr_on, c.fr_name.clone()) };
    if !s.mq_ok() || !fr_on { return; }
    let Some(p) = *s.my_pos.lock().unwrap() else { return };
    let now = now_ms();
    let fresh = now.saturating_sub(p.t) < 3000;
    if !fresh && now.saturating_sub(s.last_pub.load(Ordering::Relaxed)) < 5000 { return; }
    s.last_pub.store(now, Ordering::Relaxed);
    let r = |v: f64, k: f64| (v * k).round() / k;
    let msg = json!({"n": name, "x": r(p.x, 10.0), "z": r(p.z, 10.0), "h": r(p.h, 1000.0), "v": r(p.v, 10.0), "c": p.c, "g": if fresh { 1 } else { 0 }});
    publish(s, &format!("{}/{}", topic(s), s.sid), msg.to_string().as_bytes(), false);
}

fn friends_tick(s: &Shared) {
    let now = now_ms();
    let mut list = Vec::new();
    {
        let mut fr = s.friends.lock().unwrap();
        fr.retain(|_, f| now.saturating_sub(f.seen) <= 12000);
        for (id, f) in fr.iter() {
            list.push(json!({"id": id, "n": f.n, "x": f.x, "z": f.z, "h": f.h, "v": f.v, "c": f.c, "g": f.g, "age": now.saturating_sub(f.seen)}));
        }
    }
    let (on, name, group) = { let c = s.cfg.lock().unwrap(); (c.fr_on, c.fr_name.clone(), c.fr_group.clone()) };
    let lr = s.flr.lock().unwrap().list.len();
    let upd = s.upd.lock().unwrap().state.clone();
    let map = crate::shared::ok_count(s);
    s.emit(Some("fr"), json!({"on": on, "ok": s.mq_ok(), "name": name, "group": group, "list": list, "lr": lr, "upd": upd, "map": map}).to_string());
}

// ---------------- Gemeinsam lernen ----------------
fn flr_save(s: &Shared) {
    let text = Value::Object(s.flr.lock().unwrap().list.clone()).to_string();
    let f = s.file("freunde-strassen.json");
    let tmp = s.file("freunde-strassen.json.tmp");
    if std::fs::write(&tmp, text).is_ok() { let _ = std::fs::rename(&tmp, &f); }
}

fn flr_on_msg(s: &Shared, id: &str, payload: &[u8]) {
    let Some(text) = gunzip(payload) else { return };
    let Ok(o) = serde_json::from_str::<Value>(&text) else { return };
    let Some(roads) = o.get("roads").and_then(Value::as_array) else { return };
    let ts = o.get("ts").and_then(Value::as_f64);
    {
        let mut flr = s.flr.lock().unwrap();
        if let (Some(prev), Some(ts)) = (flr.list.get(id).and_then(|p| p.get("ts")).and_then(Value::as_f64), ts) {
            if prev >= ts { return; }
        }
        let n = match o.get("n") { Some(Value::String(n)) if !n.is_empty() => n.clone(), _ => "Freund".into() };
        let cands: Vec<Value> = o.get("cands").and_then(Value::as_array).map(|c| c.iter().take(8000).cloned().collect()).unwrap_or_default();
        let entry = json!({"n": take_chars(&n, 24), "ts": ts.map(Value::from).unwrap_or_else(|| now_ms().into()), "roads": roads.iter().take(5000).cloned().collect::<Vec<_>>(), "cands": cands});
        flr.list.insert(id.to_string(), entry);
        flr.v += 1;
    }
    flr_save(s);
}

// gelernte Straßen aus der Karten-Datei eines Freundes: wie empfangene Freundes-Straßen ablegen (bleibt auf diesem PC)
pub fn flr_import(s: &Shared, name: &str, roads: Vec<Value>, cands: Vec<Value>) {
    let n = take_chars(name, 24);
    let id = format!("import-{}", &sha_hex(n.to_lowercase().as_bytes())[..8]);
    {
        let mut flr = s.flr.lock().unwrap();
        flr.list.insert(id, json!({"n": n, "ts": now_ms(), "roads": roads.into_iter().take(5000).collect::<Vec<_>>(), "cands": cands.into_iter().take(8000).collect::<Vec<_>>()}));
        flr.v += 1;
    }
    flr_save(s);
    log(format!("Gelernte Straßen von {n} importiert"));
}

pub fn lr_publish(s: &Shared, force: bool) {
    let (fr_on, name) = { let c = s.cfg.lock().unwrap(); (c.fr_on, c.fr_name.clone()) };
    if !s.mq_ok() || !fr_on { return; }
    let now = now_ms();
    let lver = s.lver.load(Ordering::Relaxed) as i64;
    {
        let f = s.flr.lock().unwrap();
        let since = now.saturating_sub(f.pub_t);
        if !force && f.pub_v == lver && since < 15 * 60000 { return; }
        if !force && since < 60000 { return; }
    }
    let Ok(text) = std::fs::read_to_string(s.file("gelernte-strassen.json")) else { return };
    let Ok(d) = serde_json::from_str::<Value>(&text) else { return };
    let Some(roads) = d.get("roads").and_then(Value::as_array) else { return };
    let long = |v: &Value| v.get("p").and_then(Value::as_array).is_some_and(|p| p.len() >= 4);
    let roads: Vec<Value> = roads.iter().filter(|r| long(r)).map(|r| {
        let mut o = serde_json::Map::new();
        // feste Kennung, damit ausgeblendete Straßen auch nach Änderungen ausgeblendet bleiben
        if let Some(i) = r.get("i") { o.insert("i".into(), i.clone()); }
        o.insert("t".into(), r.get("t").filter(|v| truthy(v)).cloned().unwrap_or(Value::Null));
        o.insert("lv".into(), r.get("lv").filter(|v| truthy(v)).cloned().unwrap_or(json!(0)));
        if let Some(c) = r.get("c") { o.insert("c".into(), c.clone()); }
        o.insert("p".into(), r["p"].clone());
        Value::Object(o)
    }).collect();
    let mut cands: Vec<&Value> = d.get("cands").and_then(Value::as_array).map(|c| c.iter().filter(|c| long(c)).collect()).unwrap_or_default();
    let ts = |v: &Value| v.get("ts").and_then(Value::as_f64).unwrap_or(0.0);
    cands.sort_by(|a, b| ts(b).partial_cmp(&ts(a)).unwrap_or(std::cmp::Ordering::Equal));
    let mut cands: Vec<Value> = cands.into_iter().map(|c| {
        let mut o = serde_json::Map::new();
        o.insert("p".into(), c["p"].clone());
        if let Some(k) = c.get("k") { o.insert("k".into(), k.clone()); }
        Value::Object(o)
    }).collect();
    let gz = loop {
        let body = json!({"n": name, "ts": now, "roads": roads, "cands": cands}).to_string();
        let gz = gzip(body.as_bytes());
        if gz.len() <= 180_000 || cands.is_empty() { break gz; }
        cands.truncate(cands.len() * 3 / 4);
    };
    publish(s, &format!("{}/lr/{}", topic(s), s.sid), &gz, true);
    let mut f = s.flr.lock().unwrap();
    f.pub_v = lver;
    f.pub_t = now;
}

pub fn gzip(b: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::new(9));
    let _ = e.write_all(b);
    e.finish().unwrap_or_default()
}

pub fn gunzip(b: &[u8]) -> Option<String> {
    let mut text = String::new();
    GzDecoder::new(b).read_to_string(&mut text).ok()?;
    Some(text)
}

// ---------------- Automatische Updates (ForzaDashboard.exe) ----------------
// Nur Pakete mit gültiger Signatur des Herausgebers werden installiert (Schlüssel: herausgeber.key, nur auf dem Herausgeber-PC).
const UPD_PUB: &str = "MCowBQYDK2VwAyEARFw+U6bmc7q6IDkl02pwPx2CWif6zv+oSYvhpUlze7Y=";
const UPD_CH: usize = 48000;
const EXE_NAME: &str = "ForzaDashboard.exe";

// eigenes Thema für die exe, damit die Node-Versionen die großen exe-Teile nicht mitladen
fn updx_topic() -> String {
    format!("fhdash/updx/{}", &sha_hex(UPD_PUB.as_bytes())[..20])
}

fn pub_key() -> Option<ed25519_dalek::VerifyingKey> {
    let der = B64.decode(UPD_PUB).ok()?;
    let raw: [u8; 32] = der.get(der.len().checked_sub(32)?..)?.try_into().ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&raw).ok()
}

#[derive(Default)]
pub struct Upd {
    pub state: String,
    man: Option<Manifest>,
    chunks: HashMap<String, Vec<u8>>,
    last_req: u64,
    pub_t: u64,
    req_t: u64,
    installing: bool,
    t0: u64,
}

#[derive(Clone)]
struct Manifest {
    ver: String,
    h: String,
    size: usize,
    c: usize,
    x: String,
}

fn hex64(v: &Value) -> Option<String> {
    v.as_str().filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).map(str::to_string)
}

// gepackte eigene exe (einmal berechnen – sie ändert sich während des Laufs nicht)
fn own_package() -> Option<&'static (Vec<u8>, String)> {
    static PKG: OnceLock<Option<(Vec<u8>, String)>> = OnceLock::new();
    PKG.get_or_init(|| {
        let exe = std::fs::read(std::env::current_exe().ok()?).ok()?;
        let x = sha_hex(&exe);
        Some((gzip(&exe), x))
    })
    .as_ref()
}

pub fn upd_publish(s: &Shared, force: bool) {
    let Some(key) = &s.priv_key else { return };
    if !s.mq_ok() { return; }
    let now = now_ms();
    {
        let mut u = s.upd.lock().unwrap();
        if !force && now.saturating_sub(u.pub_t) < 30000 { return; }
        u.pub_t = now;
    }
    let Some((gz, x)) = own_package() else { return };
    let h = sha_hex(gz);
    let c = gz.len().div_ceil(UPD_CH);
    let b = json!({"ver": APP_VER, "ts": now, "files": [{"n": EXE_NAME, "h": h, "s": gz.len(), "c": c, "x": x}]}).to_string();
    let sig = B64.encode(key.sign(b.as_bytes()).to_bytes());
    let t = updx_topic();
    for (ci, part) in gz.chunks(UPD_CH).enumerate() {
        publish(s, &format!("{t}/d/{}/{ci}", &h[..16]), part, true);
    }
    publish(s, &format!("{t}/m"), json!({"b": b, "s": sig}).to_string().as_bytes(), true);
    // alte Teile beim Vermittler löschen
    let state_file = s.file("herausgeber-stand-exe.json");
    let old: Vec<Value> = std::fs::read_to_string(&state_file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for o in old {
        let (Some(oh), Some(oc)) = (o.get("h").and_then(Value::as_str), o.get("c").and_then(Value::as_u64)) else { continue };
        if oh == &h[..16] { continue; }
        for ci in 0..oc { publish(s, &format!("{t}/d/{oh}/{ci}"), &[], true); }
    }
    let _ = std::fs::write(&state_file, json!([{"h": &h[..16], "c": c}]).to_string());
    s.upd.lock().unwrap().state = format!("Herausgeber · verteilt {APP_VER}");
    log(format!("Update {APP_VER} verteilt ({} Teile)", c));
}

fn upd_on_msg(s: &Arc<Shared>, sub: &str, payload: &[u8]) {
    let publisher = s.priv_key.is_some();
    if sub == "req" {
        if publisher {
            let now = now_ms();
            let mut u = s.upd.lock().unwrap();
            if now.saturating_sub(u.req_t) > 20000 {
                u.req_t = now;
                let s2 = s.clone();
                tauri::async_runtime::spawn_blocking(move || upd_publish(&s2, true));
            }
        }
        return;
    }
    if publisher || payload.is_empty() { return; }
    if sub == "m" {
        if s.upd.lock().unwrap().installing { return; }
        let Ok(m) = serde_json::from_slice::<Value>(payload) else { return };
        let (Some(b), Some(sig)) = (m.get("b").and_then(Value::as_str), m.get("s").and_then(Value::as_str)) else { return };
        let (Some(pk), Ok(sig)) = (pub_key(), B64.decode(sig)) else { return };
        let Ok(sig) = ed25519_dalek::Signature::from_slice(&sig) else { return };
        if pk.verify(b.as_bytes(), &sig).is_err() { return; }
        let Ok(man) = serde_json::from_str::<Value>(b) else { return };
        let ver = man.get("ver").and_then(Value::as_str).unwrap_or("").to_string();
        if !ver_newer(&ver, APP_VER) { return; }
        let Some([f]) = man.get("files").and_then(Value::as_array).map(Vec::as_slice) else { return };
        if f.get("n").and_then(Value::as_str) != Some(EXE_NAME) { return; }
        let (Some(h), Some(x)) = (f.get("h").and_then(hex64), f.get("x").and_then(hex64)) else { return };
        let size = f.get("s").and_then(Value::as_u64).unwrap_or(0) as usize;
        let c = f.get("c").and_then(Value::as_u64).unwrap_or(0) as usize;
        if c == 0 || c >= 400 { return; }
        {
            let mut u = s.upd.lock().unwrap();
            if u.man.as_ref().is_some_and(|m| m.ver == ver) { return; }
            u.state = format!("Update {ver} wird geladen …");
            u.t0 = now_ms();
            u.man = Some(Manifest { ver: ver.clone(), h, size, c, x });
        }
        log(format!("Neue Version {ver} gefunden – wird geladen"));
        upd_check(s);
    } else if let Some(rest) = sub.strip_prefix("d/") {
        {
            let mut u = s.upd.lock().unwrap();
            if u.chunks.len() > 600 { u.chunks.clear(); }
            u.chunks.insert(rest.to_string(), payload.to_vec());
        }
        upd_check(s);
    }
}

fn upd_check(s: &Arc<Shared>) {
    let (man, blob) = {
        let u = s.upd.lock().unwrap();
        let Some(man) = u.man.clone() else { return };
        if u.installing { return; }
        let mut blob = Vec::with_capacity(man.size);
        for ci in 0..man.c {
            let Some(p) = u.chunks.get(&format!("{}/{ci}", &man.h[..16])) else { return };
            blob.extend_from_slice(p);
        }
        (man, blob)
    };
    if blob.len() != man.size || sha_hex(&blob) != man.h { return; }
    s.upd.lock().unwrap().installing = true;
    let s = s.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut exe = Vec::new();
        let res = if GzDecoder::new(&blob[..]).read_to_end(&mut exe).is_ok() && sha_hex(&exe) == man.x {
            install(&exe)
        } else {
            Err("Prüfsumme falsch".into())
        };
        match res {
            Ok(()) => {
                s.upd.lock().unwrap().state = format!("Update {} installiert – startet neu", man.ver);
                log(format!("Update {} installiert – startet neu", man.ver));
                s.emit(Some("upd"), json!({"ver": man.ver}).to_string());
                std::thread::sleep(Duration::from_millis(1500));
                crate::restart_self(&s);
            }
            Err(e) => {
                let mut u = s.upd.lock().unwrap();
                u.installing = false;
                u.state = format!("Update fehlgeschlagen: {e}");
                log(&u.state);
            }
        }
    });
}

// laufende exe umbenennen (das erlaubt Windows) und die neue an ihre Stelle legen
fn install(bytes: &[u8]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or(EXE_NAME).to_string();
    let neu = exe.with_file_name(format!("{name}.neu"));
    let alt = exe.with_file_name(format!("{name}.alt"));
    std::fs::write(&neu, bytes).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&alt);
    std::fs::rename(&exe, &alt).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&neu, &exe) {
        let _ = std::fs::rename(&alt, &exe);
        return Err(e.to_string());
    }
    Ok(())
}

// fehlende Teile beim Herausgeber nachfragen
fn upd_request_missing(s: &Shared) {
    let now = now_ms();
    let ask = {
        let mut u = s.upd.lock().unwrap();
        let ask = u.man.is_some() && !u.installing && s.mq_ok() && now.saturating_sub(u.t0) > 15000 && now.saturating_sub(u.last_req) > 60000;
        if ask { u.last_req = now; }
        ask
    };
    if ask { publish(s, &format!("{}/req", updx_topic()), b"1", false); }
}
