# Eigene Automatik: Schaltwippen behalten, ohne Gerätewechsel schalten

Stand: 26.11.2026 · Status: **Konzept + getesteter Code-Baustein, nicht eingebaut** (braucht einen 5-Minuten-Test am echten Spiel)

## Das Problem

Die eigene Automatik (`app/src/schaltung.rs`) schaltet, indem sie für Forza einen Knopf drückt. Dafür gibt es drei Wege:

| Weg | Ergebnis heute |
|---|---|
| **vJoy** (virtueller Joystick im selben eigenen Lenkrad-Profil) | Kein Gerätewechsel, kein Ruck. **Aber:** Forza erlaubt im Menü nur *eine* Belegung je Aktion. Liegt „Hochschalten“ auf vJoy, ist die Wippe dort nicht mehr belegt. Das Programm reicht die Wippe zwar selbst an vJoy weiter (`wippe_btn` → `tap`), ohne laufendes Dashboard schalten die Wippen dann aber gar nicht. |
| **Virtueller Xbox-Controller** (ViGEm) | Forza springt bei jedem Druck kurz auf den Controller um. Das Lenkrad verliert für einen Moment Force Feedback und „schaukelt“. |
| **Tastatur** (E/Q) | Gleiches Umspringen wie beim Controller. |

Gewünscht: Die **Wippen bleiben in Forza belegt** (funktionieren auch ohne Dashboard), und die Automatik schaltet **trotzdem über vJoy**, also ohne Gerätewechsel.

## Was die Recherche ergeben hat

1. **Eigene Lenkrad-Profile sind XML, und jede Belegung trägt ihr eigenes Gerät.** Beispiel aus einem veröffentlichten FH5-Profil ([roxrage/forza_moza_mapping](https://github.com/roxrage/forza_moza_mapping/tree/main/FH5)):
   ```xml
   <RawGameControllerInputMappingProfile … IsDefaultProfile="false" PrimaryDeviceVidPid="0x346e0004" FFBDeviceVidPid="0x346e0004" …>
     <Context Version="1" Context="INPUTCONTEXT_RACING">
       <Value Version="1" Key="INPUTCMD_SHIFTUP"   VidPid="0x346e0004" InputType="Button" Index="13" />
       <Value Version="1" Key="INPUTCMD_SHIFTDOWN" VidPid="0x346e0004" InputType="Button" Index="12" />
       <Value Version="1" Key="INPUTCMD_GEAR_REVERSE" VidPid="0x346e001e" InputType="Button" Index="4" />
   ```
   Ein Profil mischt also schon heute Geräte (Lenkrad + Schaltknüppel). Es gibt keinen Geräte-Index, nur die `VidPid` je Eintrag. Die „eine Belegung je Aktion“ ist eine Grenze des **Menüs**, nicht unbedingt des Dateiformats.
2. **Forza übernimmt von Hand bearbeitete Profile.** Ein bekannter Steam-Guide legt durch Bearbeiten von `InputTranslationManager.Profiles` *zwei Aktionen auf einen Knopf*, was das Menü ebenfalls nicht erlaubt. Danach zeigt das Menü solche Einträge als „nicht belegt“, sie funktionieren aber ([gamepretty.com](https://gamepretty.com/forza-horizon-5-force-controller-key-bindings/)). Ablage bei Steam (FH5): `Steam\userdata\<id>\1551360\remote\<id>\InputTranslationManager.Profiles` ([Simucube-Forum](https://community.granitedevices.com/t/forza-and-simucube-2/3948?page=4)). Für FH6 ist der Pfad nicht belegt: gleiche Struktur unter der FH6-App-ID erwartet, bei der Microsoft-Store-Version unter `%LOCALAPPDATA%\Packages\Microsoft.*\…`.
3. **FH6 unterstützt mehrere Geräte im eigenen Profil** und gibt Force Feedback nur an „Device 1“ ([Forza Support: FH6 Supported Wheels and Devices](https://support.forza.net/hc/en-us/articles/51674028831251-FH6-Supported-Wheels-and-Devices)). vJoy darf also im Profil stehen, solange das Lenkrad Device 1 bleibt. Das ist heute schon so.
4. **vJoy** meldet sich als `VID_1234&PID_BEAD` (`vjoy.inx`). In Forzas Schreibweise wäre das `0x1234bead`. Sicherer ist, die Kennung aus der Datei zu lesen, nachdem vJoy einmal irgendwo belegt wurde.
5. Ungeklärt und nur am Spiel prüfbar: **Nimmt Forza zwei `<Value>`-Einträge mit derselben `Key` an**, und zwar beide statt nur den letzten?

## Lösung A (empfohlen): zweite Belegung direkt in der Profil-Datei

Neben jeder belegten Wippe wird ein gleicher Eintrag für vJoy ergänzt (Knopf 1 = `Index="0"`, Knopf 2 = `Index="1"`):

```xml
<Value Version="1" Key="INPUTCMD_SHIFTUP" VidPid="0x346e0004" InputType="Button" Index="13" />
<!-- ForzaDashboard: vJoy -->
<Value Version="1" Key="INPUTCMD_SHIFTUP" VidPid="0x1234bead" InputType="Button" Index="0" />
```

Dann gilt:
- **Wippen** schalten direkt in Forza, auch ohne Dashboard.
- **Automatik** drückt vJoy-Knopf 1/2. Das ist dasselbe eigene Profil, also kein Gerätewechsel und kein Ruck.
- **Im Programm ist schon alles da:** `schaltung.rs` erkennt beim ersten Wippen-Druck selbst, ob Forza direkt schaltet (`wip_direct = Some(true)`, „Wippe schaltet in Forza direkt – vJoy nur für die Automatik“). Dann reicht es die Wippe *nicht* zusätzlich weiter, also gibt es keinen doppelten Gangwechsel. An `schaltung.rs` muss dafür nichts geändert werden.

### Handtest vor dem Einbau (ca. 5 Minuten)

1. Forza schließen. Steam-Cloud für Forza kurz ausschalten (sonst kann Steam die Datei beim Start überschreiben).
2. Die Profil-Datei sichern (kopieren), z. B. `InputTranslationManager.Profiles`.
3. In einem Texteditor das **eigene** Lenkrad-Profil suchen (`IsDefaultProfile="false"`), darin `Context="INPUTCONTEXT_RACING"`, darin die Zeile mit `INPUTCMD_SHIFTUP`. Die Zeile direkt darunter kopieren, dabei `VidPid` auf die vJoy-Kennung und `Index` auf `0` setzen. Dasselbe für `INPUTCMD_SHIFTDOWN` mit `Index="1"`.
   *Tipp für die vJoy-Kennung:* in Forza vorher kurz eine unwichtige Aktion (z. B. Hupe) auf vJoy legen. In der Datei steht dann die echte `VidPid` von vJoy.
4. Dashboard neu starten (es merkt sich bis zum Neustart, ob die Wippe direkt schaltet), Forza starten, Getriebe „Manuell“. **Test 1:** Wippen schalten? **Test 2:** Im Dashboard „Einrichten → Hoch“ (Probe-Druck) schaltet Forza hoch, ohne dass das Lenkrad kurz locker wird?
5. Beides ja → Lösung A funktioniert. Nur Test 2 ja → Forza nimmt nur den letzten Eintrag, weiter mit Lösung C. Bei Problemen: gesicherte Datei zurückkopieren.

### Einbau danach (Code-Vorschlag liegt bereit)

[`docs/vorschlag/wippen_profil.rs`](vorschlag/wippen_profil.rs) enthält die getesteten Text-Funktionen:
- `vjoy_dazu(xml, vjoy)`: ergänzt die Einträge nur in eigenen Profilen und nur im Fahr-Kontext. Mehrfaches Aufrufen schadet nicht.
- `vjoy_weg(xml)`: entfernt genau die ergänzten Einträge (Rückgängig).
- `vjoy_kennung(xml)`: liest die echte vJoy-Kennung aus der Datei.
- 4 Tests: nur eigenes Profil, nur Fahr-Kontext, Einrückung, idempotent, Rückgängig ergibt das Original byte-genau, unbelegte Wippe wird übersprungen.

Geplanter Ablauf im Programm (Assistent „Einrichten“, neuer Schritt statt „In Forza verbinden“):
1. Prüfen, dass Forza **nicht** läuft (Prozessliste), sonst Hinweis „Forza bitte erst schließen“.
2. Profil-Dateien suchen: Dateien unter `Steam\userdata\*\<App-ID>\remote\**` bzw. im Store-Paketordner, die `<RawGameControllerInputMappingProfile` enthalten.
3. Sicherung `…Profiles.vor-forzadashboard-<Datum>` anlegen, `vjoy_dazu` anwenden, schreiben, erneut lesen und vergleichen.
4. Nach dem nächsten Fahren zeigt `wip_direct` im Fahrtenbuch (`/schaltung/log`), ob die Wippe direkt schaltet. Ein Knopf „Zurücksetzen“ ruft `vjoy_weg` auf bzw. spielt die Sicherung zurück.

Warum nicht schon heute eingebaut: Das Schreiben in Forzas Spielstand-Ordner ohne einen einzigen Test am echten Spiel (FH6-Pfad, Steam-Cloud, ob Forza Doppel-Einträge annimmt) ist zu riskant. Ein kaputtes Profil hieße: Lenkrad im Spiel unbrauchbar, bis die Sicherung zurück ist.

## Lösung B (Alternative): Gänge direkt über vJoy wählen

Das Profil kennt Einzelgänge (`INPUTCMD_GEAR_1` … `INPUTCMD_GEAR_REVERSE`, für H-Schaltungen). Diese könnte man auf vJoy-Knöpfe legen, dann blieben die Wippen auf Hoch/Runter. Nachteile:
- Forza liest H-Schaltungen offenbar nur bei „Manuell mit Kupplung“. Die Automatik müsste also zusätzlich kuppeln.
- Bei einer H-Schaltung ist der Gang nur drin, solange der Knopf gehalten wird. Ein Wippen-Druck dazwischen würde damit kollidieren.
- Mehr Umbau in `schaltung.rs` (Gang statt Richtung).

→ Nur verfolgen, falls Lösung A scheitert *und* Lösung C nicht reicht.

## Lösung C (heute vorhanden, Rückfall)

„Hochschalten/Runterschalten“ nur auf vJoy, das Programm reicht die Wippen weiter. Kein Ruck, aber ohne laufendes Dashboard schalten die Wippen nicht. Mögliche kleine Verbesserung ohne Risiko: Beim Beenden des Dashboards einen Hinweis zeigen, dass die Wippen dann nicht schalten, solange Forza noch läuft.

## Was nicht hilft

- **ViGEm/Controller oder Tastatur:** Forza wechselt das aktive Eingabegerät, und daher kommt das „Schaukeln“. Daran lässt sich von außen nichts ändern.
- **Das echte Lenkrad verstecken und eine Kopie anbieten** (HidHide + virtuelles Gerät): Force Feedback ginge verloren, weil vJoy es nicht ans echte Lenkrad durchreicht.
- **Tasten im echten Lenkrad „einspeisen“:** Das ginge nur mit einem eigenen Treiber, viel zu riskant.

## Quellen

- [Forza Support: FH6 Supported Wheels and Devices](https://support.forza.net/hc/en-us/articles/51674028831251-FH6-Supported-Wheels-and-Devices) (mehrere Geräte im eigenen Profil, FFB an Device 1)
- [Forza Support: Forza Horizon 6 on Wheel – Wheel Input on Steam](https://support.forza.net/hc/en-us/articles/51642975681427-Forza-Horizon-6-on-Wheel-Wheel-Input-on-Steam)
- [roxrage/forza_moza_mapping (FH5-Profile als XML)](https://github.com/roxrage/forza_moza_mapping/tree/main/FH5)
- [Gamepretty: Force Controller Key Bindings (Profil-Datei von Hand bearbeiten)](https://gamepretty.com/forza-horizon-5-force-controller-key-bindings/)
- [Granite Devices Forum: Ablage von InputTranslationManager.Profiles](https://community.granitedevices.com/t/forza-and-simucube-2/3948?page=4)
- [Steam-Diskussion zur Profil-Datei](https://steamcommunity.com/app/1551360/discussions/0/3155328676442500022)
- [vJoy public.h (VID_1234&PID_BEAD)](https://cdn.jsdelivr.net/npm/vjoy@1.3.0/include/public.h)
