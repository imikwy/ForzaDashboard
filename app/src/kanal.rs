// Verschlüsselter Kanal über das Netz (für Lobbys und Bearbeiten-Sitzungen).
// Aus dem Einladungscode entstehen getrennt: der Themen-Name (wo die Nachrichten liegen) und der Schlüssel
// (womit sie verschlüsselt sind). scrypt macht das Durchprobieren von Codes teuer. Der Vermittler sieht weder
// Namen noch Positionen noch Straßen. Große Nachrichten werden in Stücke geteilt und wieder zusammengesetzt.
use crate::mqtt::Net;
use crate::util::{log, norm_code, now_ms, random_hex};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock};

const PART: usize = 60_000;

#[derive(Clone)]
pub struct Chan {
    pub topic: String,
    key: [u8; 32],
}

pub fn channel(kind: &str, code: &str) -> Chan {
    static CACHE: OnceLock<Mutex<HashMap<String, Chan>>> = OnceLock::new();
    let norm = norm_code(code);
    let ck = format!("{kind}:{norm}");
    let cache = CACHE.get_or_init(Default::default);
    if let Some(c) = cache.lock().unwrap().get(&ck) { return c.clone(); }
    let mut master = [0u8; 32];
    let params = scrypt::Params::new(14, 8, 1, 32).expect("scrypt");
    let _ = scrypt::scrypt(norm.as_bytes(), format!("fd-kanal-v1:{kind}").as_bytes(), &params, &mut master);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&master).expect("hmac");
    mac.update(b"thema");
    let topic_id = hex::encode(mac.finalize().into_bytes())[..32].to_string();
    let mut key = [0u8; 32];
    let _ = hkdf::Hkdf::<Sha256>::new(None, &master).expand(b"schluessel", &mut key);
    let ch = Chan { topic: format!("fdx/{kind}/{topic_id}"), key };
    cache.lock().unwrap().insert(ck, ch.clone());
    ch
}

// Format: [Version 1 Byte][IV 12][Tag 16][Daten]; Version 2 = Daten vor dem Verschlüsseln komprimiert
pub fn seal(ch: &Chan, v: &Value) -> Vec<u8> {
    let mut pt = v.to_string().into_bytes();
    let mut ver = 1u8;
    if pt.len() > 600 {
        let mut e = DeflateEncoder::new(Vec::new(), Compression::new(6));
        let _ = e.write_all(&pt);
        pt = e.finish().unwrap_or_default();
        ver = 2;
    }
    let mut iv = [0u8; 12];
    let _ = getrandom::fill(&mut iv);
    let cipher = Aes256Gcm::new_from_slice(&ch.key).expect("key");
    let Ok(ct) = cipher.encrypt(Nonce::from_slice(&iv), Payload { msg: &pt, aad: ch.topic.as_bytes() }) else { return Vec::new() };
    // aes-gcm hängt das Prüf-Tag hinten an – im Format steht es vorn
    let (data, tag) = ct.split_at(ct.len() - 16);
    let mut out = vec![ver];
    out.extend_from_slice(&iv);
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    out
}

pub fn open(ch: &Chan, buf: &[u8]) -> Option<Value> {
    if buf.len() < 30 || (buf[0] != 1 && buf[0] != 2) { return None; }
    let mut ct = buf[29..].to_vec();
    ct.extend_from_slice(&buf[13..29]);
    let cipher = Aes256Gcm::new_from_slice(&ch.key).ok()?;
    let pt = cipher.decrypt(Nonce::from_slice(&buf[1..13]), Payload { msg: &ct, aad: ch.topic.as_bytes() }).ok()?;
    let pt = if buf[0] == 2 {
        let mut out = Vec::new();
        DeflateDecoder::new(&pt[..]).take(64 << 20).read_to_end(&mut out).ok()?;
        out
    } else { pt };
    serde_json::from_slice(&pt).ok()
}

pub type OnMsg = Arc<dyn Fn(Value, &str) + Send + Sync>;

struct Parts {
    n: usize,
    got: HashMap<usize, String>,
    t: u64,
}

pub struct Kanal {
    net: Arc<Net>,
    pub ch: Chan,
    handler: u64,
}

impl Kanal {
    pub fn new(net: &Arc<Net>, kind: &str, code: &str, on_msg: OnMsg) -> Kanal {
        let ch = channel(kind, code);
        let prefix = format!("{}/", ch.topic);
        let parts: Mutex<HashMap<String, Parts>> = Mutex::new(HashMap::new());
        let ch2 = ch.clone();
        let handler = net.on_message(Arc::new(move |topic: &str, payload: &[u8]| {
            let Some(sub) = topic.strip_prefix(&prefix) else { return };
            if payload.is_empty() { return; }
            let Some(o) = open(&ch2, payload) else { return };
            if o["y"] != "~" { return on_msg(o, sub); }
            // Stück einer großen Nachricht
            let (Some(id), Some(i), Some(n), Some(s)) = (o["id"].as_str(), o["i"].as_u64(), o["n"].as_u64(), o["s"].as_str()) else { return };
            if n == 0 || n >= 2000 || i >= n { return; }
            let full = {
                let mut ps = parts.lock().unwrap();
                let now = now_ms();
                ps.retain(|_, p| now.saturating_sub(p.t) < 90_000);
                let p = ps.entry(id.to_string()).or_insert_with(|| Parts { n: n as usize, got: HashMap::new(), t: now });
                p.got.insert(i as usize, s.to_string());
                if p.got.len() < p.n { return; }
                let p = ps.remove(id).unwrap();
                (0..p.n).map(|k| p.got.get(&k).cloned().unwrap_or_default()).collect::<String>()
            };
            match serde_json::from_str::<Value>(&full) {
                Ok(v) => on_msg(v, sub),
                Err(_) => log("Kanal: zusammengesetzte Nachricht unlesbar"),
            }
        }));
        net.subscribe(&format!("{}/#", ch.topic));
        Kanal { net: net.clone(), ch, handler }
    }

    pub fn topic(&self) -> &str {
        &self.ch.topic
    }

    pub fn send(&self, v: &Value) -> bool {
        self.send_to(v, "m", false)
    }

    pub fn send_to(&self, v: &Value, sub: &str, retain: bool) -> bool {
        let s = v.to_string();
        let t = format!("{}/{sub}", self.ch.topic);
        if s.len() <= PART { return self.net.publish(&t, &seal(&self.ch, v), retain); }
        let id = random_hex(6);
        let chars: Vec<char> = s.chars().collect();
        let pieces: Vec<String> = chars.chunks(PART).map(|c| c.iter().collect()).collect();
        let n = pieces.len();
        let mut ok = true;
        for (i, p) in pieces.into_iter().enumerate() {
            ok &= self.net.publish(&t, &seal(&self.ch, &json!({"y": "~", "id": id, "i": i, "n": n, "s": p})), false);
        }
        ok
    }
}

impl Drop for Kanal {
    fn drop(&mut self) {
        self.net.off(self.handler);
        self.net.unsubscribe(&format!("{}/#", self.ch.topic));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip() {
        let ch = channel("test", "K7M2QX9PA");
        let small = json!({"y": "hb", "n": "Anna"});
        assert_eq!(open(&ch, &seal(&ch, &small)), Some(small));
        let big = json!({"y": "lr", "s": "x".repeat(5000)});
        let b = seal(&ch, &big);
        assert_eq!(b[0], 2);
        assert_eq!(open(&ch, &b), Some(big.clone()));
        // falscher Code oder veränderte Daten -> nichts
        assert_eq!(open(&channel("test", "K7M2QX9PB"), &b), None);
        let mut bad = b.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert_eq!(open(&ch, &bad), None);
    }
}
