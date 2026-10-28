# ForzaDashboard 3

Telemetrie-Dashboard für Forza Horizon 6 mit **einer gemeinsamen, geschützten Karte**, gemeinsamem Bearbeiten in Echtzeit, dauerhaften Spiel-Lobbys und automatischen Updates über GitHub.

Für Spieler steht alles in [LIESMICH.txt](LIESMICH.txt). Diese Datei richtet sich an die **Herausgeber**, also an euch zwei, die Karte und Programm veröffentlichen.

---

## Einmalige Einrichtung (Herausgeber)

1. **Repo anlegen:** `imikwy/ForzaDashboard` als **öffentliches** Repo. Die Spieler laden Karten und Updates ohne Anmeldung, das geht nur bei öffentlichen Releases. Ein anderer Name? Dann `repo` in [app/config.json](app/config.json) ändern.
2. **Signier-Schlüssel als Secret eintragen:** Repo → Settings → Secrets and variables → Actions → *New repository secret*
   - Name: `FD_SIGN_KEY`
   - Wert: der komplette Inhalt von `C:\Users\ikwyg\ForzaDashboard-Signierschluessel\signier-schluessel.pem`
   - Danach die Datei auf einen USB-Stick sichern und vom PC löschen. Der Schlüssel gehört **nur** ins GitHub-Secret, nie ins Repo und nie zu den Spielern.
3. **Code hochladen** (in diesem Ordner):
   ```bash
   gh auth setup-git
   git remote add origin https://github.com/imikwy/ForzaDashboard.git
   git push -u origin main
   ```
4. **main schützen:** Settings → Rules → *New branch ruleset* für `main`: „Block force pushes“ und „Restrict deletions“ aktivieren. Keine Pull-Request-Pflicht, denn das Dashboard veröffentlicht direkt auf main.
5. **Kumpel einladen:** Settings → Collaborators → mit Rolle *Write*. Nur wer Schreibrechte hat, kann eine Kartenversion veröffentlichen.
6. **Erste Releases:**
   - Karte: Der erste Push erstellt automatisch `karte-v1` aus `karte/karte.json` (Actions → „Karte veröffentlichen“). Fehlte das Secret noch, dort *Run workflow* erneut auslösen.
   - Programm: `git tag app-v3.0.0 && git push origin app-v3.0.0`. Daraus werden `app-v3.0.0` und eine Zip zum Herunterladen.
7. **Kumpel:** `git clone https://github.com/imikwy/ForzaDashboard.git`, dann im Ordner `ForzaDashboard-starten.bat`. Ein Repo-Klon ist automatisch ein „Herausgeber-PC“ und kann veröffentlichen. Alle anderen Spieler laden einfach die Zip aus dem neuesten `app-v…`-Release.

### Wichtig für den Umstieg

- **Straßenkorrekturen deines Kumpels:** Die alte exe hat Korrekturen der Forza-Straßen nur lokal auf seinem PC gespeichert. Deshalb sahen die Straßen bei dir „kaputt“ aus. Beim ersten Start übernimmt das neue Dashboard sie aus dem Speicher der alten exe. Dann: Sitzung starten → im Editor **„Fahr-Korrekturen übernehmen“** → veröffentlichen. Danach sieht jeder exakt dieselbe Karte.
- **Hat dein Kumpel nach dem 28.10. um 04:33 Uhr noch mit der alten exe bearbeitet?** Dann in einer Sitzung seine `karte-gemeinsam.json` aufs Dashboard ziehen. Übernommen wird nur, was neuer ist, und danach wird veröffentlicht.
- Die alte `ForzaDashboard.exe` beendet sich beim ersten Start der neuen Version automatisch. Ihre Dateien im Hauptordner werden nicht mehr benutzt und können später nach `_Archiv` verschoben werden.

---

## Wie es funktioniert

```
           ┌────────────── GitHub (imikwy/ForzaDashboard) ──────────────┐
           │ karte/karte.json  ──Actions: prüfen + signieren──▶ karte-vN  │
           │ Tag app-v3.x.y    ──Actions: Tests + signieren───▶ app-v3.x.y│
           └───────▲──────────────────────────────────────────┬─────────┘
    Veröffentlichen│ (nur Herausgeber, git push)              │ alle 20 min (signiert)
                   │                                          ▼
   ┌─ PC A (Host) ─┴──┐   verschlüsselt über 2 öffentliche   ┌─ PC B ───────────┐
   │ Bearbeiten-       │◀── MQTT-Vermittler gleichzeitig ───▶│ Bearbeiten-       │
   │ Sitzung (Journal) │   (Sitzung: Code, Lobby: Code)       │ Sitzung (Journal) │
   └───────────────────┘                                      └──────────────────┘
```

| Teil | Datei | Aufgabe |
|---|---|---|
| Kartenversionen | [app/lib/karte.js](app/lib/karte.js) | geprüfte, schreibgeschützte Versionen in `daten/karten/`, Selbstreparatur |
| Kartenprüfung | [app/lib/kartenpruefung.js](app/lib/kartenpruefung.js) | Schema, einheitliches Format, Diff, Schutz gegen Massenlöschung, Zusammenführen |
| Bearbeiten-Sitzung | [app/lib/sitzung.js](app/lib/sitzung.js) | Host legt die Reihenfolge fest, Journal auf jedem PC, Host-Wechsel, Prüfsummen-Kette für den Abgleich, Verlauf und Rückgängig |
| Lobbys | [app/lib/lobby.js](app/lib/lobby.js) | dauerhafte Spiel-Lobbys per Code, Positionen und gelernte Straßen |
| Netz | [app/lib/netz.js](app/lib/netz.js), [kanal.js](app/lib/kanal.js) | MQTT über TLS, zwei Vermittler gleichzeitig, AES-256-GCM mit Schlüssel aus dem Code |
| Updates | [app/lib/updates.js](app/lib/updates.js), [starten.js](starten.js) | GitHub Releases, Signaturprüfung, Installation im Stand, automatisches Zurück |
| Veröffentlichen | [app/lib/veroeffentlichen.js](app/lib/veroeffentlichen.js) | Sitzung mit `origin/main` zusammenführen, prüfen, Commit direkt auf main |
| Umzug | [app/lib/umzug.js](app/lib/umzug.js) | einmalige Übernahme aus der alten exe (auch ihr Browser-Speicher) |

**Warum die Karte jetzt auf jedem PC gleich aussieht:** Alles, was die Geometrie beeinflusst, steht in der Kartenversion: gezeichnete Straßen, Radierungen, Ausblendungen, Kartenausrichtung (`cfg:mapfit`) und Straßenkorrekturen (`cfg:roadfix`). Lokale Messungen sind nur noch Vorschläge. Jede Geräte-Kennung ist an den PC gebunden (`%LOCALAPPDATA%\ForzaDashboard\geraet.json`), kopierte Ordner bekommen automatisch eine neue.

**Warum nichts mehr verloren geht:** Die veröffentlichte Karte ist schreibgeschützt. Jede Version bleibt auf GitHub und lokal erhalten, und jede Sitzung führt auf jedem PC ein Journal. Beim Veröffentlichen wird mit dem neuesten Stand zusammengeführt, statt ihn zu überschreiben. GitHub lehnt außerdem Massenlöschungen ohne `[massenloeschung-ok]` ab.

## Entwickeln

```bash
node --test "test/*.test.js"
```

```bash
node tools/telemetrie-simulator.js 5400
```

```bash
node tools/karte.js pruefen
```

Zwei Instanzen auf einem PC testen: `FD_DATA=<ordner> FD_DEVICE_ID=aaaaaaaaaa node app/server.js`, mit eigenen Ports in `<ordner>/einstellungen.txt`.

Neue Programmversion: Version in [app/package.json](app/package.json) erhöhen, committen, dann `git tag app-vX.Y.Z` und pushen.
