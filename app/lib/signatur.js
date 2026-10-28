// Ed25519-Signaturen für Karten- und Programm-Versionen.
// Der private Schlüssel liegt NUR als GitHub-Secret (FD_SIGN_KEY) – auf keinem PC. Das Programm kennt nur den
// öffentlichen Schlüssel (app/config.json) und installiert nur, was damit nachweislich signiert wurde.
"use strict";
const crypto = require("crypto");

// immer gleiche Schreibweise (sortierte Felder), damit die Signatur unabhängig von der Feld-Reihenfolge ist
function stable(v){
  if (Array.isArray(v)) return "[" + v.map(stable).join(",") + "]";
  if (v && typeof v === "object") return "{" + Object.keys(v).filter(k => v[k] !== undefined).sort().map(k => JSON.stringify(k) + ":" + stable(v[k])).join(",") + "}";
  return JSON.stringify(v);
}
function body(manifest){ const o = Object.assign({}, manifest); delete o.sig; return Buffer.from(stable(o), "utf8"); }
function sign(manifest, privPem){
  const key = crypto.createPrivateKey(privPem);
  return Object.assign({}, manifest, {sig:crypto.sign(null, body(manifest), key).toString("base64")});
}
function verify(manifest, pubB64){
  try {
    if (!manifest || typeof manifest.sig !== "string" || !pubB64) return false;
    const key = crypto.createPublicKey({key:Buffer.from(pubB64, "base64"), format:"der", type:"spki"});
    return crypto.verify(null, body(manifest), key, Buffer.from(manifest.sig, "base64"));
  } catch (e) { return false; }
}
module.exports = {stable, sign, verify};
