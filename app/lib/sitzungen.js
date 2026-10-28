// Verwaltung der Bearbeiten-Sitzungen auf diesem PC: erstellen, beitreten, verlassen, nach Neustart fortsetzen.
"use strict";
const fs = require("fs"), path = require("path"), EventEmitter = require("events");
const {P} = require("./basis");
const {readJson, writeJson, log, newCode, normCode, validCode, fmtCode} = require("./util");
const {Sitzung} = require("./sitzung");

const CODE_LEN = 9;
const AKTIV = () => path.join(P.SITZUNGEN, "aktiv.json");

class Sitzungen extends EventEmitter {
  constructor(ctx){ super(); this.ctx = ctx; this.cur = null; fs.mkdirSync(P.SITZUNGEN, {recursive:true}); }
  // nach einem Neustart automatisch wieder in die letzte Sitzung
  resume(){
    const a = readJson(AKTIV(), null);
    if (a && a.code) { const info = readJson(path.join(P.SITZUNGEN, a.code, "sitzung.json"), null); if (info) this.open(info, false); }
  }
  open(info, asHost){
    if (this.cur) this.close(false);
    const s = new Sitzung(info, this.ctx);
    s.saveInfo();
    for (const ev of ["change", "status", "presence", "note"]) s.on(ev, x => this.emit(ev, x));
    this.cur = s; s.start(asHost);
    writeJson(AKTIV(), {code:info.code}, true);
    log("Bearbeiten-Sitzung " + fmtCode(info.code) + (asHost ? " erstellt" : " beigetreten"));
    this.emit("change", {reload:true}); this.emit("status");
    return s;
  }
  create(title){
    const base = this.ctx.karten.active && this.ctx.karten.active.version;
    if (!base) return {ok:false, why:"Keine gültige Kartenversion vorhanden"};
    const code = normCode(newCode(CODE_LEN));
    const s = this.open({code, title:String(title || "").slice(0, 40) || "Sitzung von " + this.ctx.name(), created:new Date().toISOString(), base, term:1, owner:this.ctx.dev}, true);
    return {ok:true, code:fmtCode(s.info.code)};
  }
  join(code){
    if (!validCode(code, CODE_LEN)) return {ok:false, why:"Der Code hat 9 Zeichen, z. B. K7M-2QX-9PA"};
    code = normCode(code);
    if (this.cur && this.cur.info.code === code) return {ok:true, code:fmtCode(code)};
    // schon einmal dabei gewesen? Dann mit dem gespeicherten Journal weitermachen
    const info = readJson(path.join(P.SITZUNGEN, code, "sitzung.json"), null) || {code, title:"", created:new Date().toISOString(), base:0, term:0, owner:""};
    this.open(info, false);
    return {ok:true, code:fmtCode(code)};
  }
  close(clearActive){
    if (!this.cur) return;
    const c = this.cur; this.cur = null; c.stop(); c.removeAllListeners();
    if (clearActive !== false) { try { fs.unlinkSync(AKTIV()); } catch (e) {} log("Bearbeiten-Sitzung " + fmtCode(c.info.code) + " verlassen"); }
    this.emit("change", {reload:true}); this.emit("status");
  }
  // Sitzungen, in denen dieser PC schon war (zum schnellen Wieder-Beitreten)
  saved(){
    const out = [];
    try {
      for (const d of fs.readdirSync(P.SITZUNGEN)) {
        const info = readJson(path.join(P.SITZUNGEN, d, "sitzung.json"), null); if (!info || !info.code) continue;
        let n = 0; try { n = fs.readFileSync(path.join(P.SITZUNGEN, d, "ops.jsonl"), "utf8").split("\n").filter(Boolean).length; } catch (e) {}
        out.push({code:fmtCode(info.code), title:info.title || "", created:info.created, base:info.base, schritte:n, mine:info.owner === this.ctx.dev});
      }
    } catch (e) {}
    return out.sort((a, b) => String(b.created).localeCompare(String(a.created))).slice(0, 12);
  }
}
module.exports = {Sitzungen, CODE_LEN};
