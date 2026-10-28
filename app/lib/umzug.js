// Übernahme aus der alten Version (einmalig, es wird nur KOPIERT – die alten Dateien bleiben unverändert liegen):
// - gelernte Straßen, Straßen der Freunde, Einstellungen (ohne die alte, kopierte Kennung)
// - Dashboard-Einstellungen aus dem Speicher der alten ForzaDashboard.exe (Kacheln, Tank, Straßenkorrekturen …)
// Außerdem: Verknüpfung "ForzaDashboard starten" anlegen (funktioniert auch, wenn Smart App Control .bat blockiert).
"use strict";
const fs = require("fs"), path = require("path");
const {P, S, setSetting} = require("./basis");
const {readJson, writeJson, log} = require("./util");

const FLAG = () => path.join(P.DATA, "umzug.json");

function run(){
  if (fs.existsSync(FLAG())) return;
  const done = [];
  const copy = (from, to) => { const f = path.join(P.ROOT, from); if (fs.existsSync(f) && !fs.existsSync(to)) { fs.copyFileSync(f, to); done.push(from); } };
  copy("gelernte-strassen.json", P.LEARNED);
  copy("gelernte-strassen.version", P.LEARNED_VER);
  copy("freunde-strassen.json", P.FRIENDS_LR);
  const old = path.join(P.ROOT, "einstellungen.txt");
  if (fs.existsSync(old)) {
    for (const raw of fs.readFileSync(old, "utf8").split(/\r?\n/)) {
      const m = /^\s*([a-z_]+)\s*=\s*(.*?)\s*$/i.exec(raw); if (!m) continue;
      const k = m[1].toLowerCase();
      if (["udp_port", "web_port"].includes(k) && +m[2] > 0) setSetting(k, +m[2]);
      else if (["beim_start_oeffnen", "freunde", "spotify_lautstaerke"].includes(k)) setSetting(k, !["nein", "0", "false", "aus"].includes(m[2].toLowerCase()));
      else if (k === "name" && m[2]) setSetting("name", m[2].slice(0, 24));
    }
    done.push("einstellungen.txt (ohne alte Kennung)");
  }
  writeJson(FLAG(), {am:new Date().toISOString(), uebernommen:done}, true);
  if (done.length) log("Aus der alten Version übernommen: " + done.join(", "));
}

// ---------------- Speicher der alten exe lesen (LevelDB von WebView2/Chromium) ----------------
function varint(b, o){ let r = 0, s = 0, x; do { x = b[o.i++]; r += (x & 127) * 2 ** s; s += 7; } while (x & 128 && s < 63); return r; }
function snappy(b){
  const o = {i:0}, n = varint(b, o), out = Buffer.alloc(n); let w = 0;
  while (o.i < b.length) {
    const tag = b[o.i++], t = tag & 3;
    if (t === 0) {
      let len = tag >> 2;
      if (len >= 60) { const nb = len - 59; len = 0; for (let k = 0; k < nb; k++) len |= b[o.i++] << (8 * k); }
      len += 1; b.copy(out, w, o.i, o.i + len); o.i += len; w += len;
    } else {
      let len, off;
      if (t === 1) { len = ((tag >> 2) & 7) + 4; off = ((tag >> 5) << 8) | b[o.i++]; }
      else if (t === 2) { len = (tag >> 2) + 1; off = b.readUInt16LE(o.i); o.i += 2; }
      else { len = (tag >> 2) + 1; off = b.readUInt32LE(o.i); o.i += 4; }
      for (let k = 0; k < len; k++, w++) out[w] = out[w - off];
    }
  }
  return out;
}
function blockEntries(blk, cb){
  const nRest = blk.readUInt32LE(blk.length - 4), end = blk.length - 4 - nRest * 4, o = {i:0};
  let last = Buffer.alloc(0);
  while (o.i < end) {
    const shared = varint(blk, o), nonShared = varint(blk, o), vlen = varint(blk, o);
    const key = Buffer.concat([last.subarray(0, shared), blk.subarray(o.i, o.i + nonShared)]); o.i += nonShared;
    const val = blk.subarray(o.i, o.i + vlen); o.i += vlen; last = key; cb(key, val);
  }
}
function readBlock(file, off, size){
  const raw = file.subarray(off, off + size), type = file[off + size];
  return type === 1 ? snappy(raw) : raw;
}
function readTable(buf, put){
  const foot = buf.subarray(buf.length - 48), o = {i:0};
  varint(foot, o); varint(foot, o);   // Metaindex
  const io = varint(foot, o), is = varint(foot, o);
  blockEntries(readBlock(buf, io, is), (k, v) => {
    const h = {i:0}, off = varint(v, h), size = varint(v, h);
    blockEntries(readBlock(buf, off, size), (ik, val) => {
      const uk = ik.subarray(0, ik.length - 8), tail = ik.readBigUInt64LE(ik.length - 8);
      put(uk, Number(tail >> 8n), (tail & 0xffn) === 1n ? val : null);
    });
  });
}
function readLog(buf, put){
  const recs = []; let cur = null;
  for (let blk = 0; blk < buf.length; blk += 32768) {
    let i = blk;
    while (i + 7 <= Math.min(buf.length, blk + 32768)) {
      const len = buf.readUInt16LE(i + 4), type = buf[i + 6];
      if (type === 0 && len === 0) break;
      const data = buf.subarray(i + 7, i + 7 + len); i += 7 + len;
      if (type === 1) recs.push(data); else if (type === 2) cur = [data]; else if (type === 3 && cur) cur.push(data); else if (type === 4 && cur) { cur.push(data); recs.push(Buffer.concat(cur)); cur = null; }
    }
  }
  for (const r of recs) {
    if (r.length < 12) continue;
    let seq = Number(r.readBigUInt64LE(0)); const count = r.readUInt32LE(8), o = {i:12};
    for (let c = 0; c < count && o.i < r.length; c++, seq++) {
      const tag = r[o.i++], kl = varint(r, o), key = r.subarray(o.i, o.i + kl); o.i += kl;
      if (tag === 1) { const vl = varint(r, o), val = r.subarray(o.i, o.i + vl); o.i += vl; put(key, seq, val); } else put(key, seq, null);
    }
  }
}
function decodeStr(b){ return !b || !b.length ? "" : b[0] === 1 ? b.subarray(1).toString("latin1") : b.subarray(1).toString("utf16le"); }
function readLocalStorage(dir){
  const best = new Map();
  const put = (k, seq, v) => { const s = k.toString("latin1"); const c = best.get(s); if (!c || seq >= c.seq) best.set(s, {seq, k, v}); };
  for (const f of fs.readdirSync(dir)) {
    try {
      const b = fs.readFileSync(path.join(dir, f));
      if (f.endsWith(".ldb") || f.endsWith(".sst")) readTable(b, put); else if (f.endsWith(".log")) readLog(b, put);
    } catch (e) {}
  }
  const out = {};
  for (const {k, v} of best.values()) {
    const s = k.toString("latin1");
    if (!/^_http:\/\/(localhost|127\.0\.0\.1):\d+\x00/.test(s) || !v) continue;
    const kb = k.subarray(s.indexOf("\x00") + 1), key = decodeStr(kb);
    if (key.startsWith("fd.")) out[key] = decodeStr(v);
  }
  return out;
}
function oldStorage(){
  const dir = path.join(P.LOCAL_ROOT, "webview", "EBWebView", "Default", "Local Storage", "leveldb");
  try {
    if (!fs.existsSync(dir)) return {ok:false, werte:{}};
    const werte = readLocalStorage(dir); delete werte["fd.learned"];
    return {ok:true, quelle:"ForzaDashboard.exe", werte};
  } catch (e) { log("Alter Dashboard-Speicher nicht lesbar:", e.message); return {ok:false, werte:{}}; }
}

// ---------------- Verknüpfung (ohne Skript – Smart App Control blockiert manchmal .bat) ----------------
function makeLnk(o){
  const u16 = s => { const b = Buffer.alloc(2 + s.length * 2); b.writeUInt16LE(s.length, 0); b.write(s, 2, "utf16le"); return b; };
  const flags = 0x02 | 0x04 | 0x10 | 0x20 | 0x80 | (o.icon ? 0x40 : 0);
  const hdr = Buffer.alloc(76);
  hdr.writeUInt32LE(0x4C, 0); Buffer.from("0114020000000000c000000000000046", "hex").copy(hdr, 4);
  hdr.writeUInt32LE(flags, 20); hdr.writeUInt32LE(0x20, 24); hdr.writeUInt32LE(1, 60);
  const vol = Buffer.alloc(17); vol.writeUInt32LE(17, 0); vol.writeUInt32LE(3, 4); vol.writeUInt32LE(0, 8); vol.writeUInt32LE(0x10, 12);
  const base = Buffer.from(o.target + "\0", "latin1"), suffix = Buffer.from("\0");
  const head = 0x1C, volOff = head, baseOff = volOff + vol.length, sufOff = baseOff + base.length, size = sufOff + suffix.length;
  const li = Buffer.alloc(head);
  li.writeUInt32LE(size, 0); li.writeUInt32LE(head, 4); li.writeUInt32LE(1, 8); li.writeUInt32LE(volOff, 12); li.writeUInt32LE(baseOff, 16); li.writeUInt32LE(0, 20); li.writeUInt32LE(sufOff, 24);
  const parts = [hdr, li, vol, base, suffix, u16(o.name || ""), u16(o.workdir || ""), u16(o.args || "")];
  if (o.icon) parts.push(u16(o.icon));
  parts.push(Buffer.alloc(4));
  return Buffer.concat(parts);
}
function ensureShortcut(){
  if (process.platform !== "win32") return;
  try {
    const cmd = path.join(process.env.SystemRoot || "C:\\Windows", "System32", "cmd.exe");
    const buf = makeLnk({target:cmd, args:'/c title ForzaDashboard & "' + process.execPath + '" "' + path.join(P.ROOT, "starten.js") + '"', workdir:P.ROOT, name:"ForzaDashboard starten", icon:process.execPath});
    const f = path.join(P.ROOT, "ForzaDashboard starten.lnk");
    if (!fs.existsSync(f) || !fs.readFileSync(f).equals(buf)) fs.writeFileSync(f, buf);
  } catch (e) {}
}

module.exports = {run, oldStorage, readLocalStorage, ensureShortcut, snappy};
