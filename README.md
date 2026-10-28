# ForzaDashboard

Telemetrie-Dashboard für Forza Horizon 6 als eigene Windows-App (Rust + Tauri): **eine gemeinsame, geschützte Karte**, gemeinsames Bearbeiten in Echtzeit, dauerhafte Spiel-Lobbys und automatische Updates über GitHub.

Für Spieler steht alles in [LIESMICH.txt](LIESMICH.txt). Diese Datei ist für euch zwei, die Herausgeber.

---

## Einmal einrichten

1. **Repo** `imikwy/ForzaDashboard` anlegen, **öffentlich** (Spieler laden Updates ohne Anmeldung).
2. **Secret** eintragen: Repo → Settings → Secrets and variables → Actions → *New repository secret*
   - Name: `FD_SIGN_KEY`
   - Wert: der komplette Inhalt von `herausgeber.key` (derselbe Schlüssel wie bisher)
3. **Hochladen**, in diesem Ordner:
   ```bash
   git remote add origin https://github.com/imikwy/ForzaDashboard.git
   ```
   ```bash
   git push -u origin main
   ```
   Daraus entsteht automatisch die Kartenversion `karte-v1`.
4. **Kumpel einladen:** Settings → Collaborators → Rolle *Write*. Nur wer Schreibrechte hat, kann veröffentlichen.
5. **main schützen** (empfohlen): Settings → Rules → *New branch ruleset* für `main` mit „Block force pushes“ und „Restrict deletions“.
6. **Erste Programmversion:** Actions → „Programm veröffentlichen“ → *Run workflow*.

## Umstieg der alten exe (einmalig, auf dem PC des Kumpels)

Wer noch die alte ForzaDashboard.exe hat, bekommt die neue automatisch über den alten Update-Weg. Das passiert von selbst, sobald die neue exe auf dem PC mit `herausgeber.key` läuft:

1. `git clone https://github.com/imikwy/ForzaDashboard.git`
2. Im geklonten Ordner `ForzaDashboard-bauen.bat` doppelklicken. Sie baut die exe, legt sie in den bisherigen Ordner auf dem Desktop (dort liegt `herausgeber.key`) und startet sie.
   Liegt `herausgeber.key` woanders: neben die gestartete exe legen und die exe neu starten.
3. Die neue exe verteilt sich jetzt an alle alten exes. Nach ein paar Tagen, wenn alle umgestiegen sind, `herausgeber.key` aus dem Desktop-Ordner nehmen und sicher aufbewahren, damit er nicht wieder mit einem kopierten Ordner verteilt wird.
4. Einmal in einer Bearbeiten-Sitzung **„Fahr-Korrekturen übernehmen“** klicken und veröffentlichen. Damit kommen seine Straßenkorrekturen in die Karte, und alle sehen dieselben Straßen.

## Alltag

| Was | Wie |
|---|---|
| Karte ändern | Dashboard → Karte → Stift → Sitzung starten (Code an die anderen) → bearbeiten → **Veröffentlichen**. GitHub prüft, signiert und verteilt die neue Version in 1–2 Minuten. |
| Neue Programmversion | Code ändern, committen, pushen → Actions → „Programm veröffentlichen“ → *Run workflow*. Jeder bekommt sie automatisch, sobald er gerade nicht fährt. |
| Lokal ausprobieren | `ForzaDashboard-bauen.bat` (zählt die Version hoch, baut, startet) |

Veröffentlichen geht nur auf PCs mit einer Repo-Kopie (die Bat trägt den Ordner automatisch ein) und nur mit Schreibrechten.

## Wie die Karte geschützt ist

- **Eine Wahrheit:** `karte/karte.json` im Repo. Jede Version wird von GitHub geprüft und signiert (`karte-vN`), jedes Dashboard prüft die Signatur, bevor es eine Version benutzt. Versionen sind schreibgeschützt, die letzten 10 bleiben liegen.
- **Gleiche Straßen überall:** Kartenausrichtung (`cfg:mapfit`) und Straßenkorrekturen (`cfg:roadfix`) stehen in der Karte. Lokale Messungen sind nur Vorschläge.
- **Nichts wird überschrieben:** Veröffentlichen führt mit dem neuesten Stand auf GitHub zusammen. Wer mehr als 5 % der Karte (mindestens 200 Objekte) löscht, wird blockiert, außer mit `[massenloeschung-ok]`.
- **Bearbeiten nur in Sitzungen:** Ein Host legt die Reihenfolge fest, jeder PC führt ein Journal (`Sitzungen\`). Fällt der Host aus, übernimmt ein anderer. Getrennt gemachte Änderungen werden beim Wiederverbinden nachgereicht.
- **Geräte-Kennung** ist an den PC gebunden (`%LOCALAPPDATA%\ForzaDashboard\geraet.json`). Kopierte Ordner bekommen eine eigene.

## Aufbau

| Datei | Aufgabe |
|---|---|
| [app/src/karte.rs](app/src/karte.rs) | Prüfung, Format, Signatur, Kartenversionen |
| [app/src/sitzung.rs](app/src/sitzung.rs) | Bearbeiten-Sitzungen, Verlauf, Rückgängig |
| [app/src/lobby.rs](app/src/lobby.rs) | Lobbys per Code |
| [app/src/mqtt.rs](app/src/mqtt.rs), [kanal.rs](app/src/kanal.rs) | zwei Vermittler gleichzeitig, Verschlüsselung (AES-256-GCM, Schlüssel aus dem Code) |
| [app/src/updates.rs](app/src/updates.rs) | Updates von GitHub, automatisch zurück bei Absturz |
| [app/src/veroeffentlichen.rs](app/src/veroeffentlichen.rs) | Sitzung → Commit auf main |
| [app/src/bruecke.rs](app/src/bruecke.rs) | Umstieg der alten exe |
| [src/dashboard.html](src/dashboard.html) | Oberfläche |
| [tools/](tools) | Karten- und Programm-Releases (für GitHub Actions) |

Tests:

```bash
cargo test --manifest-path app/Cargo.toml
```
