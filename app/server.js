// ForzaDashboard – Server (Node.js, keine Zusatzpakete)
// Empfängt Forza "Data Out" per UDP, stellt das Dashboard im Browser bereit und verbindet sich mit Freunden.
// Start über starten.js (Verknüpfung "ForzaDashboard starten"), nicht direkt.
"use strict";
const dgram = require("dgram"), http = require("http"), fs = require("fs"), path = require("path"), os = require("os"), zlib = require("zlib"), {execFile, spawn} = require("child_process");
const {P, S, VERSION, CONFIG, DEVICE_ID, setSetting} = require("./lib/basis");
const U = require("./lib/util");
const {log} = U;
U.setLogFile(P.LOG);
log("ForzaDashboard " + VERSION + " startet · Daten: " + P.DATA + " · Gerät " + DEVICE_ID);

const {parse} = require("./lib/telemetrie");
const {mediaState, mediaCmd} = require("./lib/medien");
const {Net} = require("./lib/netz");
const {KartenSpeicher, BASE_HASH} = require("./lib/karte");
const K = require("./lib/kartenpruefung");
const {Sitzungen} = require("./lib/sitzungen");
const {Lobbys} = require("./lib/lobby");
const G = require("./lib/gelernt");
const umzug = require("./lib/umzug");
const updates = require("./lib/updates");
const pub = require("./lib/veroeffentlichen");

// ---------------- Daten aus der alten Version übernehmen (einmalig, nur kopieren) ----------------
try { umzug.run(); } catch (e) { log("Übernahme alter Daten fehlgeschlagen:", e.message); }

// ---------------- Karte ----------------
const karten = new KartenSpeicher().init();
const net = new Net("fd3-" + DEVICE_ID, CONFIG.brokers);
const name = () => S.name;
const sitz = new Sitzungen({net, karten, dev:DEVICE_ID, name});
const lobbys = new Lobbys({net, dev:DEVICE_ID, name, learnedFile:P.LEARNED, learnedVer:() => G.learned.v, onFriendRoads:G.onFriendRoads});

// nur auf diesem PC: Tacho-Zuordnung je Auto; eigene Kartenausrichtung nur, wenn die Karte keine hat
const lokal = {ents:new Map((U.readJson(P.LOKAL, {ents:[]}).ents || []).map(e => [e.id, e]))};
function lokalSave(){ U.writeJson(P.LOKAL, {ents:[...lokal.ents.values()]}); }
function releaseHas(id){ const a = karten.active; return !!(a && a.map.get(id) && a.map.get(id).d !== 1); }
function mapEnts(){
  const s = sitz.cur;
  const base = s && s.ready ? s.view() : karten.active ? karten.active.ents : [];
  if (!lokal.ents.size) return base;
  const m = new Map(base.map(e => [e.id, e]));
  for (const e of lokal.ents.values()) if (e.id.startsWith("cfg:car:") || !m.has(e.id) || m.get(e.id).d === 1) m.set(e.id, e);
  return [...m.values()];
}
function mode(){ const s = sitz.cur; return s ? (s.ready ? "sitzung" : "sitzung-wartet") : "spiel"; }

// ---------------- Live-Daten an Browser (Server-Sent Events) ----------------
const clients = new Set();
function sse(ev, obj){ const m = (ev ? "event: " + ev + "\n" : "") + "data: " + JSON.stringify(obj) + "\n\n"; for (const c of clients) c.write(m); }
setInterval(() => { for (const c of clients) c.write(": ping\n\n"); }, 2000);

sitz.on("change", x => { if (x && x.reload) sse("mapreload", {mode:mode()}); else if (x && x.ents && x.ents.length) sse("map", {ents:x.ents, auth:1}); });
sitz.on("status", () => sse("sess", sessInfo()));
sitz.on("note", t => sse("note", {t}));
sitz.on("presence", o => sse("mp", {id:o.d + "/" + o.c, n:String(o.n || "Freund").slice(0, 24), on:o.on, tool:o.tool, sel:o.sel, x:o.x, z:o.z}));
karten.onChange(a => { sse("kv", karten.info()); if (!sitz.cur) sse("mapreload", {mode:mode()}); else sse("note", {t:"Neue Kartenversion " + a.version + " geladen – deine Sitzung arbeitet weiter auf Version " + sitz.cur.info.base});});

function sessInfo(){
  const s = sitz.cur;
  return {aktiv:s ? s.status() : null, gespeichert:sitz.saved(), karte:karten.info(), herausgeber:pub.capability()};
}

// ---------------- Telemetrie (UDP) ----------------
let latest = null, seq = 0, sentSeq = -1, packets = 0, lastLen = 0, lastPacketT = 0, udp = null, portChanged = false;
const WANTED_UDP = S.udp_port;
let UDP_PORT = S.udp_port, WEB_PORT = S.web_port;
function onPacket(msg){
  if (msg.length < 232) return;
  try {
    const d = parse(msg); latest = JSON.stringify(d); seq++; packets++; lastLen = msg.length; lastPacketT = Date.now();
    if (clients.size) { sentSeq = seq; const m = "data: " + latest + "\n\n"; for (const c of clients) c.write(m); }
    if (d.on && d.dash && (d.px || d.pz)) lobbys.setPos({x:d.px, z:d.pz, h:d.yaw, v:d.spd, c:d.car, t:Date.now()});
  } catch (e) {}
}
function startUdp(port, tries, done){
  const sock = dgram.createSocket({type:"udp4", reuseAddr:false});
  sock.once("error", e => {
    try { sock.close(); } catch (x) {}
    if (tries >= 40) { done(e); return; }
    let next = port + 1; if (next >= 5200 && next <= 5300) next = 5301;   // 5200–5300 nutzt das Spiel selbst
    startUdp(next, tries + 1, done);
  });
  sock.once("listening", () => { sock.on("message", onPacket); sock.on("error", () => {}); udp = sock; UDP_PORT = port; done(null); });
  sock.bind(port, "0.0.0.0");
}
setInterval(() => { if (seq === sentSeq || !latest) return; sentSeq = seq; const m = "data: " + latest + "\n\n"; for (const c of clients) c.write(m); }, 14);

// Freundesliste an die Browser
setInterval(() => {
  const names = lobbys.liste.filter(l => l.an !== false).map(l => l.name);
  sse("fr", {on:S.freunde, ok:net.ok, name:S.name, group:names.join(", "), lobbys:lobbys.list(), list:S.freunde ? lobbys.friendList() : [], lr:G.flr.list.size, upd:updates.state().text});
}, 500);
setInterval(() => lobbys.pubRoads(false), 15000);

// ---------------- Webserver ----------------
function lanIPs(){
  const out = [];
  for (const list of Object.values(os.networkInterfaces())) for (const a of list || []) if (a.family === "IPv4" && !a.internal && !a.address.startsWith("169.254.")) out.push(a.address);
  return out;
}
let htmlCache = null;
function html(){
  const f = path.join(P.PUBLIC, "dashboard.html"), st = fs.statSync(f);
  if (!htmlCache || htmlCache.m !== st.mtimeMs) htmlCache = {m:st.mtimeMs, b:Buffer.from(fs.readFileSync(f, "utf8").replace(/const APP_VER = "[^"]*";/, 'const APP_VER = "' + VERSION + '";'), "utf8")};
  return htmlCache.b;
}
const STATIC = {"/fh6-karte.js":"fh6-karte.js", "/fh6-vorlagen.js":"fh6-vorlagen.js"};
function body(req, max){
  return new Promise((res, rej) => {
    const chunks = []; let n = 0;
    req.on("data", c => { n += c.length; if (n > max) { rej(new Error("zu groß")); req.destroy(); } else chunks.push(c); });
    req.on("end", () => res(Buffer.concat(chunks).toString("utf8"))); req.on("error", rej);
  });
}
const local = req => { const a = req.socket.remoteAddress || ""; return a.includes("127.0.0.1") || a === "::1"; };

const server = http.createServer(async (req, res) => {
  const u = new URL(req.url, "http://x"), q = u.searchParams, p = u.pathname;
  const json = (o, code) => { res.writeHead(code || 200, {"Content-Type":"application/json", "Cache-Control":"no-store"}); res.end(typeof o === "string" ? o : JSON.stringify(o)); };
  const jbody = async max => { try { return JSON.parse(await body(req, max || 64 << 20)); } catch (e) { return null; } };
  try {
    if (p === "/" || p === "/index.html") { res.writeHead(200, {"Content-Type":"text/html; charset=utf-8", "Cache-Control":"no-store"}); return res.end(html()); }
    if (STATIC[p]) {
      const f = path.join(P.PUBLIC, STATIC[p]), st = fs.statSync(f), tag = '"' + st.size + "-" + Math.floor(st.mtimeMs) + '"';
      if (req.headers["if-none-match"] === tag) { res.writeHead(304); return res.end(); }
      const raw = fs.readFileSync(f), gz = /\bgzip\b/.test(req.headers["accept-encoding"] || "");
      res.writeHead(200, Object.assign({"Content-Type":"text/javascript; charset=utf-8", "Cache-Control":"no-cache", "ETag":tag}, gz ? {"Content-Encoding":"gzip"} : {}));
      return res.end(gz ? zlib.gzipSync(raw, {level:4}) : raw);
    }
    if (p === "/events") {
      res.writeHead(200, {"Content-Type":"text/event-stream", "Cache-Control":"no-cache", "Connection":"keep-alive"});
      res.write("retry: 1000\n\n"); if (latest) res.write("data: " + latest + "\n\n");
      res.write("event: sess\ndata: " + JSON.stringify(sessInfo()) + "\n\n");
      clients.add(res); req.on("close", () => clients.delete(res)); return;
    }
    if (p === "/info") return json({udpPort:UDP_PORT, webPort:WEB_PORT, packets, packetLen:lastLen, ips:lanIPs(), markKey:"", ver:VERSION, dev:DEVICE_ID, basis:BASE_HASH, app:"fd3"});
    if (p === "/log") { log("Dashboard:", String(q.get("m") || "").slice(0, 300)); return json({ok:true}); }
    if (p === "/media") return json(mediaState());
    if (p.startsWith("/media/")) { mediaCmd(p.slice(7)); return json({ok:true}); }
    if (p === "/marks") return json([]);
    if (p === "/quit" && local(req)) { json({ok:true}); shutdown(0); return; }

    // ---- Freunde / Lobbys ----
    if (p === "/friends/config") {
      if (q.has("name")) { const n = q.get("name").replace(/[\r\n=]/g, "").trim().slice(0, 24); if (n) setSetting("name", n); }
      if (q.has("on")) { const on = q.get("on") === "1"; if (on !== S.freunde) setSetting("freunde", on); }
      return json({on:S.freunde, name:S.name, group:lobbys.liste.map(l => l.name).join(", ")});
    }
    if (p === "/lobby") return json({liste:lobbys.list(), name:S.name, on:S.freunde, ok:net.ok});
    if (p === "/lobby/neu" && req.method === "POST") { const b = await jbody(4096) || {}; const l = lobbys.create(b.name); return json({ok:true, code:U.fmtCode(l.code), liste:lobbys.list()}); }
    if (p === "/lobby/beitreten" && req.method === "POST") { const b = await jbody(4096) || {}; const r = lobbys.join(b.code); return json(Object.assign(r, {liste:lobbys.list()})); }
    if (p === "/lobby/verlassen" && req.method === "POST") { const b = await jbody(4096) || {}; return json({ok:lobbys.leave(b.code), liste:lobbys.list()}); }
    if (p === "/lobby/an" && req.method === "POST") { const b = await jbody(4096) || {}; return json({ok:lobbys.setOn(b.code, b.an), liste:lobbys.list()}); }
    if (p === "/lobby/name" && req.method === "POST") { const b = await jbody(4096) || {}; return json({ok:lobbys.rename(b.code, b.name), liste:lobbys.list()}); }

    // ---- gelernte Straßen ----
    if (p === "/learned/version") return json({v:G.learned.v});
    if (p === "/learned/lease") return json(G.lease(q.get("id")));
    if (p === "/learned" && req.method === "POST") { const t = await body(req, 64 << 20); const r = G.postLearned(parseInt(q.get("base"), 10), t); if (r.ok) setTimeout(() => lobbys.pubRoads(false), 4000); return json(r); }
    if (p === "/learned") return json(G.getLearned());
    if (p === "/friendlr/version") return json({v:G.flr.v});
    if (p === "/friendlr") return json(G.friendList(S.freunde));
    if (p === "/friendlr/import" && req.method === "POST") { const b = await jbody(); return json(b ? G.importFriend(b) : {ok:false}); }

    // ---- Karte ----
    if (p === "/map" && req.method === "GET") return json({ents:mapEnts(), rev:karten.active ? karten.active.version : 0, me:DEVICE_ID, mode:mode(), version:karten.active ? karten.active.version : 0});
    if (p === "/map" && req.method === "POST") {
      const b = await jbody(); if (!b || !Array.isArray(b.ents)) return json({ok:false});
      const now = Date.now(), out = [], rest = [];
      for (const e of b.ents) {
        if (!e || typeof e.id !== "string") continue;
        if (e.id.startsWith("cfg:car:")) { const x = Object.assign({}, e, {ts:now, a:DEVICE_ID}); lokal.ents.set(e.id, x); out.push(x); }
        else rest.push(e);
      }
      if (out.length) lokalSave();
      if (rest.length) {
        const s = sitz.cur;
        if (s && s.ready) { const sub = s.submit(rest, {grp:b.grp}); for (const e of sub.ents) out.push(Object.assign({}, e, {ts:sub.t0, a:DEVICE_ID})); }
        else if (rest.every(e => e.id === "cfg:mapfit") && !releaseHas("cfg:mapfit")) { for (const e of rest) { const x = Object.assign({}, e, {ts:now, a:DEVICE_ID}); lokal.ents.set(e.id, x); out.push(x); } lokalSave(); }
        else if (rest.every(e => e.id === "cfg:mapfit")) return json({ok:true, ents:[]});   // Ausrichtung kommt aus der Karte
        else return json({ok:false, needSession:true, ents:rest.map(e => mapEnts().find(x => x.id === e.id) || {id:e.id, k:e.k, d:1, ts:0, a:""})});
      }
      return json({ok:true, ents:out});
    }
    if (p === "/map/import" && req.method === "POST") {
      const b = await jbody(); const s = sitz.cur;
      if (!b || !Array.isArray(b.ents)) return json({ok:false, n:0});
      if (!s || !s.ready) return json({ok:false, needSession:true, n:0});
      // nur Neueres übernehmen – eine alte Datei darf nichts überschreiben
      const cur = new Map(s.view().map(e => [e.id, e])), add = b.ents.filter(e => e && e.id && !K.checkEnt(e) && !e.id.startsWith("cfg:car:") && (!cur.has(e.id) || K.newer(e, cur.get(e.id))));
      if (add.length) s.submit(add, {});
      return json({ok:true, n:add.length});
    }
    if (p === "/map/presence" && req.method === "POST") {
      const o = await jbody(4096); if (!o) return json({ok:false});
      const msg = {y:"pr", c:String(o.c || "").slice(0, 20), n:S.name, on:o.on ? 1 : 0, tool:o.tool, sel:o.sel || null, x:o.x, z:o.z};
      if (sitz.cur) sitz.cur.send(msg);
      sse("mp", {id:DEVICE_ID + "/" + msg.c, n:S.name, on:msg.on, tool:msg.tool, sel:msg.sel, x:msg.x, z:msg.z});   // andere Fenster auf diesem PC
      return json({ok:true});
    }
    if (p === "/karte/info") return json(Object.assign(karten.info(), {updates:updates.state()}));
    if (p === "/karte/version" && req.method === "POST") { const b = await jbody(4096) || {}; const err = karten.activate(b.v | 0); return json({ok:!err, why:err, info:karten.info()}); }
    if (p === "/karte/export") {
      const ents = mapEnts(), name = "karte-" + (sitz.cur ? "sitzung-" + sitz.cur.info.code : "v" + (karten.active ? karten.active.version : 0)) + ".json";
      res.writeHead(200, {"Content-Type":"application/json", "Content-Disposition":"attachment; filename=" + name});
      return res.end(K.serialize({}, ents));
    }

    // ---- Bearbeiten-Sitzung ----
    if (p === "/sitzung") return json(sessInfo());
    if (p === "/sitzung/neu" && req.method === "POST") { const b = await jbody(4096) || {}; return json(Object.assign(sitz.create(b.title), sessInfo())); }
    if (p === "/sitzung/beitreten" && req.method === "POST") { const b = await jbody(4096) || {}; return json(Object.assign(sitz.join(b.code), sessInfo())); }
    if (p === "/sitzung/verlassen" && req.method === "POST") { sitz.close(true); return json(Object.assign({ok:true}, sessInfo())); }
    if (p === "/sitzung/umstellen" && req.method === "POST") {
      const s = sitz.cur; if (!s || s.role !== "host") return json({ok:false, why:"Nur der Host kann die Sitzung umstellen"});
      const v = karten.active && karten.active.version; if (!v || v === s.info.base) return json({ok:false, why:"Die Sitzung ist schon auf der neuesten Version"});
      s.rebase(v); s.hb(); return json(Object.assign({ok:true}, sessInfo()));
    }
    if (p === "/sitzung/verlauf") { const s = sitz.cur; return json(s ? {ok:true, me:DEVICE_ID, list:s.history()} : {ok:false, list:[]}); }
    if (p === "/sitzung/schritt") { const s = sitz.cur; const d = s && s.detail(q.get("g")); return json(d ? Object.assign({ok:true}, d) : {ok:false}); }
    if (p === "/sitzung/rueckgaengig" && req.method === "POST") { const b = await jbody(4096) || {}; const s = sitz.cur; return json(s ? s.undo(b.g, !!b.force) : {ok:false, why:"Keine Sitzung"}); }
    if (p === "/sitzung/strgz" && req.method === "POST") {
      const b = await jbody(4096) || {}; const s = sitz.cur; if (!s) return json({ok:false, why:"Keine Sitzung"});
      const g = s.lastOwn(!!b.redo); if (!g) return json({ok:false, why:b.redo ? "Nichts zum Wiederholen" : "Nichts zum Rückgängigmachen"});
      return json(Object.assign(s.undo(g, !!b.force), {g}));
    }
    if (p === "/sitzung/veroeffentlichen" && req.method === "POST") {
      const b = await jbody(8192) || {}; const s = sitz.cur;
      if (!s || !s.ready) return json({ok:false, why:"Keine Sitzung aktiv"});
      if (b.vorschau) return json(pub.preview(s, karten));
      const r = await pub.publish(s, karten, {notiz:b.notiz, name:S.name, massenloeschung:!!b.massenloeschung});
      if (r.ok) s.send({y:"pub", v:r.version, n:S.name});
      return json(r);
    }

    // ---- Updates / Umzug ----
    if (p === "/updates") return json(updates.state());
    if (p === "/updates/pruefen" && req.method === "POST") { updates.check(true); return json({ok:true}); }
    if (p === "/umzug/speicher") return json(umzug.oldStorage());

    res.writeHead(404); res.end("Nicht gefunden");
  } catch (e) {
    log("Fehler bei " + p + ":", e.stack || e.message);
    try { json({ok:false, why:"Interner Fehler: " + e.message}, 500); } catch (x) {}
  }
});

// ---------------- Browser im Vollbild öffnen ----------------
function openDashboard(){
  const url = "http://localhost:" + WEB_PORT + "/";
  if (process.platform !== "win32") return;
  const pf = [process.env["ProgramFiles(x86)"], process.env.ProgramFiles, process.env.LOCALAPPDATA].filter(Boolean);
  const exes = [];
  for (const b of pf) exes.push(path.join(b, "Microsoft", "Edge", "Application", "msedge.exe"), path.join(b, "Google", "Chrome", "Application", "chrome.exe"));
  const prof = path.join(P.LOCAL_ROOT, "browser");
  for (const e of exes) if (fs.existsSync(e)) {
    spawn(e, ["--app=" + url, "--user-data-dir=" + prof, "--no-first-run", "--no-default-browser-check", "--hide-crash-restore-bubble", "--disable-session-crashed-bubble", "--start-fullscreen"], {detached:true, stdio:"ignore"}).unref();
    return;
  }
  spawn("cmd", ["/c", "start", "", url], {detached:true, stdio:"ignore", windowsHide:true}).unref();
}

let stopping = false;
function shutdown(code){
  if (stopping) return; stopping = true;
  try { if (sitz.cur) sitz.cur.stop(); } catch (e) {}
  try { lobbys.stop(); } catch (e) {}
  setTimeout(() => { try { net.stop(); } catch (e) {} process.exit(code); }, 400);
}
process.on("SIGINT", () => shutdown(0));
process.on("SIGTERM", () => shutdown(0));
process.on("uncaughtException", e => { log("Unerwarteter Fehler:", e.stack || e.message); });
updates.onInstallReady(() => {
  // Programm-Update liegt bereit: erst neu starten, wenn gerade nicht gefahren wird
  const t = setInterval(() => { if (Date.now() - lastPacketT > 60000) { clearInterval(t); log("Update wird installiert – Neustart"); sse("upd", {ver:updates.state().ready}); setTimeout(() => shutdown(42), 1500); } }, 5000);
});

let tries = 0;
server.on("error", e => {
  if (e.code === "EADDRINUSE" && tries < 10) { tries++; WEB_PORT++; server.listen(WEB_PORT, "0.0.0.0"); return; }
  log("FEHLER: Kein freier Port für das Dashboard gefunden (" + e.message + ")");
});
function banner(){
  console.log("");
  console.log("  ForzaDashboard läuft   (Version " + VERSION + ")");
  console.log("  ────────────────────────────────────────────");
  if (portChanged) console.log("  !! Port " + WANTED_UDP + " war belegt. Stattdessen Port " + UDP_PORT + " – diesen im Spiel eintragen.\n");
  console.log("  Im Spiel eintragen:  IP 127.0.0.1   Port " + UDP_PORT);
  console.log("  Dashboard am PC:     http://localhost:" + WEB_PORT);
  for (const ip of lanIPs()) console.log("  Handy / Tablet:      http://" + ip + ":" + WEB_PORT);
  console.log("  Karte:               Version " + (karten.active ? karten.active.version : "–") + (sitz.cur ? "  ·  Bearbeiten-Sitzung " + U.fmtCode(sitz.cur.info.code) : ""));
  console.log("  Dieses Fenster offen lassen. Beenden: Fenster schließen.\n");
  if (S.beim_start_oeffnen && !process.env.FD_RESTARTED && !process.env.FD_NO_BROWSER) setTimeout(() => { if (!clients.size) openDashboard(); }, 2500);
}

// läuft schon ein ForzaDashboard (auch die alte exe)? -> neues Fenster nur öffnen bzw. alte Version beenden
function probe(cb){
  const r = http.get({host:"127.0.0.1", port:WEB_PORT, path:"/info", timeout:900}, res => {
    let d = ""; res.on("data", c => d += c); res.on("end", () => { try { const j = JSON.parse(d); cb(typeof j.udpPort === "number" ? j : null); } catch (e) { cb(null); } });
  });
  r.on("timeout", () => { r.destroy(); cb(null); }); r.on("error", () => cb(null));
}
function start(info, retried){
  if (info && info.app === "fd3" && info.ver === VERSION) { console.log("\n  ForzaDashboard läuft bereits – das Dashboard wird geöffnet.\n"); openDashboard(); setTimeout(() => process.exit(0), 3000); return; }
  if (info && !retried) {
    log("Eine andere ForzaDashboard-Version (" + info.ver + ") läuft noch und wird beendet");
    http.get({host:"127.0.0.1", port:WEB_PORT, path:"/quit", timeout:800}, () => {}).on("error", () => {});
    const next = () => setTimeout(() => probe(i => start(i, true)), 2000);
    if (process.platform === "win32") execFile("taskkill", ["/f", "/im", "ForzaDashboard.exe"], {windowsHide:true}, next); else next();
    return;
  }
  if (info) { log("FEHLER: Eine ältere ForzaDashboard-Version läuft noch auf Port " + WEB_PORT + ". Bitte alle alten ForzaDashboard-Fenster schließen."); return; }
  startUdp(UDP_PORT, 0, err => {
    if (err) { log("FEHLER: Kein UDP-Port frei (" + err.code + "). In daten/einstellungen.txt bei udp_port eine andere Zahl eintragen."); return; }
    if (UDP_PORT !== WANTED_UDP) { portChanged = true; setSetting("udp_port", UDP_PORT); }
    server.listen(WEB_PORT, "0.0.0.0", banner);
    net.start(); lobbys.start(); sitz.resume();
    updates.start({karten});
    umzug.ensureShortcut();
  });
}
probe(i => start(i, false));
setInterval(() => { process.title = "ForzaDashboard – " + packets + " Pakete" + (packets ? "" : " (noch keine Daten von Forza)"); }, 1000);
