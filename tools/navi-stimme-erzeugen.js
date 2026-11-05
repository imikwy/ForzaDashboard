// Navi-Ansagen neu erzeugen (einmalig, offline). Voraussetzung in einem leeren Arbeitsordner:
//   npm install sherpa-onnx-node@1.13.8 lamejs@1.2.1   (ab sherpa-onnx 2.0 fehlt der eingebaute espeak-Teil)
//   models/vits-piper-de_DE-thorsten-high und models/vits-piper-de_DE-kerstin-low aus
//   https://github.com/k2-fsa/sherpa-onnx/releases/tag/tts-models entpacken; dann: node navi-stimme-erzeugen.js
// Ergebnis: stimmen.json (base64-MP3) -> in dashboard.html als window.NAVI_VOX einsetzen.
// Navi-Sätze mit Piper-Stimmen (Thorsten = Mann, Kerstin = Frau; beide CC0) erzeugen und als MP3 (base64) ablegen
const fs = require("fs");
const path = require("path");
const sherpa = require("sherpa-onnx-node");
const lamejs = (new Function(fs.readFileSync(require.resolve("lamejs/lame.all.js"), "utf8") + "; return lamejs;"))();
const dir = __dirname;

const SATZ = {
  on: "Navi-Stimme ist an.",
  probe: "Hallo. So klinge ich als Navi-Stimme.",
  km: "In einem Kilometer erreichst du dein Ziel.",
  kmg: "In einem Kilometer erreichst du das Gruppenziel.",
  mi: "In einer halben Meile erreichst du dein Ziel.",
  mig: "In einer halben Meile erreichst du das Gruppenziel.",
  sr: "Es liegt auf der rechten Seite.",
  sl: "Es liegt auf der linken Seite.",
  arr: "Du hast dein Ziel erreicht.",
  arrg: "Du hast das Gruppenziel erreicht.",
  ar: "Es liegt rechts.",
  al: "Es liegt links.",
  av: "Es liegt direkt vor dir.",
  ah: "Es liegt hinter dir.",
};
const TURN = {right:"rechts abbiegen", left:"links abbiegen", sright:"leicht rechts halten", sleft:"leicht links halten", uturn:"wenden"};
for (const [k, t] of Object.entries(TURN)) {
  SATZ["500_" + k] = "In 500 Metern " + t + ".";
  SATZ["200_" + k] = "In 200 Metern " + t + ".";
  SATZ["now_" + k] = "Jetzt " + t + ".";
}

const STIMMEN = {
  m: {name:"thorsten-high", speed:0.92},
  f: {name:"kerstin-low", speed:0.95},
};

function mp3(samples, rate){
  const enc = new lamejs.Mp3Encoder(1, rate, 40);
  // Stille am Anfang/Ende kürzen, leise ein- und ausblenden
  let a = 0, b = samples.length - 1; const thr = 0.008;
  while (a < b && Math.abs(samples[a]) < thr) a++;
  while (b > a && Math.abs(samples[b]) < thr) b--;
  a = Math.max(0, a - Math.round(rate * 0.03)); b = Math.min(samples.length - 1, b + Math.round(rate * 0.12));
  const n = b - a + 1, pcm = new Int16Array(n), fade = Math.round(rate * 0.01);
  let peak = 0; for (let i = a; i <= b; i++) peak = Math.max(peak, Math.abs(samples[i]));
  const gain = peak > 0 ? Math.min(3, 0.89 / peak) : 1;
  for (let i = 0; i < n; i++) { let v = samples[a + i] * gain; if (i < fade) v *= i / fade; if (n - i < fade) v *= (n - i) / fade; pcm[i] = Math.max(-32767, Math.min(32767, Math.round(v * 32767))); }
  const out = []; for (let i = 0; i < n; i += 1152) { const c = enc.encodeBuffer(pcm.subarray(i, i + 1152)); if (c.length) out.push(Buffer.from(c)); }
  const e = enc.flush(); if (e.length) out.push(Buffer.from(e));
  return Buffer.concat(out);
}

const result = {};
let total = 0;
for (const [sex, v] of Object.entries(STIMMEN)) {
  const md = path.join(dir, "models", "vits-piper-de_DE-" + v.name);
  const tts = new sherpa.OfflineTts({model:{vits:{model:path.join(md, "de_DE-" + v.name + ".onnx"), tokens:path.join(md, "tokens.txt"), dataDir:path.join(md, "espeak-ng-data")}, numThreads:2, provider:"cpu", debug:false}, maxNumSentences:1});
  result[sex] = {};
  fs.mkdirSync(path.join(dir, "out", sex), {recursive:true});
  for (const [k, text] of Object.entries(SATZ)) {
    const audio = tts.generate({text, sid:0, speed:v.speed});
    const buf = mp3(audio.samples, audio.sampleRate);
    fs.writeFileSync(path.join(dir, "out", sex, k + ".mp3"), buf);
    result[sex][k] = buf.toString("base64");
    total += buf.length;
  }
  console.log(sex, v.name, Object.keys(SATZ).length, "Sätze");
}
fs.writeFileSync(path.join(dir, "stimmen.json"), JSON.stringify(result));
console.log("MP3 gesamt", Math.round(total / 1024), "KB, base64", Math.round(total * 4 / 3 / 1024), "KB");
