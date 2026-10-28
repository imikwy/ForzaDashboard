// Netz: schlanker MQTT-3.1.1-Client (ohne Zusatzpakete) + Ende-zu-Ende-Verschlüsselung.
// - Es wird gleichzeitig mit ZWEI öffentlichen Vermittlern gesprochen. Fällt einer aus oder ist er im Netz
//   gesperrt, läuft alles über den anderen weiter – und zwei PCs landen nie auf getrennten Vermittlern.
// - Alles, was über einen Kanal (Lobby, Sitzung) läuft, ist mit AES-256-GCM verschlüsselt. Der Schlüssel
//   entsteht nur aus dem Einladungscode – der Vermittler sieht weder Namen noch Positionen noch Straßen.
"use strict";
const net = require("net"), tls = require("tls"), crypto = require("crypto"), zlib = require("zlib"), EventEmitter = require("events");
const {log, normCode, sha256} = require("./util");

const DEFAULT_BROKERS = [
  ["broker.hivemq.com", 8883, true], ["broker.emqx.io", 8883, true], ["test.mosquitto.org", 8886, true],
  ["broker.hivemq.com", 1883, false], ["broker.emqx.io", 1883, false],
];

function mqStr(s){ const b = Buffer.isBuffer(s) ? s : Buffer.from(s, "utf8"), l = Buffer.alloc(2); l.writeUInt16BE(b.length); return Buffer.concat([l, b]); }
function mqLen(n){ const out = []; do { let d = n % 128; n = Math.floor(n / 128); if (n > 0) d |= 128; out.push(d); } while (n > 0); return Buffer.from(out); }
function mqPkt(type, body){ return Buffer.concat([Buffer.from([type]), mqLen(body.length), body]); }

// Eine Verbindung zu einem Vermittler, mit automatischem Neuverbinden
class Link extends EventEmitter {
  constructor(broker, clientId){ super(); this.broker = broker; this.clientId = clientId; this.ok = false; this.subs = new Set(); this.pid = 1; this.wait = 2000; this.closed = false; this.fails = 0; this.connect(); }
  get name(){ return this.broker[0] + ":" + this.broker[1]; }
  connect(){
    if (this.closed) return;
    const [host, port, secure] = this.broker;
    this.buf = Buffer.alloc(0); this.ok = false;
    const s = secure ? tls.connect({host, port, servername:host, timeout:15000}) : net.connect({host, port, timeout:15000});
    this.sock = s; s.setNoDelay(true); s.setTimeout(45000);
    s.on(secure ? "secureConnect" : "connect", () => {
      const vh = Buffer.concat([mqStr("MQTT"), Buffer.from([4, 0x02, 0, 60])]);   // MQTT 3.1.1, clean session, 60 s keepalive
      s.write(mqPkt(0x10, Buffer.concat([vh, mqStr(this.clientId + "-" + crypto.randomBytes(2).toString("hex"))])));
    });
    s.on("data", d => this.onData(s, d));
    s.on("timeout", () => s.destroy());
    s.on("error", () => {});
    s.on("close", () => {
      clearInterval(this.pingT);
      const was = this.ok; this.ok = false;
      if (this.sock !== s) return;
      this.sock = null; if (was) this.emit("down"); else this.fails++;
      if (this.closed) return;
      setTimeout(() => this.connect(), this.wait); this.wait = Math.min(30000, this.wait * 1.6);
    });
  }
  onData(s, d){
    if (this.sock !== s) return;
    this.buf = Buffer.concat([this.buf, d]);
    for (;;) {
      if (this.buf.length < 2) return;
      let mul = 1, len = 0, i = 1, b;
      do { if (i >= this.buf.length) return; b = this.buf[i++]; len += (b & 127) * mul; mul *= 128; } while (b & 128 && i < 5);
      if (this.buf.length < i + len) return;
      const type = this.buf[0] >> 4, qos = (this.buf[0] >> 1) & 3, retain = this.buf[0] & 1, body = this.buf.subarray(i, i + len);
      this.buf = this.buf.subarray(i + len);
      if (type === 2) {                                   // CONNACK
        if (body[1] !== 0) { s.destroy(); return; }
        this.ok = true; this.wait = 2000; this.fails = 0;
        if (this.subs.size) this.sendSub([...this.subs]);
        clearInterval(this.pingT); this.pingT = setInterval(() => this.write(Buffer.from([0xC0, 0])), 30000);
        this.emit("up");
      } else if (type === 3) {                            // PUBLISH
        const tl = body.readUInt16BE(0), topic = body.subarray(2, 2 + tl).toString("utf8");
        const payload = Buffer.from(body.subarray(2 + tl + (qos ? 2 : 0)));
        this.emit("message", topic, payload, !!retain);
      }
    }
  }
  write(b){ if (this.sock && this.ok) { try { this.sock.write(b); return true; } catch (e) {} } return false; }
  sendSub(filters){
    const pid = Buffer.alloc(2); pid.writeUInt16BE((this.pid = this.pid % 60000 + 1));
    this.write(mqPkt(0x82, Buffer.concat([pid, ...filters.map(f => Buffer.concat([mqStr(f), Buffer.from([0])]))])));
  }
  sendUnsub(filters){
    const pid = Buffer.alloc(2); pid.writeUInt16BE((this.pid = this.pid % 60000 + 1));
    this.write(mqPkt(0xA2, Buffer.concat([pid, ...filters.map(f => mqStr(f))])));
  }
  subscribe(f){ if (this.subs.has(f)) return; this.subs.add(f); if (this.ok) this.sendSub([f]); }
  unsubscribe(f){ if (!this.subs.delete(f)) return; if (this.ok) this.sendUnsub([f]); }
  publish(topic, payload, retain){ return this.write(mqPkt(retain ? 0x31 : 0x30, Buffer.concat([mqStr(topic), Buffer.isBuffer(payload) ? payload : Buffer.from(payload, "utf8")]))); }
  close(){ this.closed = true; this.ok = false; clearInterval(this.pingT); if (this.sock) { try { this.write(Buffer.from([0xE0, 0])); this.sock.end(); } catch (e) {} } }
}

// Zwei gleichzeitige Verbindungen; doppelt ankommende Nachrichten werden aussortiert.
class Net extends EventEmitter {
  constructor(clientId, brokers){
    super(); this.clientId = clientId; this.brokers = brokers && brokers.length ? brokers : DEFAULT_BROKERS;
    this.links = []; this.subs = new Set(); this.seen = new Map(); this.next = 0; this.started = false;
  }
  start(){
    if (this.started) return; this.started = true;
    for (let i = 0; i < Math.min(2, this.brokers.length); i++) this.addLink();
    // ein Vermittler kommt gar nicht zustande -> nächsten aus der Liste probieren
    this.watchT = setInterval(() => {
      for (const l of this.links) if (!l.ok && l.fails >= 3 && this.brokers.length > 2) { this.links.splice(this.links.indexOf(l), 1); l.close(); this.addLink(); break; }
      const now = Date.now(); for (const [k, t] of this.seen) if (now - t > 120000) this.seen.delete(k);
    }, 5000);
    this.watchT.unref();
  }
  addLink(){
    const used = new Set(this.links.map(l => l.broker[0]));
    let b = null;
    for (let k = 0; k < this.brokers.length; k++) { const c = this.brokers[(this.next + k) % this.brokers.length]; if (!used.has(c[0])) { b = c; this.next = (this.next + k + 1) % this.brokers.length; break; } }
    if (!b) return;
    const l = new Link(b, this.clientId);
    for (const f of this.subs) l.subscribe(f);
    l.on("up", () => { log("Verbunden mit " + l.name); this.emit("up", l.name); });
    l.on("down", () => { log("Verbindung zu " + l.name + " getrennt"); if (!this.ok) this.emit("down"); });
    l.on("message", (topic, payload, retained) => {
      const k = topic + "|" + sha256(payload).slice(0, 24);
      if (this.seen.has(k)) return; this.seen.set(k, Date.now());
      this.emit("message", topic, payload, retained);
    });
    this.links.push(l);
  }
  get ok(){ return this.links.some(l => l.ok); }
  get brokerNames(){ return this.links.filter(l => l.ok).map(l => l.broker[0]); }
  subscribe(f){ this.subs.add(f); for (const l of this.links) l.subscribe(f); }
  unsubscribe(f){ this.subs.delete(f); for (const l of this.links) l.unsubscribe(f); }
  publish(topic, payload, retain){ let any = false; for (const l of this.links) any = l.publish(topic, payload, retain) || any; return any; }
  stop(){ clearInterval(this.watchT); for (const l of this.links) l.close(); this.links = []; this.started = false; }
}

// ---------------- Verschlüsselte Kanäle ----------------
// Aus dem Code entstehen getrennt: der Themen-Name (wo die Nachrichten liegen) und der Schlüssel (womit sie
// verschlüsselt sind). scrypt macht Durchprobieren von Codes teuer.
const chanCache = new Map();
function channel(kind, code){
  const norm = normCode(code), ck = kind + ":" + norm;
  if (chanCache.has(ck)) return chanCache.get(ck);
  const master = crypto.scryptSync(norm, "fd-kanal-v1:" + kind, 32, {N:1 << 14, r:8, p:1});
  const topicId = crypto.createHmac("sha256", master).update("thema").digest("hex").slice(0, 32);
  const key = Buffer.from(crypto.hkdfSync("sha256", master, Buffer.alloc(0), Buffer.from("schluessel"), 32));
  const ch = {kind, code:norm, topic:"fdx/" + kind + "/" + topicId, key};
  chanCache.set(ck, ch); return ch;
}
// Format: [Version 1 Byte][IV 12][Tag 16][Daten]; Version 2 = Daten vor dem Verschlüsseln komprimiert
function seal(ch, obj){
  let pt = Buffer.from(JSON.stringify(obj), "utf8"), ver = 1;
  if (pt.length > 600) { pt = zlib.deflateRawSync(pt, {level:6}); ver = 2; }
  const iv = crypto.randomBytes(12), c = crypto.createCipheriv("aes-256-gcm", ch.key, iv);
  c.setAAD(Buffer.from(ch.topic));
  const ct = Buffer.concat([c.update(pt), c.final()]);
  return Buffer.concat([Buffer.from([ver]), iv, c.getAuthTag(), ct]);
}
function open(ch, buf){
  try {
    if (!buf || buf.length < 30 || (buf[0] !== 1 && buf[0] !== 2)) return null;
    const d = crypto.createDecipheriv("aes-256-gcm", ch.key, buf.subarray(1, 13));
    d.setAAD(Buffer.from(ch.topic)); d.setAuthTag(buf.subarray(13, 29));
    let pt = Buffer.concat([d.update(buf.subarray(29)), d.final()]);
    if (buf[0] === 2) pt = zlib.inflateRawSync(pt, {maxOutputLength:64 << 20});
    return JSON.parse(pt.toString("utf8"));
  } catch (e) { return null; }   // falscher Schlüssel, verändert oder kaputt -> ignorieren
}

module.exports = {Net, Link, channel, seal, open, DEFAULT_BROKERS};
