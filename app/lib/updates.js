// Updates über GitHub Releases – für die Karte und für das Programm, getrennt voneinander.
//   karte-v12  -> karte.manifest.json + karte.json.gz      (neue Kartenversion: wird sofort übernommen)
//   app-v3.1.0 -> app.manifest.json  + app-bundle.json.gz  (neues Programm: wird installiert, wenn gerade nicht gefahren wird)
// Alles ist mit dem Herausgeber-Schlüssel signiert. Nur was die Prüfung besteht, wird benutzt.
"use strict";
const https = require("https"), http = require("http"), fs = require("fs"), path = require("path"), zlib = require("zlib");
const {P, S, VERSION, CONFIG, updateRepo} = require("./basis");
const {sha256, writeAtomic, writeJson, readJson, log} = require("./util");
const SIG = require("./signatur");

const EVERY = 20 * 60000;
const API = process.env.FD_UPDATE_API || "https://api.github.com";   // für Tests umstellbar
const st = {text:"", last:0, error:"", karte:{neueste:0}, app:{neueste:"", ready:""}, etag:"", releases:null, busy:false};
let ctx = null, readyCb = null;

function verCmp(a, b){ const A = String(a).split(".").map(Number), B = String(b).split(".").map(Number); for (let i = 0; i < 3; i++) if ((A[i] || 0) !== (B[i] || 0)) return (A[i] || 0) - (B[i] || 0); return 0; }

function get(url, opts, redirects){
  opts = opts || {}; redirects = redirects || 0;
  return new Promise((resolve, reject) => {
    const headers = Object.assign({"User-Agent":"ForzaDashboard/" + VERSION, "Accept":opts.accept || "application/octet-stream"}, opts.headers || {});
    const req = (url.startsWith("http:") ? http : https).get(url, {headers, timeout:30000}, res => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode) && res.headers.location && redirects < 5) { res.resume(); return resolve(get(new URL(res.headers.location, url).toString(), opts, redirects + 1)); }
      const chunks = []; let n = 0;
      res.on("data", c => { n += c.length; if (n > (opts.max || 64 << 20)) { req.destroy(new Error("Download zu groß")); } else chunks.push(c); });
      res.on("end", () => resolve({status:res.statusCode, headers:res.headers, body:Buffer.concat(chunks)}));
      res.on("error", reject);
    });
    req.on("timeout", () => req.destroy(new Error("Zeitüberschreitung")));
    req.on("error", reject);
  });
}
async function releases(){
  const repo = updateRepo(); if (!repo) throw new Error("Kein Update-Repo eingestellt");
  const r = await get(API + "/repos/" + repo + "/releases?per_page=40", {accept:"application/vnd.github+json", headers:st.etag ? {"If-None-Match":st.etag} : {}, max:4 << 20});
  if (r.status === 304 && st.releases) return st.releases;
  if (r.status === 404) throw new Error("Repo " + repo + " nicht gefunden (oder privat)");
  if (r.status === 403) throw new Error("GitHub-Limit erreicht – später erneut");
  if (r.status !== 200) throw new Error("GitHub antwortet " + r.status);
  st.etag = r.headers.etag || ""; st.releases = JSON.parse(r.body.toString("utf8"));
  return st.releases;
}
const asset = (rel, name) => (rel.assets || []).find(a => a.name === name);
function newest(list, re){
  let best = null;
  for (const rel of list) {
    if (rel.draft || rel.prerelease) continue;
    const m = re.exec(rel.tag_name || ""); if (!m) continue;
    if (!best || (re.source.includes("karte") ? +m[1] > +best.v : verCmp(m[1], best.v) > 0)) best = {v:re.source.includes("karte") ? +m[1] : m[1], rel};
  }
  return best;
}

async function checkMap(list){
  const nb = newest(list, /^karte-v(\d+)$/);
  if (!nb) return;
  st.karte.neueste = nb.v;
  const cur = ctx.karten.active ? ctx.karten.active.version : 0;
  if (nb.v <= cur || ctx.karten.has(nb.v)) return;
  const ma = asset(nb.rel, "karte.manifest.json"), da = asset(nb.rel, "karte.json.gz");
  if (!ma || !da) throw new Error("Release " + nb.rel.tag_name + " unvollständig");
  const man = JSON.parse((await get(ma.browser_download_url, {max:1 << 20})).body.toString("utf8"));
  if (!SIG.verify(man, CONFIG.signaturSchluessel)) throw new Error("Karte v" + nb.v + ": Signatur ungültig – wird NICHT übernommen");
  const gz = (await get(da.browser_download_url, {max:32 << 20})).body;
  const buf = zlib.gunzipSync(gz, {maxOutputLength:256 << 20});
  const err = ctx.karten.install(man, buf, {source:"GitHub"});
  if (err) throw new Error("Karte v" + nb.v + ": " + err);
}

async function checkApp(list){
  const nb = newest(list, /^app-v(\d+\.\d+\.\d+)$/);
  if (!nb) return;
  st.app.neueste = nb.v;
  if (verCmp(nb.v, VERSION) <= 0 || !S.updates) return;
  if (st.app.ready === nb.v) return;
  const ma = asset(nb.rel, "app.manifest.json"), ba = asset(nb.rel, "app-bundle.json.gz");
  if (!ma || !ba) throw new Error("Release " + nb.rel.tag_name + " unvollständig");
  const man = JSON.parse((await get(ma.browser_download_url, {max:1 << 20})).body.toString("utf8"));
  if (man.typ !== "app" || man.version !== nb.v || !SIG.verify(man, CONFIG.signaturSchluessel)) throw new Error("Programm " + nb.v + ": Signatur ungültig – wird NICHT installiert");
  const gz = (await get(ba.browser_download_url, {max:64 << 20})).body;
  if (sha256(gz) !== man.sha256) throw new Error("Programm " + nb.v + ": Prüfsumme stimmt nicht");
  const bundle = JSON.parse(zlib.gunzipSync(gz, {maxOutputLength:256 << 20}).toString("utf8"));
  const dir = path.join(P.UPDATES, "app-" + nb.v);
  fs.rmSync(dir, {recursive:true, force:true});
  let n = 0;
  for (const [rel, b64] of Object.entries(bundle.files || {})) {
    // nur erlaubte Ziele (Programmordner + Start-/Hilfedateien), keine Pfade nach draußen
    if (!/^(app\/[\w.\-\/]+|starten\.js|LIESMICH\.txt|ForzaDashboard-starten\.bat|GamePass-Fix\.bat)$/.test(rel) || rel.includes("..")) throw new Error("Programm " + nb.v + ": unerlaubter Dateipfad " + rel);
    const f = path.join(dir, rel); fs.mkdirSync(path.dirname(f), {recursive:true}); fs.writeFileSync(f, Buffer.from(b64, "base64")); n++;
  }
  if (!fs.existsSync(path.join(dir, "app", "server.js")) || !fs.existsSync(path.join(dir, "app", "package.json"))) throw new Error("Programm " + nb.v + ": unvollständig");
  writeJson(path.join(P.UPDATES, "bereit.json"), {version:nb.v, dir, dateien:n, geladen:new Date().toISOString()}, true);
  st.app.ready = nb.v;
  log("Programm-Update " + nb.v + " geladen und geprüft (" + n + " Dateien) – wird beim nächsten Halt installiert");
  if (readyCb) readyCb();
}

async function check(force){
  if (st.busy || (!force && Date.now() - st.last < EVERY - 5000)) return;
  st.busy = true; st.last = Date.now();
  try {
    const list = await releases();
    await checkMap(list);
    await checkApp(list);
    st.error = "";
  } catch (e) { st.error = e.message; if (!/Kein Update-Repo|nicht gefunden/.test(e.message)) log("Update-Prüfung:", e.message); }
  finally { st.busy = false; }
}
function state(){
  const kv = ctx && ctx.karten.active ? ctx.karten.active.version : 0;
  let text = st.app.ready ? "Update " + st.app.ready + " bereit – wird installiert, sobald du anhältst" : st.error ? st.error : st.last ? "Aktuell" : "Noch nicht geprüft";
  if (!st.app.ready && !st.error && st.karte.neueste > kv) text = "Karte v" + st.karte.neueste + " wird geladen …";
  return {text, error:st.error, last:st.last, repo:updateRepo(), programm:{lokal:VERSION, neueste:st.app.neueste, ready:st.app.ready, an:S.updates}, karte:{lokal:kv, neueste:st.karte.neueste}, ready:st.app.ready};
}
function start(c){
  ctx = c;
  const r = readJson(path.join(P.UPDATES, "bereit.json"), null);
  if (r && verCmp(r.version, VERSION) > 0) st.app.ready = r.version;
  setTimeout(() => check(true), 15000).unref();
  setInterval(() => check(false), EVERY).unref();
}
function onInstallReady(cb){ readyCb = cb; if (st.app.ready) setTimeout(cb, 1000); }
module.exports = {start, check, state, onInstallReady, verCmp};
