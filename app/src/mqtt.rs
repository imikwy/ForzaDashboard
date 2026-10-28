// Netz: schlanker MQTT-3.1.1-Client über öffentliche Vermittler.
// Es wird gleichzeitig mit ZWEI Vermittlern gesprochen. Fällt einer aus oder ist er im Netz gesperrt, läuft alles
// über den anderen weiter – und zwei PCs landen nie auf getrennten Vermittlern. Doppelt ankommende Nachrichten
// werden aussortiert. Was über Lobbys und Sitzungen läuft, ist zusätzlich verschlüsselt (siehe kanal.rs).
use crate::util::{log, now_ms, random_hex, sha_hex};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, timeout};

// je Verbindung eine eigene Liste: verschlüsselt zuerst, unverschlüsselt als Notlösung, wenn 8883 gesperrt ist
const SLOTS: [&[(&str, u16, bool)]; 2] = [
    &[("broker.hivemq.com", 8883, true), ("test.mosquitto.org", 8886, true), ("broker.hivemq.com", 1883, false)],
    &[("broker.emqx.io", 8883, true), ("test.mosquitto.org", 8886, true), ("broker.emqx.io", 1883, false)],
];

pub type Handler = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

#[derive(Default)]
struct Link {
    tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    ok: AtomicBool,
    host: Mutex<String>,
}

pub struct Net {
    client_id: String,
    links: [Link; 2],
    pid: AtomicU16,
    subs: Mutex<BTreeSet<String>>,
    seen: Mutex<HashMap<String, u64>>,
    handlers: Mutex<Vec<(u64, Handler)>>,
    next_h: AtomicU64,
    stopped: AtomicBool,
    #[cfg(test)]
    pub test: Mutex<Option<Arc<TestBus>>>,
    #[cfg(test)]
    pub offline: AtomicBool,
    #[cfg(test)]
    pub part: AtomicU64,
}

// Für Tests: mehrere "PCs" in einem Prozess, verbunden wie über einen Vermittler (offline = Netz getrennt)
#[cfg(test)]
#[derive(Default)]
pub struct TestBus {
    pub nets: Mutex<Vec<std::sync::Weak<Net>>>,
    queue: Mutex<Option<std::sync::mpsc::Sender<(Arc<Net>, String, Vec<u8>)>>>,
}

#[cfg(test)]
impl TestBus {
    pub fn net(self: &Arc<Self>, id: &str) -> Arc<Net> {
        let n = Net::new(id);
        *n.test.lock().unwrap() = Some(self.clone());
        self.nets.lock().unwrap().push(Arc::downgrade(&n));
        n
    }

    // zustellen wie ein Vermittler: nacheinander, aber nie im Aufruf des Absenders
    fn post(&self, n: Arc<Net>, topic: &str, payload: &[u8]) {
        let mut q = self.queue.lock().unwrap();
        let tx = q.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<(Arc<Net>, String, Vec<u8>)>();
            std::thread::spawn(move || { for (n, t, p) in rx { n.deliver(&t, &p); } });
            tx
        });
        let _ = tx.send((n, topic.to_string(), payload.to_vec()));
    }
}

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

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

impl Net {
    pub fn new(client_id: &str) -> Arc<Net> {
        Arc::new(Net {
            client_id: client_id.to_string(),
            links: Default::default(),
            pid: AtomicU16::new(1),
            subs: Mutex::new(BTreeSet::new()),
            seen: Mutex::new(HashMap::new()),
            handlers: Mutex::new(Vec::new()),
            next_h: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
            #[cfg(test)]
            test: Mutex::new(None),
            #[cfg(test)]
            offline: AtomicBool::new(false),
            #[cfg(test)]
            part: AtomicU64::new(0),
        })
    }

    pub fn start(self: &Arc<Self>) {
        for slot in 0..2 {
            let n = self.clone();
            tauri::async_runtime::spawn(async move { n.run(slot).await });
        }
        let n = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut t = interval(Duration::from_secs(30));
            loop {
                t.tick().await;
                let now = now_ms();
                n.seen.lock().unwrap().retain(|_, t| now.saturating_sub(*t) < 120_000);
            }
        });
    }

    pub fn ok(&self) -> bool {
        #[cfg(test)]
        if self.test.lock().unwrap().is_some() { return !self.offline.load(Ordering::Relaxed); }
        self.links.iter().any(|l| l.ok.load(Ordering::Relaxed))
    }

    pub fn brokers(&self) -> Vec<String> {
        self.links.iter().filter(|l| l.ok.load(Ordering::Relaxed)).map(|l| l.host.lock().unwrap().clone()).collect()
    }

    pub fn on_message(&self, h: Handler) -> u64 {
        let id = self.next_h.fetch_add(1, Ordering::Relaxed);
        self.handlers.lock().unwrap().push((id, h));
        id
    }

    pub fn off(&self, id: u64) {
        self.handlers.lock().unwrap().retain(|(i, _)| *i != id);
    }

    fn send_all(&self, pkt: &[u8]) -> bool {
        let mut any = false;
        for l in &self.links {
            if !l.ok.load(Ordering::Relaxed) { continue; }
            if let Some(tx) = l.tx.lock().unwrap().as_ref() { any |= tx.send(pkt.to_vec()).is_ok(); }
        }
        any
    }

    pub fn publish(&self, topic: &str, payload: &[u8], retain: bool) -> bool {
        #[cfg(test)]
        if let Some(bus) = self.test.lock().unwrap().clone() {
            if self.offline.load(Ordering::Relaxed) { return false; }
            let nets: Vec<Arc<Net>> = bus.nets.lock().unwrap().iter().filter_map(|w| w.upgrade()).collect();
            for n in nets {
                if n.offline.load(Ordering::Relaxed) || n.part.load(Ordering::Relaxed) != self.part.load(Ordering::Relaxed) { continue; }
                let hit = n.subs.lock().unwrap().iter().any(|f| f.strip_suffix("/#").map_or(f == topic, |p| topic.starts_with(&format!("{p}/"))));
                if hit { bus.post(n, topic, payload); }
            }
            return true;
        }
        let mut body = mq_str(topic.as_bytes());
        body.extend_from_slice(payload);
        self.send_all(&mq_pkt(if retain { 0x31 } else { 0x30 }, &body))
    }

    fn sub_pkt(&self, filters: &[String], unsub: bool) -> Vec<u8> {
        let pid = self.pid.fetch_add(1, Ordering::Relaxed) % 60000 + 1;
        let mut body = pid.to_be_bytes().to_vec();
        for f in filters {
            body.extend(mq_str(f.as_bytes()));
            if !unsub { body.push(0); }
        }
        mq_pkt(if unsub { 0xA2 } else { 0x82 }, &body)
    }

    pub fn subscribe(&self, f: &str) {
        if !self.subs.lock().unwrap().insert(f.to_string()) { return; }
        self.send_all(&self.sub_pkt(&[f.to_string()], false));
    }

    pub fn unsubscribe(&self, f: &str) {
        if !self.subs.lock().unwrap().remove(f) { return; }
        self.send_all(&self.sub_pkt(&[f.to_string()], true));
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.send_all(&[0xE0, 0]);
    }

    fn deliver(&self, topic: &str, payload: &[u8]) {
        let mut key = topic.to_string();
        key.push('|');
        key.push_str(&sha_hex(payload)[..24]);
        {
            let mut seen = self.seen.lock().unwrap();
            if seen.contains_key(&key) { return; }
            seen.insert(key, now_ms());
        }
        let hs: Vec<Handler> = self.handlers.lock().unwrap().iter().map(|(_, h)| h.clone()).collect();
        for h in hs { h(topic, payload); }
    }

    async fn run(self: Arc<Self>, slot: usize) {
        let list = SLOTS[slot];
        let (mut bi, mut wait) = (0usize, 2000u64);
        while !self.stopped.load(Ordering::Relaxed) {
            let (host, port, secure) = list[bi % list.len()];
            let connected = self.session(slot, host, port, secure).await;
            if connected { wait = 2000; } else { bi += 1; }
            sleep(Duration::from_millis(wait)).await;
            wait = (wait as f64 * 1.6).min(30000.0) as u64;
        }
    }

    // eine Verbindung bis zum Abbruch; true = sie stand zwischendurch
    async fn session(&self, slot: usize, host: &str, port: u16, secure: bool) -> bool {
        let link = &self.links[slot];
        let Ok(Ok(tcp)) = timeout(Duration::from_secs(10), TcpStream::connect((host, port))).await else { return false };
        let _ = tcp.set_nodelay(true);
        let io: Box<dyn Io> = if secure {
            let Ok(cx) = native_tls::TlsConnector::new() else { return false };
            match timeout(Duration::from_secs(10), tokio_native_tls::TlsConnector::from(cx).connect(host, tcp)).await {
                Ok(Ok(t)) => Box::new(t),
                _ => return false,
            }
        } else {
            Box::new(tcp)
        };
        let (mut rd, mut wr) = tokio::io::split(io);
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let mut connect = mq_str(b"MQTT");
        connect.extend_from_slice(&[4, 0x02, 0, 60]);
        connect.extend(mq_str(format!("{}-{}", self.client_id, random_hex(2)).as_bytes()));
        let _ = tx.send(mq_pkt(0x10, &connect));
        let writer = tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                if wr.write_all(&p).await.is_err() { break; }
            }
        });

        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = vec![0u8; 65536];
        let mut connected = false;
        let mut ping = interval(Duration::from_secs(20));
        ping.tick().await;
        loop {
            tokio::select! {
                r = timeout(Duration::from_secs(50), rd.read(&mut tmp)) => match r {
                    Ok(Ok(n)) if n > 0 => {
                        buf.extend_from_slice(&tmp[..n]);
                        if !self.packets(link, &mut buf, &tx, &mut connected, host) { break; }
                    }
                    _ => break,
                },
                _ = ping.tick() => {
                    if self.stopped.load(Ordering::Relaxed) { break; }
                    if connected { let _ = tx.send(vec![0xC0, 0]); }
                }
            }
        }
        if connected { log(format!("Verbindung zu {host} getrennt")); }
        link.ok.store(false, Ordering::Relaxed);
        *link.tx.lock().unwrap() = None;
        drop(tx);
        sleep(Duration::from_millis(50)).await;
        writer.abort();
        connected
    }

    // false = Verbindung beenden
    fn packets(&self, link: &Link, buf: &mut Vec<u8>, tx: &mpsc::UnboundedSender<Vec<u8>>, connected: &mut bool, host: &str) -> bool {
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
                    if body.get(1) != Some(&0) { return false; }
                    *connected = true;
                    *link.host.lock().unwrap() = host.to_string();
                    *link.tx.lock().unwrap() = Some(tx.clone());
                    link.ok.store(true, Ordering::Relaxed);
                    log(format!("Verbunden mit {host}"));
                    let subs: Vec<String> = self.subs.lock().unwrap().iter().cloned().collect();
                    if !subs.is_empty() { let _ = tx.send(self.sub_pkt(&subs, false)); }
                }
                3 => {
                    if body.len() < 2 { continue; }
                    let tl = u16::from_be_bytes([body[0], body[1]]) as usize;
                    let start = 2 + tl + if qos > 0 { 2 } else { 0 };
                    if body.len() < start { continue; }
                    let t = String::from_utf8_lossy(&body[2..2 + tl]).to_string();
                    self.deliver(&t, &body[start..]);
                }
                _ => {}
            }
        }
    }
}
