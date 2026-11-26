// Leistungsmessung für die Oberfläche (src/dashboard.html) – ohne Spiel, mit der eingebauten Demo-Fahrt.
//   node tools/perf-messung.js            -> Tabelle auf der Konsole
//   node tools/perf-messung.js --json x   -> zusätzlich als JSON-Datei (für Vorher/Nachher-Vergleiche)
// Braucht Playwright mit Chromium (require('playwright')). Die Seite wird von einem kleinen lokalen Server
// ausgeliefert; /info antwortet nicht -> das Dashboard startet von selbst die Demo.
// Gemessen wird mit den Chrome-Leistungszählern (Skriptzeit, Layout, Stil-Neuberechnung) und gezählten Bildern.
"use strict";
const http = require("http"), fs = require("fs"), path = require("path");
const { chromium } = require("playwright");

const ROOT = process.env.FD_SRC ? path.resolve(process.env.FD_SRC) : path.join(__dirname, "..", "src");   // FD_SRC=Ordner: anderen Stand messen (Vorher/Nachher)
const TYPES = {".html": "text/html; charset=utf-8", ".js": "text/javascript", ".json": "application/json", ".jpg": "image/jpeg"};
const args = process.argv.slice(2), jsonOut = args.includes("--json") ? args[args.indexOf("--json") + 1] : null;
const SEK = +(process.env.SEK || 6);   // Messdauer je Szenario

function server(){
  return new Promise(res => {
    const s = http.createServer((q, r) => {
      const u = decodeURIComponent(q.url.split("?")[0]);
      const f = u === "/" ? "dashboard.html" : u.replace(/^\/+/, "");
      const p = path.join(ROOT, f);
      if (!p.startsWith(ROOT) || !fs.existsSync(p) || fs.statSync(p).isDirectory()) { r.writeHead(404); r.end(); return; }
      r.writeHead(200, {"content-type": TYPES[path.extname(p)] || "application/octet-stream"}); fs.createReadStream(p).pipe(r);
    });
    s.listen(0, "127.0.0.1", () => res(s));
  });
}

async function metrics(cdp){
  const { metrics } = await cdp.send("Performance.getMetrics");
  const o = {}; for (const m of metrics) o[m.name] = m.value; return o;
}
const diff = (a, b) => ({
  skript: (b.ScriptDuration - a.ScriptDuration) * 1000,
  aufgaben: (b.TaskDuration - a.TaskDuration) * 1000,
  layout: b.LayoutCount - a.LayoutCount,
  stil: b.RecalcStyleCount - a.RecalcStyleCount,
  layoutMs: (b.LayoutDuration - a.LayoutDuration) * 1000,
  stilMs: (b.RecalcStyleDuration - a.RecalcStyleDuration) * 1000,
});

async function scenario(page, cdp, name, setup, sek = SEK){
  if (setup) await page.evaluate(setup);
  await page.waitForTimeout(800);
  await page.evaluate(() => {
    // zählen, wie oft das Dashboard selbst zeichnet: Karte (drawMap) und Anzeigen (updateUI)
    if (!window.__wrapped) { window.__wrapped = 1; window.__n = {map: 0, ui: 0}; const dm = window.drawMap, uu = window.updateUI; window.drawMap = function(){ window.__n.map++; return dm.apply(this, arguments); }; window.updateUI = function(){ window.__n.ui++; return uu.apply(this, arguments); }; }
    window.__n.map = 0; window.__n.ui = 0;
    window.__fr = 0; const f = () => { window.__fr++; window.__raf = requestAnimationFrame(f); }; cancelAnimationFrame(window.__raf); f(); });
  const a = await metrics(cdp), t0 = Date.now();
  await page.waitForTimeout(sek * 1000);
  const b = await metrics(cdp), dt = (Date.now() - t0) / 1000;
  const fr = await page.evaluate(() => window.__fr), cnt = await page.evaluate(() => window.__n);
  // laufende CSS-Animationen (Endlos-Animationen zwingen den Browser zu ständigem Neuzeichnen)
  const anim = await page.evaluate(() => { const o = {}; for (const a of document.getAnimations()) if (a.playState === "running") { const n = a.animationName || a.transitionProperty || "?"; o[n] = (o[n] || 0) + 1; } return o; });
  const d = diff(a, b);
  return {name, animationen: anim, skriptMsProS: d.skript / dt, aufgabenMsProS: d.aufgaben / dt, layoutsProS: d.layout / dt, stilProS: d.stil / dt, layoutMsProS: d.layoutMs / dt, stilMsProS: d.stilMs / dt, bilderProS: fr / dt, karteProS: cnt.map / dt, anzeigeProS: cnt.ui / dt};
}

(async () => {
  const srv = await server(), port = srv.address().port;
  const browser = await chromium.launch({args: ["--disable-gpu-vsync", "--autoplay-policy=no-user-gesture-required"]});
  const ctx = await browser.newContext({viewport: {width: 1600, height: 900}, deviceScaleFactor: 1});
  // Glas-Design wie beim Nutzer (Standard bei Handy/Tablet, am PC eingestellt)
  await ctx.addInitScript(() => { try { if (!localStorage.getItem("fd.settings")) localStorage.setItem("fd.settings", JSON.stringify({design: "glass", gWall: "kirsch", w: {}})); } catch (e) {} });
  const page = await ctx.newPage();
  const errs = []; page.on("pageerror", e => errs.push(String(e && e.message || e)));
  const cdp = await ctx.newCDPSession(page); await cdp.send("Performance.enable");
  await page.goto("http://127.0.0.1:" + port + "/", {waitUntil: "load"});
  await page.waitForFunction(() => typeof demo !== "undefined" && demo === true, null, {timeout: 30000});
  await page.waitForTimeout(2500);

  const out = {zeit: new Date().toISOString(), einstellungen: [], szenarien: []};

  // 1) Einstellungen öffnen: Zeit bis zur fertigen Seite (synchron) + alles, was danach noch nachläuft (1,2 s)
  for (let i = 0; i < 5; i++) {
    await page.evaluate(() => setView("dash")); await page.waitForTimeout(400);
    const a = await metrics(cdp);
    const r = await page.evaluate(() => { const n0 = document.getElementsByTagName("*").length, t0 = performance.now(); openSettings(); const t1 = performance.now(); return {ms: t1 - t0, n0, n1: document.getElementsByTagName("*").length}; });
    await page.waitForTimeout(1200);
    const b = await metrics(cdp), d = diff(a, b);
    out.einstellungen.push({lauf: i + 1, oeffnenMs: r.ms, skriptGesamtMs: d.skript, layouts: d.layout, knoten: r.n1});
  }
  // Kategorie wechseln (Lenkrad, Freunde …)
  out.kategorien = await page.evaluate(() => { const o = {}; for (const c of ["wheel", "friends", "map", "look", "info"]) { const t0 = performance.now(); try { setCatShow(c); } catch (e) {} document.body.offsetHeight; o[c] = performance.now() - t0; } return o; });
  await page.evaluate(() => setView("dash"));

  out.szenarien.push(await scenario(page, cdp, "Home (Demo-Fahrt)", () => setView("dash")));
  out.szenarien.push(await scenario(page, cdp, "Karte groß", () => setView("map")));
  out.szenarien.push(await scenario(page, cdp, "YouTube Vollbild", () => { WEB.yt = "full"; WEB.pip = false; setView("youtube"); window.FD_VIDEO_TEST = 1; }));
  out.szenarien.push(await scenario(page, cdp, "YouTube mit Karte", () => { WEB.yt = "split"; setView("youtube"); }));
  out.szenarien.push(await scenario(page, cdp, "YouTube Mini + Karte", () => { WEB.pip = true; setView("map"); }));
  out.szenarien.push(await scenario(page, cdp, "Einstellungen offen", () => { WEB.pip = false; openSettings(); }));

  const f1 = n => n.toFixed(1).padStart(7);
  console.log("\nEinstellungen öffnen (ms, synchron | Skript gesamt inkl. Nachlauf | Layouts | DOM-Knoten):");
  for (const e of out.einstellungen) console.log("  Lauf " + e.lauf + ":" + f1(e.oeffnenMs) + " |" + f1(e.skriptGesamtMs) + " |" + String(e.layouts).padStart(5) + " |" + String(e.knoten).padStart(7));
  console.log("Kategorie wechseln (ms): " + Object.entries(out.kategorien).map(([k, v]) => k + " " + v.toFixed(1)).join(" · "));
  console.log("\nSzenario                 Skript ms/s  Aufgaben ms/s  Layouts/s  Stil/s  Karte/s  Anzeige/s");
  for (const s of out.szenarien) console.log("  " + s.name.padEnd(22) + f1(s.skriptMsProS) + "     " + f1(s.aufgabenMsProS) + "     " + f1(s.layoutsProS) + "  " + f1(s.stilProS) + "  " + f1(s.karteProS) + "  " + f1(s.anzeigeProS));
  console.log("\nLaufende CSS-Animationen am Ende je Szenario:");
  for (const s of out.szenarien) console.log("  " + s.name.padEnd(22) + (Object.entries(s.animationen).map(([k, v]) => k + (v > 1 ? "×" + v : "")).join(", ") || "–"));
  if (errs.length) console.log("\nFehler auf der Seite:\n  " + [...new Set(errs)].slice(0, 10).join("\n  "));
  out.fehler = [...new Set(errs)];
  if (jsonOut) fs.writeFileSync(jsonOut, JSON.stringify(out, null, 2));
  await browser.close(); srv.close();
})().catch(e => { console.error(e); process.exit(1); });
