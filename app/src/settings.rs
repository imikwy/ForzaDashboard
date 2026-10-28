// einstellungen.txt lesen und schreiben (gleiches Format wie die Node-Version)
use crate::util::{random_hex, take_chars};
use std::path::Path;

pub const DEFAULT_GROUP: &str = "fd-mattis-crew-7k3q";

pub struct Settings {
    pub udp_port: u16,
    pub web_port: u16,
    pub open_on_start: bool,
    pub fr_on: bool,
    pub fr_group: String,
    pub fr_name: String,
    pub sid: String,
    pub spv_on: bool,
}

const TEMPLATE: &str = "# ForzaDashboard Einstellungen\r\n# Im Spiel: Einstellungen > HUD und Gameplay > Data Out = An,\r\n# Data Out IP-Adresse = 127.0.0.1, Data Out IP-Port = gleicher Wert wie udp_port\r\nudp_port=5400\r\nweb_port=8080\r\nbeim_start_oeffnen=ja\r\n";

fn is_off(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "nein" | "0" | "false" | "aus")
}

pub fn load(path: &Path) -> Settings {
    let mut name = take_chars(&std::env::var("USERNAME").unwrap_or_default(), 24);
    if name.is_empty() { name = "Fahrer".into(); }
    let mut s = Settings {
        udp_port: 5400, web_port: 8080, open_on_start: true, fr_on: true,
        fr_group: DEFAULT_GROUP.into(), fr_name: name, sid: String::new(), spv_on: true,
    };
    if !path.exists() { let _ = std::fs::write(path, TEMPLATE); }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_lowercase(), v.trim());
        match k.as_str() {
            "udp_port" => if let Ok(p) = v.parse::<u16>() { if p > 0 { s.udp_port = p; } },
            "web_port" => if let Ok(p) = v.parse::<u16>() { if p > 0 { s.web_port = p; } },
            "beim_start_oeffnen" => s.open_on_start = !matches!(v.to_lowercase().as_str(), "nein" | "0" | "false"),
            "freunde" => s.fr_on = !is_off(v),
            "gruppe" if !v.is_empty() => s.fr_group = take_chars(v, 40),
            "name" if !v.is_empty() => s.fr_name = take_chars(v, 24),
            "id" if v.len() == 10 && v.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) => s.sid = v.into(),
            "spotify_lautstaerke" => s.spv_on = !is_off(v),
            _ => {}
        }
    }
    if s.sid.is_empty() {
        s.sid = random_hex(5);
        set(path, "id", &s.sid);
    }
    s
}

// Eine Zeile "key=wert" ersetzen oder anhängen
pub fn set(path: &Path, key: &str, val: &str) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let mut found = false;
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let l = line.trim_end_matches('\r');
        let is_key = !found && l.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='));
        if is_key {
            found = true;
            out.push(format!("{key}={val}{}", if line.ends_with('\r') { "\r" } else { "" }));
        } else {
            out.push(line.to_string());
        }
    }
    let mut t = out.join("\n");
    if !found { t = format!("{}\r\n{key}={val}\r\n", t.trim_end()); }
    let _ = std::fs::write(path, t);
}
