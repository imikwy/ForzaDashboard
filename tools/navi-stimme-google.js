// Navi-Ansagen mit Google Cloud Text-to-Speech erzeugen (einmalig; danach laufen sie offline im Dashboard).
//
// Schlüssel: eine Textdatei mit NUR dem API-Schlüssel, außerhalb des Repos, z. B.
//   Dokumente\projekt carplay\google-tts-schluessel.txt
// (anderer Ort: Umgebungsvariable GOOGLE_TTS_KEY_FILE). Der Schlüssel wird nie ins Repo geschrieben.
//
// Aufruf:
//   node tools/navi-stimme-google.js --liste                      deutsche Stimmen anzeigen
//   node tools/navi-stimme-google.js --probe                      Probesatz je Stimme -> Ordner probe/
//   node tools/navi-stimme-google.js [--f STIMME] [--m STIMME] [--out ORDNER]
//       erzeugt alle Sätze für Frau (f) und Mann (m) -> ORDNER/stimmen.json (base64-MP3) und ORDNER/f|m/*.mp3
const fs = require("fs");
const path = require("path");
const os = require("os");
const SATZ = require("./navi-saetze.js");

const args = process.argv.slice(2);
const opt = k => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : null; };
const keyFile = process.env.GOOGLE_TTS_KEY_FILE || path.join(os.homedir(), "Documents", "projekt carplay", "google-tts-schluessel.txt");
if (!fs.existsSync(keyFile)) { console.error("Schlüsseldatei fehlt: " + keyFile); process.exit(1); }
const KEY = fs.readFileSync(keyFile, "utf8").trim();
if (!/^[A-Za-z0-9_\-]{20,}$/.test(KEY)) { console.error("In der Schlüsseldatei steht kein gültiger API-Schlüssel (nur den Schlüssel, eine Zeile)."); process.exit(1); }
const API = "https://texttospeech.googleapis.com/v1";

async function call(url, body){
  const r = await fetch(url + (url.includes("?") ? "&" : "?") + "key=" + encodeURIComponent(KEY), body ? {method:"POST", headers:{"Content-Type":"application/json"}, body:JSON.stringify(body)} : undefined);
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error("Google antwortet " + r.status + ": " + ((j.error && j.error.message) || "unbekannter Fehler"));
  return j;
}
async function voices(){
  const j = await call(API + "/voices?languageCode=de-DE");
  return (j.voices || []).filter(v => v.languageCodes.includes("de-DE"));
}
// beste Stimme je Geschlecht: Neural2 > WaveNet > Studio > Rest (Chirp/HD bewusst nicht zuerst: andere Sprechweise)
function pick(list, gender){
  const rank = v => (/Neural2/.test(v.name) ? 40 : /Wavenet/.test(v.name) ? 30 : /Studio/.test(v.name) ? 20 : /Chirp/.test(v.name) ? 10 : 0);
  return list.filter(v => v.ssmlGender === gender).sort((a, b) => rank(b) - rank(a) || a.name.localeCompare(b.name))[0];
}
async function say(text, voice){
  const j = await call(API + "/text:synthesize", {input:{text}, voice:{languageCode:"de-DE", name:voice}, audioConfig:{audioEncoding:"MP3", sampleRateHertz:24000, speakingRate:1.0, pitch:0, effectsProfileId:["handset-class-device"]}});
  return Buffer.from(j.audioContent, "base64");
}

(async () => {
  const list = await voices();
  if (args.includes("--liste")) { for (const v of list.sort((a, b) => a.name.localeCompare(b.name))) console.log(v.name.padEnd(28), v.ssmlGender); return; }
  if (args.includes("--probe")) {
    const dir = opt("--out") || "probe"; fs.mkdirSync(dir, {recursive:true});
    for (const v of list.filter(v => !/Chirp|Polyglot|News/.test(v.name))) { fs.writeFileSync(path.join(dir, v.name + ".mp3"), await say("In einem Kilometer erreichst du dein Ziel. Es liegt auf der rechten Seite.", v.name)); console.log("Probe", v.name); }
    return;
  }
  const f = opt("--f") || (pick(list, "FEMALE") || {}).name, m = opt("--m") || (pick(list, "MALE") || {}).name;
  if (!f || !m) throw new Error("Keine passende deutsche Stimme gefunden");
  const out = opt("--out") || "google-stimmen";
  const res = {f:{}, m:{}}; let bytes = 0;
  for (const [sex, voice] of [["f", f], ["m", m]]) {
    fs.mkdirSync(path.join(out, sex), {recursive:true});
    for (const [k, text] of Object.entries(SATZ)) {
      const mp3 = await say(text, voice);
      fs.writeFileSync(path.join(out, sex, k + ".mp3"), mp3);
      res[sex][k] = mp3.toString("base64"); bytes += mp3.length;
    }
    console.log(sex === "f" ? "Frau" : "Mann", voice, Object.keys(SATZ).length, "Sätze");
  }
  res.stimmen = {f, m};
  fs.writeFileSync(path.join(out, "stimmen.json"), JSON.stringify(res));
  console.log("fertig:", path.join(out, "stimmen.json"), Math.round(bytes / 1024), "KB");
})().catch(e => { console.error(e.message); process.exit(1); });
