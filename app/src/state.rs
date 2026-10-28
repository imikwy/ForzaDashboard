// Gemeinsamer Zustand für Telemetrie, Webserver, Karte, Sitzungen, Lobbys und Updates
use crate::karte::Karten;
use crate::lobby::Lobbys;
use crate::mqtt::Net;
use crate::settings::{self, Settings};
use crate::sitzung::Sitzungen;
use crate::{media, updates};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Ev {
    pub name: Option<&'static str>,
    pub data: Arc<str>,
}

#[derive(Clone, Copy)]
pub struct MyPos {
    pub x: f64,
    pub z: f64,
    pub h: f64,
    pub v: f64,
    pub c: i64,
    pub t: u64,
}

#[derive(Clone)]
pub struct Friend {
    pub n: String,
    pub x: f64,
    pub z: f64,
    pub h: f64,
    pub v: f64,
    pub c: i64,
    pub g: u8,
    pub seen: u64,
}

// gelernte Straßen der Freunde (bleiben auf diesem PC, gehören nicht zur Karte)
#[derive(Default)]
pub struct Flr {
    pub list: Map<String, Value>,
    pub v: u64,
}

pub struct Shared {
    pub root: PathBuf,
    pub cfg: Mutex<Settings>,
    pub sid: String,
    pub udp_port: AtomicU16,
    pub web_port: AtomicU16,
    pub port_changed: AtomicBool,
    pub latest: Mutex<Option<Arc<str>>>,
    pub packets: AtomicU64,
    pub last_packet: AtomicU64,
    pub last_len: AtomicUsize,
    pub tx: broadcast::Sender<Ev>,
    pub my_pos: Mutex<Option<MyPos>>,
    pub flr: Mutex<Flr>,
    pub lver: AtomicU64,
    pub lease: Mutex<(String, u64)>,
    pub media: Mutex<media::Media>,
    pub media_wanted: AtomicU64,
    pub app: OnceLock<tauri::AppHandle>,
    pub net: Arc<Net>,
    pub karten: Arc<Karten>,
    pub lokal: Mutex<Map<String, Value>>,
    pub sitz: OnceLock<Sitzungen>,
    pub lobbys: OnceLock<Arc<Lobbys>>,
    pub upd: Mutex<updates::Upd>,
    pub pub_cap: Mutex<(u64, Value)>,
}

impl Shared {
    pub fn new(root: PathBuf) -> Self {
        let cfg = settings::load(&root.join("einstellungen.txt"));
        let lver = std::fs::read_to_string(root.join("gelernte-strassen.version")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let mut flr = Flr::default();
        if let Ok(Value::Object(o)) = std::fs::read_to_string(root.join("freunde-strassen.json")).map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)) {
            flr.list = o;
            flr.v = 1;
        }
        // nur auf diesem PC: Tacho-Zuordnung je Auto, eigene Kartenausrichtung (nur wenn die Karte keine hat)
        let lokal: Map<String, Value> = crate::util::read_json(&root.join("karte-lokal.json"))
            .and_then(|v| v["ents"].as_array().cloned())
            .map(|a| a.into_iter().filter_map(|e| Some((e["id"].as_str()?.to_string(), e))).collect())
            .unwrap_or_default();
        let (tx, _) = broadcast::channel(256);
        Shared {
            sid: cfg.sid.clone(),
            udp_port: AtomicU16::new(cfg.udp_port),
            web_port: AtomicU16::new(cfg.web_port),
            net: Net::new(&format!("fd4-{}", cfg.sid)),
            karten: Arc::new(Karten::new(&root)),
            cfg: Mutex::new(cfg),
            root,
            port_changed: AtomicBool::new(false),
            latest: Mutex::new(None),
            packets: AtomicU64::new(0),
            last_packet: AtomicU64::new(0),
            last_len: AtomicUsize::new(0),
            tx,
            my_pos: Mutex::new(None),
            flr: Mutex::new(flr),
            lver: AtomicU64::new(lver),
            lease: Mutex::new((String::new(), 0)),
            media: Mutex::new(media::Media::default()),
            media_wanted: AtomicU64::new(0),
            app: OnceLock::new(),
            lokal: Mutex::new(lokal),
            sitz: OnceLock::new(),
            lobbys: OnceLock::new(),
            upd: Mutex::new(updates::Upd::default()),
            pub_cap: Mutex::new((0, Value::Null)),
        }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn set_setting(&self, key: &str, val: &str) {
        settings::set(&self.file("einstellungen.txt"), key, val);
    }

    pub fn emit(&self, name: Option<&'static str>, data: String) {
        let _ = self.tx.send(Ev { name, data: data.into() });
    }

    pub fn sitz(&self) -> &Sitzungen {
        self.sitz.get().expect("Sitzungen")
    }

    pub fn lobbys(&self) -> &Arc<Lobbys> {
        self.lobbys.get().expect("Lobbys")
    }

    pub fn name(&self) -> String {
        self.cfg.lock().unwrap().fr_name.clone()
    }

    pub fn fr_on(&self) -> bool {
        self.cfg.lock().unwrap().fr_on
    }

    pub fn net_ok(&self) -> bool {
        self.net.ok()
    }

    pub fn packets(&self) -> u64 {
        self.packets.load(Ordering::Relaxed)
    }
}
