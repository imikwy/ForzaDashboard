// Gemeinsamer Zustand für Telemetrie, Webserver, Freunde und Updates
use crate::settings::{self, Settings};
use crate::{media, net, shared};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{broadcast, mpsc, Notify};

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

#[derive(Default)]
pub struct Flr {
    pub list: Map<String, Value>,
    pub v: u64,
    pub pub_v: i64,
    pub pub_t: u64,
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
    pub last_len: AtomicUsize,
    pub tx: broadcast::Sender<Ev>,
    pub my_pos: Mutex<Option<MyPos>>,
    pub friends: Mutex<HashMap<String, Friend>>,
    pub mq: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    pub mq_ok: AtomicBool,
    pub mq_topic: Mutex<String>,
    pub mq_restart: Notify,
    pub mq_pid: AtomicU16,
    pub last_pub: AtomicU64,
    pub flr: Mutex<Flr>,
    pub map: Mutex<shared::MapDoc>,
    pub upd: Mutex<net::Upd>,
    pub lver: AtomicU64,
    pub lease: Mutex<(String, u64)>,
    pub media: Mutex<media::Media>,
    pub media_wanted: AtomicU64,
    pub app: OnceLock<tauri::AppHandle>,
    pub priv_key: Option<ed25519_dalek::SigningKey>,
}

impl Shared {
    pub fn new(root: PathBuf) -> Self {
        let cfg = settings::load(&root.join("einstellungen.txt"));
        let lver = std::fs::read_to_string(root.join("gelernte-strassen.version")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let mut flr = Flr { pub_v: -1, ..Default::default() };
        if let Ok(Value::Object(o)) = std::fs::read_to_string(root.join("freunde-strassen.json")).map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)) {
            flr.list = o;
            flr.v = 1;
        }
        let priv_key = std::fs::read_to_string(root.join("herausgeber.key")).ok().and_then(|pem| {
            use ed25519_dalek::pkcs8::DecodePrivateKey;
            ed25519_dalek::SigningKey::from_pkcs8_pem(&pem).ok()
        });
        let map = shared::MapDoc::load(&root);
        let (tx, _) = broadcast::channel(256);
        Shared {
            sid: cfg.sid.clone(),
            udp_port: AtomicU16::new(cfg.udp_port),
            web_port: AtomicU16::new(cfg.web_port),
            cfg: Mutex::new(cfg),
            root,
            port_changed: AtomicBool::new(false),
            latest: Mutex::new(None),
            packets: AtomicU64::new(0),
            last_len: AtomicUsize::new(0),
            tx,
            my_pos: Mutex::new(None),
            friends: Mutex::new(HashMap::new()),
            mq: Mutex::new(None),
            mq_ok: AtomicBool::new(false),
            mq_topic: Mutex::new(String::new()),
            mq_restart: Notify::new(),
            mq_pid: AtomicU16::new(1),
            last_pub: AtomicU64::new(0),
            flr: Mutex::new(flr),
            map: Mutex::new(map),
            upd: Mutex::new(net::Upd::default()),
            lver: AtomicU64::new(lver),
            lease: Mutex::new((String::new(), 0)),
            media: Mutex::new(media::Media::default()),
            media_wanted: AtomicU64::new(0),
            app: OnceLock::new(),
            priv_key,
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

    pub fn mq_ok(&self) -> bool {
        self.mq_ok.load(Ordering::Relaxed)
    }
}
