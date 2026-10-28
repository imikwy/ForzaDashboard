// Ed25519-Signaturen für Karten- und Programm-Versionen (gleiches Format wie im Programm, siehe app/src/karte.rs).
// Manifest-Datei: {"b": "<JSON-Text>", "s": "<Signatur über genau diesen Text>"}
// Der private Schlüssel liegt nur als GitHub-Secret FD_SIGN_KEY. Das Programm kennt nur den öffentlichen.
"use strict";
const crypto = require("crypto");

const PUB = "MCowBQYDK2VwAyEARFw+U6bmc7q6IDkl02pwPx2CWif6zv+oSYvhpUlze7Y=";

function sign(obj, privPem){
  const b = JSON.stringify(obj);
  const s = crypto.sign(null, Buffer.from(b, "utf8"), crypto.createPrivateKey(privPem)).toString("base64");
  return JSON.stringify({b, s}, null, 1);
}
function verify(raw, pubB64){
  try {
    const m = typeof raw === "string" ? JSON.parse(raw) : raw;
    const key = crypto.createPublicKey({key:Buffer.from(pubB64 || PUB, "base64"), format:"der", type:"spki"});
    if (!crypto.verify(null, Buffer.from(m.b, "utf8"), key, Buffer.from(m.s, "base64"))) return null;
    return JSON.parse(m.b);
  } catch (e) { return null; }
}
// passt der private Schlüssel zum eingebauten öffentlichen?
function matches(privPem){
  return crypto.createPublicKey(crypto.createPrivateKey(privPem)).export({type:"spki", format:"der"}).toString("base64") === PUB;
}
module.exports = {sign, verify, matches, PUB};
