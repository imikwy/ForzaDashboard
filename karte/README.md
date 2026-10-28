# Karten-Quelle

`karte.json` ist die **Quelle der Wahrheit** für die gemeinsame Karte. Eine Zeile pro Objekt, sortiert, damit Git jede Änderung genau zeigt.

- Geändert wird sie normalerweise nur über das Dashboard: Bearbeiten-Sitzung → *Veröffentlichen*.
- Jeder Push auf `main`, der diese Datei ändert, erzeugt über GitHub Actions eine neue, signierte Version `karte-vN`. Alle Spieler laden sie automatisch.
- Die Prüfung lehnt ungültige Objekte und Massenlöschungen ab (mehr als 5 % und mindestens 200 Objekte). Absichtlich? Dann `[massenloeschung-ok]` in die Commit-Nachricht schreiben.
- Zurück auf einen alten Stand: `git revert` des betreffenden Commits. Daraus entsteht wieder eine neue Version, gelöscht wird nichts.

Objekt-Arten: `r` Straße · `o` Ort · `e` Radier-Kreis · `x` Ausblendung · `c` Einstellung (`cfg:mapfit` Ausrichtung, `cfg:roadfix` Straßenkorrekturen, `cfg:car:<id>` Tacho). `d:1` steht für gelöscht. Der Grabstein bleibt stehen, damit die Löschung beim Zusammenführen nicht verloren geht.
