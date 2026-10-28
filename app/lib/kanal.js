// Verschlüsselter Kanal über das Netz (für Lobbys und Bearbeiten-Sitzungen).
// Große Nachrichten werden automatisch in Stücke geteilt und beim Empfänger wieder zusammengesetzt.
"use strict";
const crypto = require("crypto");
const {channel, seal, open} = require("./netz");

const PART = 60000;   // Zeichen pro Stück (öffentliche Vermittler mögen keine riesigen Nachrichten)

class Kanal {
  constructor(net, kind, code, onMsg){
    this.net = net; this.ch = channel(kind, code); this.onMsg = onMsg; this.parts = new Map(); this.closed = false;
    this.sub = this.ch.topic + "/#";
    this.handler = (topic, payload, retained) => {
      if (this.closed || !topic.startsWith(this.ch.topic + "/")) return;
      if (!payload.length) return;
      const o = open(this.ch, payload); if (!o) return;
      const sub = topic.slice(this.ch.topic.length + 1);
      if (o.y === "~") return this.part(o, sub, retained);
      try { this.onMsg(o, sub, retained); } catch (e) { require("./util").log("Kanal-Fehler:", e.stack || e.message); }
    };
    net.on("message", this.handler);
    net.subscribe(this.sub);
  }
  get topic(){ return this.ch.topic; }
  send(obj, sub, retain){
    const s = JSON.stringify(obj), t = this.ch.topic + "/" + (sub || "m");
    if (s.length <= PART) return this.net.publish(t, seal(this.ch, obj), !!retain);
    const id = crypto.randomBytes(6).toString("hex"), n = Math.ceil(s.length / PART);
    let ok = true;
    for (let i = 0; i < n; i++) ok = this.net.publish(t, seal(this.ch, {y:"~", id, i, n, s:s.slice(i * PART, (i + 1) * PART)}), false) && ok;
    return ok;
  }
  // gespeicherte Nachricht beim Vermittler löschen
  clear(sub){ this.net.publish(this.ch.topic + "/" + sub, Buffer.alloc(0), true); }
  part(o, sub, retained){
    if (!(o.n > 0 && o.n < 2000) || !(o.i >= 0 && o.i < o.n)) return;
    let p = this.parts.get(o.id);
    if (!p) { p = {n:o.n, got:new Map(), t:Date.now()}; this.parts.set(o.id, p); }
    p.got.set(o.i, o.s);
    const now = Date.now(); for (const [k, q] of this.parts) if (now - q.t > 90000) this.parts.delete(k);
    if (p.got.size < p.n) return;
    this.parts.delete(o.id);
    let full; try { full = JSON.parse([...Array(p.n).keys()].map(i => p.got.get(i)).join("")); } catch (e) { return; }
    try { this.onMsg(full, sub, retained); } catch (e) { require("./util").log("Kanal-Fehler:", e.stack || e.message); }
  }
  close(){ this.closed = true; this.net.off("message", this.handler); this.net.unsubscribe(this.sub); }
}
module.exports = {Kanal};
