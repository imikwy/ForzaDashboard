// Gemeinsame Bausteine der Werbeclips (promo/*.html)
// - feste Bühne (z. B. 1920×1080 oder 1080×1920), passt sich ins Browserfenster ein
// - alles hängt nur von der Zeit t ab (Sekunden): so läuft der Clip live UND lässt sich Bild für Bild sauber aufnehmen
//   (promo/aufnehmen.js ruft window.renderAt(t) auf und macht je Bild ein Bildschirmfoto)
// - Sprache: ?lang=en für Englisch (Standard Deutsch)
// - Tasten: Leertaste = Pause/weiter, R = von vorn, ←/→ = 0,5 s springen
"use strict";
const P = {
  lang: new URLSearchParams(location.search).get("lang") === "en" ? "en" : "de",
  rec: new URLSearchParams(location.search).has("rec"),
};
const tr = (de, en) => P.lang === "en" && en !== undefined ? en : de;
const clamp = (v, a = 0, b = 1) => Math.max(a, Math.min(b, v));
const lerp = (a, b, k) => a + (b - a) * k;
// Fortschritt 0..1 im Zeitfenster [a, b]
const seg = (t, a, b) => clamp((t - a) / (b - a));
const ease = {
  out: k => 1 - Math.pow(1 - k, 3),
  in: k => k * k * k,
  inOut: k => k < .5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2,
  // federt leicht über (für hereinfliegende Glasflächen)
  back: k => { const c1 = 1.55, c3 = c1 + 1; return 1 + c3 * Math.pow(k - 1, 3) + c1 * Math.pow(k - 1, 2); },
  expo: k => k === 1 ? 1 : 1 - Math.pow(2, -10 * k),
};
// ein-/ausblenden in einem Fenster: rein bis a+fi, raus ab b-fo
const fadeWin = (t, a, b, fi = .35, fo = .35) => Math.min(ease.out(seg(t, a, a + fi)), 1 - ease.in(seg(t, b - fo, b)));
const $ = s => document.querySelector(s);
const $$ = s => [...document.querySelectorAll(s)];
function css(el, o){ for (const k in o) el.style[k] = o[k]; }
// deterministischer Zufall (gleiche Werte bei jeder Aufnahme)
function rng(seed){ let s = seed >>> 0 || 1; return () => (s = (s * 1664525 + 1013904223) >>> 0) / 4294967296; }

// Bühne einpassen
function fitStage(){
  const st = $("#stage"), W = +st.dataset.w, H = +st.dataset.h;
  if (P.rec) { css(st, {transform: "none", left: "0", top: "0"}); return; }
  const k = Math.min(innerWidth / W, innerHeight / H);
  css(st, {transform: "scale(" + k + ")", left: (innerWidth - W * k) / 2 + "px", top: (innerHeight - H * k) / 2 + "px"});
}

// Abspielen: render(t) wird jedes Bild aufgerufen; dur = Länge in Sekunden (danach von vorn)
function play(render, dur){
  window.CLIP_DUR = dur;
  window.renderAt = t => render(Math.max(0, Math.min(dur, t)));
  addEventListener("resize", fitStage); fitStage();
  document.documentElement.lang = P.lang;
  $$("[data-de]").forEach(e => { e.innerHTML = tr(e.dataset.de, e.dataset.en); });
  if (P.rec) { document.documentElement.classList.add("rec"); render(0); return; }
  let t0 = performance.now(), paused = false, tp = 0;
  const loop = now => { const t = paused ? tp : ((now - t0) / 1000) % (dur + 1.2); render(Math.min(t, dur)); requestAnimationFrame(loop); };
  addEventListener("keydown", e => {
    if (e.code === "Space") { if (paused) { t0 = performance.now() - tp * 1000; paused = false; } else { tp = ((performance.now() - t0) / 1000) % (dur + 1.2); paused = true; } e.preventDefault(); }
    if (e.key === "r" || e.key === "R") { t0 = performance.now(); paused = false; }
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") { const d = e.key === "ArrowRight" ? .5 : -.5; if (paused) tp = clamp(tp + d, 0, dur); else t0 -= d * 1000; }
  });
  requestAnimationFrame(loop);
}

// ---- gemeinsame Zeichnungen ----
// Straßennetz (Bezier-Kurven in einem 1000×1000-Raster); Fahrweg = Punkte entlang einer Kurve
const ROADS = [
  [[-80, 620], [180, 560], [300, 760], [520, 700], [760, 640], [820, 380], [1080, 300]],
  [[420, -60], [430, 200], [560, 330], [520, 520], [500, 700], [470, 900], [520, 1080]],
  [[-60, 200], [200, 260], [380, 120], [620, 180], [860, 240], [940, 520], [1080, 600]],
  [[120, 1080], [160, 860], [60, 700], [180, 560]],
  [[760, 640], [880, 820], [1080, 860]],
  [[620, 180], [680, 40], [760, -60]],
  [[300, 760], [280, 940], [340, 1080]],
];
// Catmull-Rom-Kurve durch die Punkte -> dichte Punktliste
function spline(pts, n = 24){
  const o = [];
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[i - 1] || pts[i], p1 = pts[i], p2 = pts[i + 1], p3 = pts[i + 2] || p2;
    for (let k = 0; k < n; k++) {
      const t = k / n, t2 = t * t, t3 = t2 * t;
      o.push([0, 1].map(j => .5 * ((2 * p1[j]) + (-p0[j] + p2[j]) * t + (2 * p0[j] - 5 * p1[j] + 4 * p2[j] - p3[j]) * t2 + (-p0[j] + 3 * p1[j] - 3 * p2[j] + p3[j]) * t3)));
    }
  }
  o.push(pts[pts.length - 1].slice());
  return o;
}
// Punkt + Richtung bei Anteil k (0..1) entlang einer Punktliste
function along(path, k){
  if (!path.L) { let L = 0; path.cum = [0]; for (let i = 1; i < path.length; i++) { L += Math.hypot(path[i][0] - path[i - 1][0], path[i][1] - path[i - 1][1]); path.cum.push(L); } path.L = L; }
  const d = clamp(k) * path.L; let i = 1; while (i < path.length - 1 && path.cum[i] < d) i++;
  const a = path[i - 1], b = path[i], f = (d - path.cum[i - 1]) / Math.max(1e-6, path.cum[i] - path.cum[i - 1]);
  return {x: lerp(a[0], b[0], f), y: lerp(a[1], b[1], f), h: Math.atan2(b[0] - a[0], -(b[1] - a[1]))};
}
function drawRoads(g, opt = {}){
  g.lineCap = g.lineJoin = "round";
  for (const pass of [[opt.edge || "#05070a", (opt.w || 14) + 6], [opt.col || "#2b3340", opt.w || 14]]) {
    g.strokeStyle = pass[0]; g.lineWidth = pass[1];
    for (const r of ROADS) { const s = r.s || (r.s = spline(r)); g.beginPath(); s.forEach((p, i) => i ? g.lineTo(p[0], p[1]) : g.moveTo(p[0], p[1])); g.stroke(); }
  }
}
function drawPath(g, path, k0, k1, col, w){
  g.strokeStyle = col; g.lineWidth = w; g.lineCap = g.lineJoin = "round"; g.beginPath();
  const n = 120; for (let i = 0; i <= n; i++) { const p = along(path, lerp(k0, k1, i / n)); i ? g.lineTo(p.x, p.y) : g.moveTo(p.x, p.y); }
  g.stroke();
}
function drawArrow(g, x, y, h, s, col){
  g.save(); g.translate(x, y); g.rotate(h); g.scale(s, s);
  g.fillStyle = col; g.globalAlpha = .22; g.beginPath(); g.arc(0, 0, 26, 0, 7); g.fill(); g.globalAlpha = 1;
  g.fillStyle = "rgba(0,0,0,.5)"; g.beginPath(); g.moveTo(0, -20); g.lineTo(16, 16); g.lineTo(0, 9); g.lineTo(-16, 16); g.closePath(); g.fill();
  g.fillStyle = "#fff"; g.beginPath(); g.moveTo(0, -17); g.lineTo(13, 13); g.lineTo(0, 6); g.lineTo(-13, 13); g.closePath(); g.fill();
  g.fillStyle = col; g.beginPath(); g.moveTo(0, -11); g.lineTo(8.5, 8.5); g.lineTo(0, 3.5); g.lineTo(-8.5, 8.5); g.closePath(); g.fill();
  g.restore();
}
function drawDot(g, x, y, col, r = 9, label){
  g.fillStyle = "#fff"; g.beginPath(); g.arc(x, y, r + 2.5, 0, 7); g.fill();
  g.fillStyle = col; g.beginPath(); g.arc(x, y, r, 0, 7); g.fill();
  if (label) {
    g.font = "700 15px Inter, system-ui, sans-serif"; const tw = g.measureText(label).width + 18;
    g.fillStyle = "rgba(12,14,18,.82)"; g.beginPath(); g.roundRect(x - tw / 2, y - r - 34, tw, 24, 12); g.fill();
    g.fillStyle = col; g.textAlign = "center"; g.textBaseline = "middle"; g.fillText(label, x, y - r - 21.5);
  }
}
// eigenes Zeichen: Tacho-Bogen in einer Glas-Kachel (kein fremdes Logo)
const MARK_SVG = '<svg viewBox="0 0 64 64" fill="none"><defs><linearGradient id="mk" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#6cb6ff"/><stop offset="1" stop-color="#0a84ff"/></linearGradient></defs>' +
  '<path d="M14 42a20 20 0 1 1 36 0" stroke="url(#mk)" stroke-width="6" stroke-linecap="round"/><path d="M32 40 44 24" stroke="#fff" stroke-width="5" stroke-linecap="round"/><circle cx="32" cy="40" r="5" fill="#fff"/></svg>';
