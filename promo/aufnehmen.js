// Werbeclips als Video aufnehmen – Bild für Bild, ruckelfrei (unabhängig davon, wie schnell der PC ist).
//   node promo/aufnehmen.js                 -> alle Clips, Deutsch und Englisch, 30 Bilder/s
//   node promo/aufnehmen.js clip-2 en 60    -> nur Clip 2, Englisch, 60 Bilder/s
// Braucht: Node, Playwright mit Chromium (npm i playwright && npx playwright install chromium) und ffmpeg im PATH.
// Ergebnis: promo/videos/<clip>-<sprache>.mp4 (H.264, für TikTok/Shorts/Reels/Nexus geeignet)
"use strict";
const fs = require("fs"), path = require("path"), os = require("os"), { spawnSync } = require("child_process");
const { chromium } = require("playwright");

const DIR = __dirname, OUT = path.join(DIR, "videos");
const [nur, sprache, fpsArg] = process.argv.slice(2);
const FPS = +(fpsArg || 30);
const clips = fs.readdirSync(DIR).filter(f => /^clip-.*\.html$/.test(f) && (!nur || f.startsWith(nur)));
const langs = sprache ? [sprache] : ["de", "en"];

(async () => {
  fs.mkdirSync(OUT, {recursive: true});
  const browser = await chromium.launch();
  for (const clip of clips) for (const lang of langs) {
    const page = await browser.newPage({viewport: {width: 1920, height: 1920}});
    await page.goto("file://" + path.join(DIR, clip) + "?rec&lang=" + lang);
    await page.waitForFunction(() => typeof window.renderAt === "function");
    await page.evaluate(() => document.fonts.ready);
    const [w, h, dur] = await page.evaluate(() => { const s = document.getElementById("stage"); return [+s.dataset.w, +s.dataset.h, window.CLIP_DUR]; });
    await page.setViewportSize({width: w, height: h});
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "fd-clip-"));
    const n = Math.round(dur * FPS);
    process.stdout.write(clip + " (" + lang + "): " + n + " Bilder ");
    for (let i = 0; i <= n; i++) {
      await page.evaluate(t => window.renderAt(t), i / FPS);
      await page.screenshot({path: path.join(tmp, String(i).padStart(5, "0") + ".jpg"), type: "jpeg", quality: 95});
      if (i % FPS === 0) process.stdout.write(".");
    }
    const out = path.join(OUT, clip.replace(/\.html$/, "") + "-" + lang + ".mp4");
    const r = spawnSync("ffmpeg", ["-y", "-loglevel", "error", "-framerate", String(FPS), "-i", path.join(tmp, "%05d.jpg"),
      "-c:v", "libx264", "-preset", "slow", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart", out], {stdio: "inherit"});
    fs.rmSync(tmp, {recursive: true, force: true});
    console.log(r.status === 0 ? " -> " + path.relative(process.cwd(), out) + " (" + (fs.statSync(out).size / 1e6).toFixed(1) + " MB)" : " ffmpeg-Fehler");
    await page.close();
  }
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
