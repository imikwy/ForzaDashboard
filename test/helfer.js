// Gemeinsame Test-Helfer: eigener, leerer Datenordner pro Testdatei und ein Netz ohne Internet.
"use strict";
const fs = require("fs"), os = require("os"), path = require("path"), crypto = require("crypto"), EventEmitter = require("events");

const DATA = fs.mkdtempSync(path.join(os.tmpdir(), "fd-test-"));
process.env.FD_DATA = DATA;
process.env.FD_QUIET = "1";
process.env.FD_DEVICE_ID = process.env.FD_DEVICE_ID || "test000000";

// Test-Schlüssel statt des echten Herausgeber-Schlüssels
const keys = crypto.generateKeyPairSync("ed25519");
const PEM = keys.privateKey.export({type:"pkcs8", format:"pem"});
const PUB = keys.publicKey.export({type:"spki", format:"der"}).toString("base64");
const basis = require("../app/lib/basis");
basis.CONFIG.signaturSchluessel = PUB;

// Netz-Attrappe: alle "PCs" in diesem Prozess hängen am selben Bus; Nachrichten kommen asynchron an
class Bus { constructor(){ this.nets = new Set(); this.drop = () => false; } }
class FakeNet extends EventEmitter {
  constructor(bus){ super(); this.bus = bus; this.ok = true; this.subs = new Set(); bus.nets.add(this); }
  subscribe(f){ this.subs.add(f); }
  unsubscribe(f){ this.subs.delete(f); }
  publish(topic, payload){
    if (!this.ok) return false;
    for (const n of this.bus.nets) {
      if (!n.ok || this.bus.drop(this, n, topic)) continue;
      if (![...n.subs].some(f => f.endsWith("/#") ? topic.startsWith(f.slice(0, -2)) : f === topic)) continue;
      const p = Buffer.from(payload); setImmediate(() => n.emit("message", topic, p, false));
    }
    return true;
  }
}
const wait = ms => new Promise(r => setTimeout(r, ms));
async function until(fn, ms, what){
  const t0 = Date.now();
  while (Date.now() - t0 < (ms || 5000)) { if (fn()) return; await wait(20); }
  throw new Error("Zeitüberschreitung: " + (what || "Bedingung"));
}
module.exports = {DATA, PEM, PUB, Bus, FakeNet, wait, until};
