// Spiel-Lobbys: Freunde live auf der Karte – einfach und sicher.
// - Eine Lobby wird EINMAL erstellt (oder per Code beigetreten) und bleibt gespeichert (daten/lobbys.json).
//   Danach reicht: Spiel starten, Dashboard starten, losfahren – die Verbindung kommt von selbst.
// - Geteilt werden nur Name, Position, Tempo und die selbst gelernten Straßen. Alles verschlüsselt mit dem
//   Lobby-Code. Im Spielmodus kann niemand die Karte verändern – das geht nur in einer Bearbeiten-Sitzung.
"use strict";
const fs = require("fs"), EventEmitter = require("events");
const {P, S} = require("./basis");
const {readJson, writeJson, log, newCode, normCode, validCode, fmtCode} = require("./util");
const {Kanal} = require("./kanal");
const {seal} = require("./netz");

const CODE_LEN = 9, LR_MAX = 180000;

class Lobbys extends EventEmitter {
  constructor(ctx){
    super(); this.ctx = ctx;   // {net, dev, name(), learnedFile, onFriendRoads}
    const o = readJson(P.LOBBYS, {liste:[]});
    this.liste = (o.liste || []).filter(l => validCode(l.code, CODE_LEN));
    this.kanaele = new Map(); this.friends = new Map(); this.myPos = null; this.lastPub = 0; this.lrSent = new Map();
  }
  save(){ writeJson(P.LOBBYS, {liste:this.liste}, true); }
  start(){
    for (const l of this.liste) if (l.an !== false) this.open(l);
    this.posT = setInterval(() => this.pubPos(), 500); this.posT.unref();
    this.cleanT = setInterval(() => { const now = Date.now(); for (const [d, f] of this.friends) if (now - f.seen > 12000) this.friends.delete(d); }, 2000); this.cleanT.unref();
  }
  open(l){
    if (this.kanaele.has(l.code)) return;
    const k = new Kanal(this.ctx.net, "play", l.code, (o, sub) => this.onMsg(l, o, sub));
    this.kanaele.set(l.code, k);
    setTimeout(() => this.pubRoads(true), 1500);
  }
  close(code, bye){
    const k = this.kanaele.get(code); if (!k) return;
    if (bye) k.send({y:"bye", d:this.ctx.dev});
    k.close(); this.kanaele.delete(code);
    for (const [d, f] of this.friends) { f.lobbys.delete(code); if (!f.lobbys.size) this.friends.delete(d); }
  }
  onMsg(l, o, sub){
    if (!o || o.d === this.ctx.dev) return;
    if (o.y === "pos") {
      if (!S.freunde) return;
      if (typeof o.x !== "number" || typeof o.z !== "number") return;
      let f = this.friends.get(o.d); if (!f) this.friends.set(o.d, f = {lobbys:new Set()});
      Object.assign(f, {n:String(o.n || "Freund").slice(0, 24), x:o.x, z:o.z, h:+o.h || 0, v:+o.v || 0, c:o.c | 0, g:o.g ? 1 : 0, seen:Date.now()});
      f.lobbys.add(l.code);
      // Lobby-Name vom Ersteller übernehmen (wer beitritt, kennt erst nur den Code)
      if (o.ln && o.own && !l.owner && l.name !== o.ln) { l.name = String(o.ln).slice(0, 30); this.save(); }
      return;
    }
    if (o.y === "bye") { this.friends.delete(o.d); return; }
    if (o.y === "lr" && sub.startsWith("lr/")) { if (Array.isArray(o.roads)) this.ctx.onFriendRoads(o.d, o); return; }
  }
  setPos(p){ this.myPos = p; }
  // eigene Position: 2×/s beim Fahren, sonst alle 5 s ein Lebenszeichen
  pubPos(){
    if (!S.freunde || !this.kanaele.size || !this.myPos || !this.ctx.net.ok) return;
    const now = Date.now(), p = this.myPos, fresh = now - p.t < 3000;
    if (!fresh && now - this.lastPub < 5000) return;
    this.lastPub = now;
    for (const l of this.liste) {
      const k = this.kanaele.get(l.code); if (!k) continue;
      k.send({y:"pos", d:this.ctx.dev, n:this.ctx.name(), x:Math.round(p.x * 10) / 10, z:Math.round(p.z * 10) / 10, h:Math.round(p.h * 1000) / 1000, v:Math.round(p.v * 10) / 10, c:p.c, g:fresh ? 1 : 0, ln:l.owner ? l.name : undefined, own:l.owner ? 1 : undefined});
    }
  }
  // gelernte Straßen mit der Lobby teilen (bleibt beim Vermittler liegen, damit später Kommende sie auch bekommen)
  pubRoads(force){
    if (!S.freunde || !this.kanaele.size || !this.ctx.net.ok) return;
    const d = readJson(this.ctx.learnedFile, null); if (!d || !Array.isArray(d.roads)) return;
    const now = Date.now();
    const roads = d.roads.filter(r => r.p && r.p.length >= 4 && !r.s).map(r => ({t:r.t || null, lv:r.lv || 0, m:r.m ? 1 : 0, ts:r.ts || 0, c:r.c, p:r.p}));
    let cands = (d.cands || []).filter(c => c.p && c.p.length >= 4).sort((a, b) => (b.ts || 0) - (a.ts || 0)).map(c => ({p:c.p, k:c.k}));
    for (const [code, k] of this.kanaele) {
      const sig = this.ctx.learnedVer() + "|" + code;
      if (!force && this.lrSent.get(code) === sig) continue;
      let msg, buf;
      for (let c2 = cands; ; c2 = c2.slice(0, Math.floor(c2.length * 0.75))) {
        msg = {y:"lr", d:this.ctx.dev, n:this.ctx.name(), ts:now, roads, cands:c2};
        buf = seal(k.ch, msg);
        if (buf.length <= LR_MAX || !c2.length) break;
      }
      this.ctx.net.publish(k.topic + "/lr/" + this.ctx.dev, buf, true);
      this.lrSent.set(code, sig);
    }
  }
  stop(){ for (const code of [...this.kanaele.keys()]) this.close(code, true); clearInterval(this.posT); clearInterval(this.cleanT); }

  // ---------------- Verwaltung ----------------
  create(name){
    const code = normCode(newCode(CODE_LEN));
    const l = {code, name:String(name || ("Lobby von " + this.ctx.name())).slice(0, 30), owner:true, an:true, created:new Date().toISOString()};
    this.liste.push(l); this.save(); this.open(l);
    log("Lobby erstellt: " + l.name + " (" + fmtCode(code) + ")");
    return l;
  }
  join(code){
    if (!validCode(code, CODE_LEN)) return {ok:false, why:"Der Code hat 9 Zeichen, z. B. K7M-2QX-9PA"};
    code = normCode(code);
    let l = this.liste.find(x => x.code === code);
    if (!l) { l = {code, name:"Lobby " + fmtCode(code), owner:false, an:true, created:new Date().toISOString()}; this.liste.push(l); }
    l.an = true; this.save(); this.open(l);
    return {ok:true, lobby:l};
  }
  leave(code){
    code = normCode(code); const i = this.liste.findIndex(x => x.code === code); if (i < 0) return false;
    this.close(code, true);
    // eigene gespeicherte Straßen beim Vermittler entfernen
    const k = new Kanal(this.ctx.net, "play", code, () => {}); this.ctx.net.publish(k.topic + "/lr/" + this.ctx.dev, Buffer.alloc(0), true); k.close();
    this.liste.splice(i, 1); this.save(); return true;
  }
  setOn(code, on){ const l = this.liste.find(x => x.code === normCode(code)); if (!l) return false; l.an = !!on; this.save(); if (on) this.open(l); else this.close(l.code, true); return true; }
  rename(code, name){ const l = this.liste.find(x => x.code === normCode(code)); if (!l || !name) return false; l.name = String(name).slice(0, 30); this.save(); return true; }
  list(){
    return this.liste.map(l => ({code:fmtCode(l.code), name:l.name, owner:!!l.owner, an:l.an !== false, created:l.created,
      online:[...this.friends.values()].filter(f => f.lobbys.has(l.code)).map(f => f.n)}));
  }
  friendList(){ const now = Date.now(); return [...this.friends.entries()].map(([id, f]) => ({id, n:f.n, x:f.x, z:f.z, h:f.h, v:f.v, c:f.c, g:f.g, age:now - f.seen})); }
}
module.exports = {Lobbys};
