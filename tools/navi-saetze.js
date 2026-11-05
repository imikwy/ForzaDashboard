// Alle Navi-Ansagen (Schlüssel -> Satz). Gemeinsame Liste für die Erzeuger (Piper, Google).
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
// Neue Route, Fahrzeit beim Start, Tank fast leer
SATZ.reroute = "Route wird neu berechnet.";
SATZ.eta = "Du erreichst dein Ziel in etwa";
SATZ.etag = "Du erreichst das Gruppenziel in etwa";
const ZAHL = {1:"einer Minute", 2:"zwei Minuten", 3:"drei Minuten", 4:"vier Minuten", 5:"fünf Minuten", 6:"sechs Minuten", 7:"sieben Minuten", 8:"acht Minuten", 9:"neun Minuten", 10:"zehn Minuten", 12:"zwölf Minuten", 15:"fünfzehn Minuten", 20:"zwanzig Minuten", 25:"fünfundzwanzig Minuten", 30:"dreißig Minuten", 40:"vierzig Minuten", 45:"fünfundvierzig Minuten", 50:"fünfzig Minuten", 60:"einer Stunde"};
for (const [n, t] of Object.entries(ZAHL)) SATZ["min_" + n] = t + ".";
SATZ.min_long = "über einer Stunde.";
SATZ.fuel = "Der Tank ist fast leer.";
SATZ.fuelnext = "Die nächste Tankstelle ist";
SATZ.km_lt1 = "weniger als einen Kilometer entfernt.";
const KM = {1:"einen", 2:"zwei", 3:"drei", 4:"vier", 5:"fünf", 6:"sechs", 8:"acht", 10:"zehn", 15:"fünfzehn", 20:"zwanzig"};
for (const [n, t] of Object.entries(KM)) SATZ["km_" + n] = t + " Kilometer entfernt.";
SATZ.km_gt20 = "mehr als zwanzig Kilometer entfernt.";
for (const [k, t] of Object.entries(TURN)) {
  SATZ["500_" + k] = "In 500 Metern " + t + ".";
  SATZ["200_" + k] = "In 200 Metern " + t + ".";
  SATZ["now_" + k] = "Jetzt " + t + ".";
}
module.exports = SATZ;
