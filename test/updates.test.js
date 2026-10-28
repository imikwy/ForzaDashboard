"use strict";
const {PEM, DATA, until} = require("./helfer");
const test = require("node:test"), assert = require("node:assert");
const http = require("http"), fs = require("fs"), path = require("path"), zlib = require("zlib"), crypto = require("crypto");
const K = require("../app/lib/kartenpruefung");
const SIG = require("../app/lib/signatur");
const basis = require("../app/lib/basis");
const {KartenSpeicher, buildManifest} = require("../app/lib/karte");

// Nachgebautes GitHub: /repos/o/r/releases + Dateien
const files = new Map(); let releases = [];
const srv = http.createServer((req, res) => {
  if (req.url.startsWith("/repos/o/r/releases")) { res.writeHead(200, {"Content-Type":"application/json"}); return res.end(JSON.stringify(releases)); }
  const f = files.get(req.url); if (!f) { res.writeHead(404); return res.end(); }
  res.writeHead(200); res.end(f);
});
let base = "";
function rel(tag, assets){
  releases.push({tag_name:tag, draft:false, prerelease:false, assets:Object.entries(assets).map(([name, buf]) => { const u = "/dl/" + tag + "/" + name; files.set(u, buf); return {name, browser_download_url:base + u}; })});
}

test("Updates von GitHub: Karte wird übernommen, Programm bereitgelegt, Fälschung abgelehnt", async () => {
  await new Promise(r => srv.listen(0, "127.0.0.1", r));
  base = "http://127.0.0.1:" + srv.address().port;
  process.env.FD_UPDATE_API = base; basis.CONFIG.repo = "o/r";
  const updates = require("../app/lib/updates");
  const karten = new KartenSpeicher(); karten.bundled = () => null; karten.init();

  // gültige Karte v3
  const ents = [{id:"a", k:"r", t:"road", lv:0, sm:1, p:[0, 0, 5, 5], ts:1, a:"x"}];
  const buf = Buffer.from(K.serialize({version:3}, ents), "utf8");
  rel("karte-v3", {"karte.manifest.json":Buffer.from(JSON.stringify(SIG.sign(buildManifest(3, buf, ents), PEM))), "karte.json.gz":zlib.gzipSync(buf)});
  // gefälschtes Programm (mit falschem Schlüssel signiert)
  const fake = crypto.generateKeyPairSync("ed25519").privateKey.export({type:"pkcs8", format:"pem"});
  const bundle = zlib.gzipSync(Buffer.from(JSON.stringify({version:"9.9.9", files:{"app/server.js":Buffer.from("böse").toString("base64"), "app/package.json":Buffer.from('{"version":"9.9.9"}').toString("base64")}})));
  const sha = crypto.createHash("sha256").update(bundle).digest("hex");
  rel("app-v9.9.9", {"app.manifest.json":Buffer.from(JSON.stringify(SIG.sign({typ:"app", version:"9.9.9", sha256:sha}, fake))), "app-bundle.json.gz":bundle});

  updates.start({karten});
  await updates.check(true);
  assert.strictEqual(karten.active.version, 3, "Karte v3 übernommen");
  assert.match(updates.state().error, /Signatur ungültig/, "gefälschtes Programm abgelehnt");
  assert.ok(!fs.existsSync(path.join(DATA, "updates", "bereit.json")));

  // echtes Programm 9.9.9 (richtiger Schlüssel) -> wird geprüft und bereitgelegt
  releases = releases.filter(r => r.tag_name !== "app-v9.9.9");
  rel("app-v9.9.9", {"app.manifest.json":Buffer.from(JSON.stringify(SIG.sign({typ:"app", version:"9.9.9", sha256:sha}, PEM))), "app-bundle.json.gz":bundle});
  let ready = false; updates.onInstallReady(() => { ready = true; });
  await updates.check(true);
  await until(() => ready, 3000, "Programm bereit");
  const r = JSON.parse(fs.readFileSync(path.join(DATA, "updates", "bereit.json"), "utf8"));
  assert.strictEqual(r.version, "9.9.9");
  assert.strictEqual(fs.readFileSync(path.join(r.dir, "app", "server.js"), "utf8"), "böse".replace("böse", "böse"));

  // Paket mit Pfad nach draußen wird abgelehnt
  const evil = zlib.gzipSync(Buffer.from(JSON.stringify({version:"9.9.10", files:{"../../boese.txt":"eA==", "app/server.js":"eA==", "app/package.json":"eA=="}})));
  rel("app-v9.9.10", {"app.manifest.json":Buffer.from(JSON.stringify(SIG.sign({typ:"app", version:"9.9.10", sha256:crypto.createHash("sha256").update(evil).digest("hex")}, PEM))), "app-bundle.json.gz":evil});
  await updates.check(true);
  assert.match(updates.state().error, /unerlaubter Dateipfad/);
  srv.close();
});
