// Kleine Helfer: sicheres Schreiben, JSON lesen, Prüfsummen, Protokoll.
"use strict";
const fs = require("fs"), path = require("path"), crypto = require("crypto");

const sha256 = b => crypto.createHash("sha256").update(b).digest("hex");

// Datei absturzsicher schreiben: erst in eine temporäre Datei, auf die Platte zwingen, dann umbenennen.
// Wird der PC mitten drin ausgeschaltet, bleibt entweder die alte oder die neue Datei – nie eine halbe.
function writeAtomic(file, data){
  fs.mkdirSync(path.dirname(file), {recursive:true});
  const tmp = file + "." + process.pid + "." + crypto.randomBytes(3).toString("hex") + ".tmp";
  const fd = fs.openSync(tmp, "w");
  try { fs.writeSync(fd, typeof data === "string" ? Buffer.from(data, "utf8") : data); fs.fsyncSync(fd); }
  finally { fs.closeSync(fd); }
  for (let i = 0; ; i++) {
    try { fs.renameSync(tmp, file); return; }
    catch (e) {   // Windows: Virenscanner/Indexer hält die Datei manchmal kurz fest
      if (i >= 8 || !["EPERM", "EACCES", "EBUSY"].includes(e.code)) { try { fs.unlinkSync(tmp); } catch (x) {} throw e; }
      const t = Date.now() + 40 * (i + 1); while (Date.now() < t) {}
    }
  }
}
function writeJson(file, obj, pretty){ writeAtomic(file, JSON.stringify(obj, null, pretty ? 1 : 0)); }
function readJson(file, fallback){
  try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch (e) { return fallback; }
}
// Zeile an eine Datei anhängen und sofort auf die Platte schreiben (Protokolle, Journal)
function appendLine(file, line){
  fs.mkdirSync(path.dirname(file), {recursive:true});
  const fd = fs.openSync(file, "a");
  try { fs.writeSync(fd, line + "\n"); fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
}

// Protokoll mit Rotation (3 × 1 MB), damit die Datei nicht endlos wächst
let LOGFILE = null;
function setLogFile(f){ LOGFILE = f; }
function log(...a){
  const line = new Date().toISOString().replace("T", " ").slice(0, 19) + "  " + a.map(x => typeof x === "string" ? x : JSON.stringify(x)).join(" ");
  if (process.env.FD_QUIET !== "1") console.log("  " + line.slice(20));
  if (!LOGFILE) return;
  try {
    if (fs.existsSync(LOGFILE) && fs.statSync(LOGFILE).size > 1 << 20) {
      for (let i = 2; i >= 1; i--) { try { fs.renameSync(LOGFILE + "." + i, LOGFILE + "." + (i + 1)); } catch (e) {} }
      fs.renameSync(LOGFILE, LOGFILE + ".1");
    }
    fs.appendFileSync(LOGFILE, line + "\n");
  } catch (e) {}
}

// Lesbare Codes ohne verwechselbare Zeichen (kein 0/O, 1/I/L)
const CODE_ABC = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";
function newCode(n){
  const b = crypto.randomBytes(n); let s = "";
  for (let i = 0; i < n; i++) s += CODE_ABC[b[i] % CODE_ABC.length];
  return s.match(/.{1,3}/g).join("-");
}
function normCode(s){ return String(s || "").toUpperCase().replace(/[^0-9A-Z]/g, ""); }
function validCode(s, n){ const c = normCode(s); return c.length === n && [...c].every(ch => CODE_ABC.includes(ch)); }
function fmtCode(s){ return normCode(s).match(/.{1,3}/g)?.join("-") || ""; }

module.exports = {sha256, writeAtomic, writeJson, readJson, appendLine, log, setLogFile, newCode, normCode, validCode, fmtCode};
