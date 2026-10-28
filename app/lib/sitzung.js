// Bearbeiten-Sitzungen: mehrere Editoren (2, 3 oder mehr) arbeiten gleichzeitig an derselben Karte.
//
// So bleibt nichts verloren und alle sehen dasselbe:
// - Ein PC ist "Host" und legt die Reihenfolge aller Änderungen fest (fortlaufende Nummer = seq).
//   Damit gibt es keine Uhrzeit-Konflikte mehr: Jeder wendet dieselben Schritte in derselben Reihenfolge an.
// - Jede Änderung wird auf JEDEM PC sofort ins Journal (ops.jsonl) geschrieben. Absturz/Neustart = nichts weg.
// - Eigene Änderungen, die der Host noch nicht bestätigt hat, bleiben "ausstehend" (pending.json) und werden
//   so lange erneut geschickt, bis sie bestätigt sind – auch nach einem Neustart oder wenn der Host offline war.
// - Ist der Host weg, übernimmt automatisch der Teilnehmer mit dem vollständigsten Stand (Amtszeit "term" +1).
// - Grundlage ist immer eine veröffentlichte Kartenversion (base). Fehlt sie einem Teilnehmer, schickt der Host sie
//   mit (signiert, wird wie ein Download geprüft).
"use strict";
const fs = require("fs"), path = require("path"), zlib = require("zlib"), EventEmitter = require("events"), crypto = require("crypto");
const {P} = require("./basis");
const {readJson, writeJson, writeAtomic, appendLine, log} = require("./util");
const {Kanal} = require("./kanal");
const K = require("./kartenpruefung");

const HB = 3000, HOST_TIMEOUT = 11000, PEER_TIMEOUT = 15000, RETRY = 3000, BATCH = 400;

const strip = e => { const o = Object.assign({}, e); delete o.ts; delete o.a; return o; };
const TYP = {motor:"Autobahn", road:"Straße", gravel:"Schotter", off:"Offroad", tunnel:"Tunnel"};
const POI = {park:"Parkplatz", meet:"Car-Meet", fav:"Favorit", photo:"Aussicht", home:"Zuhause", fuel:"Tankstelle", other:"Ort"};
const km = p => { let L = 0; for (let i = 2; i + 1 < p.length; i += 2) L += Math.hypot(p[i] - p[i - 2], p[i + 1] - p[i - 1]); return (L / 1000).toFixed(2).replace(".", ","); };
const live = e => e && e.d !== 1;

// Bezeichnung eines Schritts aus dem Vorher/Nachher der betroffenen Objekte
function describe(ents, before){
  const c = {rNeu:0, rGe:0, rDel:0, fix:0, x:0, xAus:0, e:0, oNeu:0, oGe:0, oDel:0, cfg:[]};
  let road = null;
  for (const e of ents) {
    const b = before(e.id);
    if (e.k === "r") { if (!live(e)) c.rDel++; else if (!live(b)) { if (e.id.startsWith("fh:")) c.fix++; else c.rNeu++; road = e; } else { c.rGe++; road = e; } }
    else if (e.k === "x") { if (live(e)) c.x++; else c.xAus++; }
    else if (e.k === "e") { if (live(e)) c.e++; }
    else if (e.k === "o") { if (!live(e)) c.oDel++; else if (!live(b)) c.oNeu++; else c.oGe++; }
    else if (e.k === "c") c.cfg.push(e.id);
  }
  const n = ents.length, onlyRoad = c.rNeu + c.rGe + c.fix + c.rDel === 1 && n - c.x <= 1;
  const t = road ? (TYP[road.t] || "Straße") + (road.p ? " · " + km(road.p) + " km" : "") : "";
  if (n && c.e === n) return "Radiert" + (n > 1 ? " · " + n + " Stellen" : "");
  if (onlyRoad && c.rNeu) return "Neue Straße · " + t;
  if (onlyRoad && c.fix) return "Forza-Straße korrigiert · " + t;
  if (onlyRoad && c.rGe) return (c.x ? "Gelernte Straße übernommen · " : "Straße geändert · ") + t;
  if (onlyRoad && c.rDel) return c.x ? "Korrektur entfernt" : "Straße gelöscht";
  if (n === c.x) return n > 1 ? n + " Straßen ausgeblendet" : "Straße gelöscht";
  if (n === c.xAus) return "Wieder eingeblendet";
  if (n === 1 && (c.oNeu || c.oGe || c.oDel)) { const p = ents[0], k = POI[p.kind] || (live(before(p.id)) && POI[before(p.id).kind]) || "Ort"; return (c.oNeu ? "Ort gesetzt · " : c.oGe ? "Ort geändert · " : "Ort gelöscht · ") + k; }
  if (c.cfg.length === n) return c.cfg.includes("cfg:roadfix") ? "Straßenkorrekturen übernommen" : c.cfg.includes("cfg:mapfit") ? "Kartenausrichtung geändert" : "Einstellung geändert";
  const parts = [];
  if (c.rNeu + c.fix) parts.push((c.rNeu + c.fix) + " Straßen neu"); if (c.rGe) parts.push(c.rGe + " geändert"); if (c.rDel + c.x) parts.push((c.rDel + c.x) + " gelöscht");
  if (c.e) parts.push(c.e + " Radierungen"); if (c.oNeu + c.oGe + c.oDel) parts.push((c.oNeu + c.oGe + c.oDel) + " Orte");
  return "Sammel-Änderung · " + (parts.join(", ") || n + " Objekte");
}

class Sitzung extends EventEmitter {
  // info: {code, title, created, base, term, owner}; ctx: {net, karten, dev, name()}
  constructor(info, ctx){
    super();
    this.info = info; this.ctx = ctx; this.dev = ctx.dev;
    this.dir = path.join(ctx.dir || P.SITZUNGEN, info.code);
    fs.mkdirSync(this.dir, {recursive:true});
    this.ops = []; this.chain = []; this.byId = new Map(); this.touch = new Map(); this.held = new Map();
    this.pending = new Map(); this.peers = new Map(); this.baseParts = null;
    this.role = "member"; this.term = info.term || 0; this.hostDev = null; this.lastHost = 0;
    this.lastHb = 0; this.lastRetry = 0; this.lastNeed = 0; this.lastT = 0; this.subN = 0; this.closed = false;
    this.loadJournal();
    for (const s of readJson(path.join(this.dir, "pending.json"), [])) if (s && s.id && !this.byId.has(s.id)) this.pending.set(s.id, s);
    this.materialize();
  }

  // ---------------- Journal ----------------
  get journal(){ return path.join(this.dir, "ops.jsonl"); }
  loadJournal(){
    let txt = ""; try { txt = fs.readFileSync(this.journal, "utf8"); } catch (e) { return; }
    for (const line of txt.split("\n")) {
      if (!line.trim()) continue;
      let op; try { op = JSON.parse(line); } catch (e) { log("Sitzung " + this.info.code + ": beschädigte Journal-Zeile übersprungen"); continue; }   // halbe letzte Zeile nach Absturz
      if (op.s !== this.ops.length + 1) continue;
      this.push(op);
    }
    if (this.ops.length) this.lastT = this.ops[this.ops.length - 1].t;
  }
  // Prüfsummen-Kette über die Reihenfolge: zwei PCs mit gleichem Kettenwert an Stelle s haben bis s exakt denselben Verlauf
  push(op){
    this.chain.push(crypto.createHash("sha256").update((this.chain.length ? this.chain[this.chain.length - 1] : "") + "|" + op.id).digest("hex").slice(0, 16));
    this.ops.push(op); this.byId.set(op.id, op);
    for (const e of op.ents) { let a = this.touch.get(e.id); if (!a) this.touch.set(e.id, a = []); a.push(op.s); }
  }
  rewriteJournal(){ writeAtomic(this.journal, this.ops.map(o => JSON.stringify(o)).join("\n") + (this.ops.length ? "\n" : "")); }
  savePending(){ writeJson(path.join(this.dir, "pending.json"), [...this.pending.values()]); }
  saveInfo(){ this.info.term = this.term; writeJson(path.join(this.dir, "sitzung.json"), this.info, true); }

  // ---------------- Zustand ----------------
  get seq(){ return this.ops.length; }
  baseEnts(){ return this.info.base ? this.ctx.karten.ents(this.info.base) : null; }
  materialize(){
    const base = this.baseEnts();
    this.ready = !!base;
    this.state = new Map((base || []).map(e => [e.id, e]));
    this.baseMaxTs = (base || []).reduce((m, e) => Math.max(m, e.ts || 0), 0);
    for (const op of this.ops) this.apply(op);
  }
  apply(op){
    for (const e of op.ents) {
      const cur = this.state.get(e.id);
      // nach dem Umstellen auf eine neuere Kartenversion gewinnt dort die jeweils neuere Änderung
      if (cur && (cur.ts || 0) > op.t) continue;
      this.state.set(e.id, Object.assign({}, e, {ts:op.t, a:op.by}));
    }
  }
  // Ansicht für das Dashboard: bestätigter Stand + eigene, noch ausstehende Änderungen
  view(){
    if (!this.pending.size) return [...this.state.values()];
    const m = new Map(this.state);
    for (const s of this.pending.values()) for (const e of s.ents) m.set(e.id, Object.assign({}, e, {ts:s.t0, a:this.dev}));
    return [...m.values()];
  }
  // Wert eines Objekts direkt vor Schritt seq
  before(id, seq){
    const a = this.touch.get(id);
    if (a) for (let i = a.length - 1; i >= 0; i--) if (a[i] < seq) { const op = this.ops[a[i] - 1]; const e = op.ents.find(x => x.id === id); return Object.assign({}, e, {ts:op.t, a:op.by}); }
    const base = this.baseEnts(); if (!base) return undefined;
    if (!this.baseIdx || this.baseIdx.v !== this.info.base) this.baseIdx = {v:this.info.base, m:new Map(base.map(e => [e.id, e]))};
    return this.baseIdx.m.get(id);
  }

  // ---------------- Start / Netz ----------------
  start(asHost){
    this.kanal = new Kanal(this.ctx.net, "edit", this.info.code, (o, sub) => this.onMsg(o, sub));
    this.lastHost = Date.now();   // erst auf den Host warten, bevor dieser PC selbst übernimmt
    if (asHost) this.becomeHost(true);
    this.timer = setInterval(() => this.tick(), 1000); this.timer.unref();
    this.sendSync();
    this.hb();
  }
  // Abgleich mit dem Host: Kettenwerte an den Stellen seq, seq-1, seq-2, seq-4, … – der Host findet so schnell die
  // letzte gemeinsame Stelle und schickt alles danach (auch wenn sich die Verläufe getrennt entwickelt haben)
  sendSync(){
    const pts = []; for (let k = 0, s = this.seq; s > 0; s = this.seq - (2 ** k++)) pts.push([s, this.chain[s - 1]]);
    pts.push([0, ""]); this.send({y:"sync", pts, b:this.info.base || 0});
  }
  chainAt(s){ return s <= 0 ? "" : this.chain[s - 1]; }
  stop(){
    if (this.closed) return; this.closed = true;
    try { this.send({y:"bye"}); } catch (e) {}
    clearInterval(this.timer); if (this.kanal) this.kanal.close();
  }
  send(o){ if (!this.kanal) return false; o.d = this.dev; return this.kanal.send(o); }
  hb(){ this.lastHb = Date.now(); this.send({y:"hb", n:this.ctx.name(), r:this.role, tm:this.term, s:this.seq, b:this.info.base || 0, ow:this.info.owner, ti:this.info.title || ""}); }
  becomeHost(initial){
    const maxTm = Math.max(this.term, ...[...this.peers.values()].map(p => p.tm || 0));
    this.role = "host"; this.hostDev = this.dev; this.term = initial && !this.ops.length ? Math.max(1, maxTm) : maxTm + 1; this.lastHost = Date.now();
    this.saveInfo();
    log("Sitzung " + this.info.code + ": dieser PC ist jetzt Host (Amtszeit " + this.term + ")");
    this.hb();
    for (const s of [...this.pending.values()]) this.accept(s);
    this.emit("status");
  }
  stepDown(dev, tm){
    if (this.role === "host") log("Sitzung " + this.info.code + ": " + dev + " ist Host (Amtszeit " + tm + ") – dieser PC macht als Teilnehmer weiter");
    this.role = "member"; this.hostDev = dev; this.term = tm; this.lastHost = Date.now(); this.saveInfo();
    this.sendSync();
    this.emit("status");
  }
  tick(){
    const now = Date.now();
    if (now - this.lastHb >= HB) this.hb();
    for (const [d, p] of this.peers) if (now - p.last > PEER_TIMEOUT) { this.peers.delete(d); this.emit("status"); }
    if (this.role === "member") {
      if (!this.ctx.net.ok) this.lastHost = Math.max(this.lastHost, now - HOST_TIMEOUT + 6000);   // offline: nach dem Verbinden erst zuhören
      else if (now - this.lastHost > HOST_TIMEOUT) this.elect();
      else if (this.pending.size && this.hostDev && now - this.lastRetry > RETRY) {
        this.lastRetry = now;
        for (const s of this.pending.values()) this.send({y:"sub", op:s});
      }
    }
  }
  // Host fehlt: der Teilnehmer mit dem vollständigsten Stand übernimmt (gleich -> kleinste Kennung)
  elect(){
    const now = Date.now();
    const cands = [{d:this.dev, s:this.seq}, ...[...this.peers.entries()].filter(([, p]) => now - p.last < HOST_TIMEOUT).map(([d, p]) => ({d, s:p.s}))];
    cands.sort((a, b) => b.s - a.s || (a.d < b.d ? -1 : 1));
    if (cands[0].d === this.dev && this.ready) this.becomeHost(false);
    else this.lastHost = now - HOST_TIMEOUT + 4000;   // dem anderen kurz Zeit lassen
  }
  better(tm, d){ return tm > this.term || (tm === this.term && (!this.hostDev || d < this.hostDev)); }

  onMsg(o){
    if (!o || o.d === this.dev || this.closed) return;
    const now = Date.now();
    if (o.y === "hb") {
      const p = this.peers.get(o.d) || {}; const isNew = !this.peers.has(o.d);
      Object.assign(p, {n:String(o.n || "Freund").slice(0, 24), r:o.r, tm:o.tm | 0, s:o.s | 0, b:o.b | 0, last:now}); this.peers.set(o.d, p);
      if (o.r === "host") {
        if (o.d === this.hostDev && o.tm === this.term) this.lastHost = now;
        else if (this.better(o.tm | 0, o.d) || (this.role === "member" && now - this.lastHost > HOST_TIMEOUT)) this.stepDown(o.d, o.tm | 0);
        else if (this.role === "host") this.hb();   // der andere soll merken, dass es hier einen besseren Host gibt
        if (o.d === this.hostDev) {
          if (o.b && o.b !== this.info.base) this.rebase(o.b);
          if (o.s > this.seq && now - this.lastNeed > 2000) { this.lastNeed = now; this.send({y:"need", from:this.seq + 1}); }
          if (!this.info.title && o.ti) { this.info.title = String(o.ti).slice(0, 40); this.saveInfo(); }
          if (!this.info.owner && o.ow) { this.info.owner = o.ow; this.saveInfo(); }
        }
      }
      if (isNew) this.emit("status");
      return;
    }
    if (o.y === "sync") {
      if (this.role !== "host" || !Array.isArray(o.pts)) return;
      let match = 0;
      for (const [s, h] of o.pts.slice().sort((a, b) => b[0] - a[0])) if (s === 0 || (s <= this.seq && this.chainAt(s) === h)) { match = s; break; }
      this.hb(); this.send({y:"syncok", to:o.d, s:match, hs:this.seq, hh:this.chainAt(this.seq)});
      this.sendOps(match + 1); return;
    }
    if (o.y === "syncok") {
      // dieser PC hat Schritte, die der Host nicht kennt (z. B. von einem Host, der inzwischen abgelöst ist) -> neu einreichen
      if (o.to === this.dev && o.d === this.hostDev && this.seq > (o.hs | 0) && this.chainAt(o.hs | 0) === o.hh) { this.truncate((o.hs | 0) + 1); this.emit("change", {reload:true}); }
      return;
    }
    if (o.y === "need") { if (this.role === "host") this.sendOps(o.from | 0); return; }
    if (o.y === "sub") { if (this.role === "host" && o.op) this.accept(o.op); return; }
    if (o.y === "ops") { if (o.d === this.hostDev || this.better(o.tm | 0, o.d)) { if (o.d !== this.hostDev) this.stepDown(o.d, o.tm | 0); this.receive(o.list || []); } return; }
    if (o.y === "rej") { if (this.pending.delete(o.id)) { this.savePending(); this.emit("change", {reload:true}); this.emit("note", "Eine Änderung wurde abgelehnt: " + (o.why || "ungültig")); } return; }
    if (o.y === "pr") { this.emit("presence", o); return; }
    if (o.y === "bye") { this.peers.delete(o.d); if (o.d === this.hostDev) this.lastHost = 0; this.emit("status"); return; }
    if (o.y === "bq") { if (this.role === "host") this.sendBase(o.v | 0); return; }
    if (o.y === "bp") { this.baseRecv(o); return; }
    if (o.y === "pub") { this.emit("note", (o.n || "Ein Editor") + " hat die Karte veröffentlicht – die neue Version kommt in 1–2 Minuten automatisch"); return; }
  }

  // ---------------- Host: Änderungen annehmen ----------------
  accept(sub){
    if (this.byId.has(sub.id)) { this.broadcast([this.byId.get(sub.id)]); this.dropPending(sub.id); return; }
    const ents = [], bad = [];
    for (const raw of Array.isArray(sub.ents) ? sub.ents : []) {
      const e = K.canonEnt(strip(raw)); delete e._op;
      const err = K.checkEnt(e); if (err) bad.push(err); else ents.push(e);
    }
    if (!ents.length) {
      this.send({y:"rej", id:sub.id, why:bad[0] || "leer"});
      if (this.pending.delete(sub.id)) { this.savePending(); this.emit("change", {reload:true}); }
      return;
    }
    const s = this.seq + 1, t = Math.max(Date.now(), this.lastT + 1, (this.baseMaxTs || 0) + 1);   // immer neuer als alles Bisherige
    const before = id => this.state.get(id);
    let lb;
    if (sub.u) { const g = this.group(sub.u); lb = g ? (g.isUndo ? "Wiederholt: " + g.lb.replace(/^Rückgängig: /, "") : "Rückgängig: " + g.lb) : "Rückgängig"; }
    else lb = describe(ents, before);
    const op = {s, id:String(sub.id).slice(0, 40), by:String(sub.by || this.dev).slice(0, 12), n:String(sub.n || "Freund").slice(0, 24), t, tm:this.term, ents, grp:sub.grp ? String(sub.grp).slice(0, 40) : null, u:sub.u || null, lb};
    this.commit(op);
    this.broadcast([op]);
  }
  commit(op){
    this.push(op); this.lastT = op.t; this.apply(op);
    appendLine(this.journal, JSON.stringify(op));
    this.dropPending(op.id);
    this.emit("change", {ents:op.ents.map(e => this.state.get(e.id)).filter(Boolean)});
  }
  dropPending(id){ if (this.pending.delete(id)) this.savePending(); }
  broadcast(list){ this.send({y:"ops", tm:this.term, list}); }
  sendOps(from){
    from = Math.max(1, from | 0);
    for (let i = from; i <= this.seq; i += BATCH) this.send({y:"ops", tm:this.term, list:this.ops.slice(i - 1, i - 1 + BATCH)});
  }

  // ---------------- Teilnehmer: Änderungen vom Host übernehmen ----------------
  receive(list){
    list = list.filter(op => op && op.s > 0 && Array.isArray(op.ents)).sort((a, b) => a.s - b.s);
    let changed = false, reload = false;
    for (const op of list) {
      if (op.s <= this.seq) {
        if (this.ops[op.s - 1].id === op.id) continue;
        // eigener Stand weicht ab (z. B. zwei Hosts während einer Netz-Störung): ab hier dem Host folgen,
        // die eigenen abweichenden Schritte werden neu eingereicht – es geht nichts verloren
        this.truncate(op.s); reload = true;
      }
      if (op.s === this.seq + 1) {
        if (!op.ents.every(e => !K.checkEnt(e))) { log("Sitzung: ungültiger Schritt vom Host ignoriert"); break; }
        this.push(op); this.lastT = op.t; this.apply(op); appendLine(this.journal, JSON.stringify(op)); this.dropPending(op.id); changed = true;
        if (!reload) this.emit("change", {ents:op.ents.map(e => this.state.get(e.id)).filter(Boolean)});
      } else this.held.set(op.s, op);
    }
    // vorher zu früh angekommene Schritte nachholen
    while (this.held.has(this.seq + 1)) { const op = this.held.get(this.seq + 1); this.held.delete(op.s); this.receive([op]); }
    if (this.held.size && Date.now() - this.lastNeed > 2000) { this.lastNeed = Date.now(); this.send({y:"need", from:this.seq + 1}); }
    if (reload) this.emit("change", {reload:true});
    if (changed) this.emit("status");
  }
  truncate(s){
    const keep = this.ops.slice(0, s - 1), removed = this.ops.slice(s - 1);
    try { fs.copyFileSync(this.journal, this.journal + ".vor-abgleich-" + Date.now()); } catch (e) {}   // alter Stand bleibt als Datei liegen
    this.ops = []; this.chain = []; this.byId = new Map(); this.touch = new Map();
    for (const op of keep) this.push(op);
    this.lastT = keep.length ? keep[keep.length - 1].t : 0;
    this.rewriteJournal(); this.materialize();
    for (const op of removed) if (!this.pending.has(op.id)) this.pending.set(op.id, {id:op.id, by:op.by, n:op.n, t0:op.t, ents:op.ents, grp:op.grp, u:op.u});
    this.savePending();
    log("Sitzung " + this.info.code + ": " + removed.length + " Schritte werden neu eingereicht (Abgleich mit dem Host)");
  }
  // ---------------- Basis (Kartenversion) ----------------
  rebase(v){
    if (v === this.info.base) return;
    this.info.base = v; this.saveInfo();
    if (!this.ctx.karten.has(v)) { this.ready = false; this.send({y:"bq", v}); this.emit("status"); return; }
    this.materialize(); this.emit("change", {reload:true}); this.emit("status");
  }
  sendBase(v){
    const r = this.ctx.karten.raw(v); if (!r || !r.man) return;
    const gz = zlib.gzipSync(r.buf).toString("base64"), PARTB = 45000, n = Math.ceil(gz.length / PARTB);
    for (let i = 0; i < n; i++) this.send({y:"bp", v, i, n, man:i === 0 ? r.man : undefined, data:gz.slice(i * PARTB, (i + 1) * PARTB)});
  }
  baseRecv(o){
    if (o.v !== this.info.base || this.ctx.karten.has(o.v)) return;
    if (!this.baseParts || this.baseParts.v !== o.v || this.baseParts.n !== o.n) this.baseParts = {v:o.v, n:o.n, got:new Map(), man:null};
    const bp = this.baseParts; bp.got.set(o.i, o.data); if (o.man) bp.man = o.man;
    if (bp.got.size < bp.n || !bp.man) return;
    this.baseParts = null;
    try {
      const buf = zlib.gunzipSync(Buffer.from([...Array(bp.n).keys()].map(i => bp.got.get(i)).join(""), "base64"));
      const err = this.ctx.karten.install(bp.man, buf, {source:"Sitzung"});
      if (err) { log("Kartenversion aus der Sitzung abgelehnt:", err); return; }
      this.materialize(); this.emit("change", {reload:true}); this.emit("status");
    } catch (e) { log("Kartenversion aus der Sitzung kaputt:", e.message); }
  }

  // ---------------- Eigene Änderungen ----------------
  submit(ents, meta){
    meta = meta || {};
    const sub = {id:this.dev + "-" + Date.now().toString(36) + "-" + (++this.subN).toString(36) + crypto.randomBytes(2).toString("hex"),
      by:this.dev, n:this.ctx.name(), t0:Date.now(), ents:ents.map(e => K.canonEnt(strip(e))), grp:meta.grp || null, u:meta.u || null};
    this.pending.set(sub.id, sub); this.savePending();
    if (this.role === "host") this.accept(sub);
    else { this.send({y:"sub", op:sub}); this.lastRetry = Date.now(); }
    return sub;
  }

  // ---------------- Verlauf ----------------
  // Schritte mit gleicher Gruppe (z. B. ein Radier-Wisch) vom selben Editor zählen als EIN Eintrag
  groups(){
    const out = [], idx = new Map();
    for (const op of this.ops) {
      const key = op.grp ? op.by + ":" + op.grp : "s" + op.s;
      let g = idx.get(key);
      if (!g) { g = {g:key, seqs:[], by:op.by, n:op.n, t:op.t, t1:op.t, lb:op.lb, isUndo:!!op.u, u:op.u, nE:0, er:0}; idx.set(key, g); out.push(g); }
      g.seqs.push(op.s); g.t1 = op.t; g.nE += op.ents.length;
      if (op.ents.every(e => e.k === "e")) g.er += op.ents.length;
      if (g.er === g.nE && g.nE > 1) g.lb = "Radiert · " + g.nE + " Stellen";
    }
    // rückgängig gemacht? (von hinten nach vorn: ein Rückgängig zählt nur, wenn es selbst noch gilt)
    const undone = new Map();
    for (let i = out.length - 1; i >= 0; i--) {
      const g = out[i]; g.undone = !!undone.get(g.g);
      if (g.isUndo && !g.undone) undone.set(g.u, true);
    }
    this._groups = out; this._gidx = new Map(out.map(g => [g.g, g]));
    return out;
  }
  group(key){ if (!this._gidx || this._groupsAt !== this.seq) { this.groups(); this._groupsAt = this.seq; } return this._gidx.get(key); }
  history(){
    const gs = this.groups(); this._groupsAt = this.seq;
    return gs.map(g => ({g:g.g, by:g.by, n:g.n, t:g.t, t1:g.t1, lb:g.lb, undone:g.undone, isUndo:g.isUndo, u:g.u || null, steps:g.seqs.length, objs:g.nE})).reverse();
  }
  // Vorher/Nachher eines Eintrags – zum Hervorheben auf der Karte
  detail(key){
    const g = this.group(key); if (!g) return null;
    const first = g.seqs[0], last = g.seqs[g.seqs.length - 1], ids = new Map();
    for (const s of g.seqs) for (const e of this.ops[s - 1].ents) ids.set(e.id, s);
    const items = [];
    for (const [id] of ids) {
      const b = this.before(id, first) || null, a = this.valueAfter(id, last);
      items.push({id, before:b ? strip(b) : null, after:a ? strip(a) : null});
    }
    return {g:g.g, lb:g.lb, n:g.n, by:g.by, t:g.t, undone:g.undone, items};
  }
  valueAfter(id, seq){ return this.before(id, seq + 1); }
  // Eintrag rückgängig machen. Hat jemand die Objekte danach noch geändert, wird zuerst nachgefragt.
  undo(key, force){
    const g = this.group(key); if (!g) return {ok:false, why:"Schritt nicht gefunden"};
    if (g.undone) return {ok:false, why:"Schon rückgängig gemacht"};
    const first = g.seqs[0], last = g.seqs[g.seqs.length - 1], ids = new Set();
    for (const s of g.seqs) for (const e of this.ops[s - 1].ents) ids.add(e.id);
    const later = new Map();
    for (const id of ids) for (const s of this.touch.get(id) || []) if (s > last) { const op = this.ops[s - 1]; later.set(op.by, op.n); }
    for (const sub of this.pending.values()) for (const e of sub.ents) if (ids.has(e.id)) later.set(this.dev, "dir");
    if (later.size && !force) return {ok:false, conflict:[...later.values()], why:"Danach noch geändert"};
    const ents = [];
    for (const id of ids) {
      const b = this.before(id, first);
      if (b) ents.push(strip(b));
      else { const cur = this.state.get(id); ents.push({id, k:cur ? cur.k : (id.startsWith("x:") ? "x" : "r"), d:1}); }
    }
    this.submit(ents, {u:g.g});
    return {ok:true};
  }
  // Strg+Z: letzter eigener, noch gültiger Schritt; Strg+Y: letztes eigenes Rückgängig wieder aufheben
  lastOwn(redo){
    const gs = this.groups(); this._groupsAt = this.seq;
    for (let i = gs.length - 1; i >= 0; i--) { const g = gs[i]; if (g.by !== this.dev || g.undone) continue; if (redo ? g.isUndo : !g.isUndo) return g.g; }
    return null;
  }

  status(){
    const now = Date.now();
    const members = [{d:this.dev, n:this.ctx.name(), r:this.role, me:true, s:this.seq}];
    for (const [d, p] of this.peers) if (now - p.last < PEER_TIMEOUT) members.push({d, n:p.n, r:p.r, s:p.s});
    return {code:this.info.code, title:this.info.title || "", base:this.info.base || 0, ready:this.ready, role:this.role, host:this.hostDev, term:this.term,
      seq:this.seq, pending:this.pending.size, members, owner:this.info.owner, created:this.info.created, net:this.ctx.net.ok,
      hostOk:this.role === "host" || now - this.lastHost < HOST_TIMEOUT};
  }
}

module.exports = {Sitzung, describe};
