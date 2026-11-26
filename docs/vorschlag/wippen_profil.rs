// VORSCHLAG – nicht eingebaut (wird nicht mitkompiliert). Siehe docs/gang-automatik-wippen.md.
//
// Ziel: In Forza bleiben die Lenkrad-Wippen auf „Hochschalten/Runterschalten“ belegt UND zusätzlich vJoy-Knopf 1/2 –
// zwei Belegungen für dieselbe Aktion, beide im selben eigenen Lenkrad-Profil. Das Menü von Forza erlaubt nur eine,
// die Profil-Datei speichert aber jede Belegung als eigenen <Value>-Eintrag mit eigener Geräte-Kennung (VidPid):
//
//   <Value Version="1" Key="INPUTCMD_SHIFTUP" VidPid="0x346e0004" InputType="Button" Index="13" />
//
// Dieser Baustein ergänzt neben jedem belegten Wippen-Eintrag einen gleichen Eintrag für vJoy (Index 0 = Knopf 1,
// Index 1 = Knopf 2 – Forza zählt in der Datei ab 0, im Menü ab 1). Er ändert nur eigene Profile
// (IsDefaultProfile="false") und nur im Fahr-Kontext INPUTCONTEXT_RACING. Rückgängig: vjoy_weg().
//
// OB Forza zwei Einträge für dieselbe Aktion annimmt, ist ungeprüft – deshalb erst der Handtest aus der Anleitung.
// Die reinen Text-Funktionen unten sind getestet (cargo test in einem eigenen Projekt, siehe Anleitung).

/// vJoy meldet sich unter Windows als VID 0x1234, PID 0xBEAD (vjoy.inx). Forza schreibt VidPid als 0xVVVVPPPP.
/// Sicherer ist es, die Kennung aus der Datei zu lesen, nachdem man vJoy einmal irgendwo im Profil belegt hat:
/// vjoy_kennung() sucht sie dort.
pub const VJOY_VIDPID: &str = "0x1234bead";

const ZIEL: [(&str, u32); 2] = [("INPUTCMD_SHIFTUP", 0), ("INPUTCMD_SHIFTDOWN", 1)];

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!(" {name}=\"");
    let i = tag.find(&pat)? + pat.len();
    let j = tag[i..].find('"')? + i;
    Some(&tag[i..j])
}

/// Bereiche (Start, Ende) der eigenen Profile und darin der Fahr-Kontexte
fn bloecke<'a>(xml: &'a str, start: &str, ende: &str) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut pos = 0;
    while let Some(a) = xml[pos..].find(start).map(|i| i + pos) {
        let Some(b) = xml[a..].find(ende).map(|i| i + a + ende.len()) else { break };
        v.push((a, b));
        pos = b;
    }
    v
}
fn fahr_kontexte(xml: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (a, b) in bloecke(xml, "<RawGameControllerInputMappingProfile", "</RawGameControllerInputMappingProfile>") {
        let kopf_ende = xml[a..b].find('>').map(|i| i + a).unwrap_or(a);
        if attr(&xml[a..kopf_ende], "IsDefaultProfile") != Some("false") { continue; }   // mitgelieferte Profile nie anfassen
        for (c, d) in bloecke(&xml[a..b], "<Context", "</Context>") {
            let (c, d) = (c + a, d + a);
            let k_ende = xml[c..d].find('>').map(|i| i + c).unwrap_or(c);
            if attr(&xml[c..k_ende], "Context") == Some("INPUTCONTEXT_RACING") { out.push((c, d)); }
        }
    }
    out
}

/// alle <Value …/>-Einträge eines Bereichs: (Start, Ende, Tag-Text)
fn eintraege(xml: &str, a: usize, b: usize) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut pos = a;
    while let Some(i) = xml[pos..b].find("<Value ").map(|i| i + pos) {
        let Some(j) = xml[i..b].find("/>").map(|j| j + i + 2) else { break };
        v.push((i, j));
        pos = j;
    }
    v
}

/// Kennung von vJoy aus der Datei lesen: irgendein Eintrag, dessen VidPid mit 0x1234 beginnt (VID von vJoy)
pub fn vjoy_kennung(xml: &str) -> Option<String> {
    let mut pos = 0;
    while let Some(i) = xml[pos..].find(" VidPid=\"0x1234").map(|i| i + pos) {
        let s = i + " VidPid=\"".len();
        let e = xml[s..].find('"')? + s;
        let id = &xml[s..e];
        if id.len() == 10 { return Some(id.to_ascii_lowercase()); }
        pos = e;
    }
    None
}

/// Neben jeder belegten Wippe (Hoch/Runter) einen vJoy-Eintrag ergänzen. Gibt den neuen Text und die Zahl der
/// ergänzten Einträge zurück. Mehrfach aufrufen schadet nicht (schon vorhandene vJoy-Einträge werden erkannt).
pub fn vjoy_dazu(xml: &str, vjoy: &str) -> (String, usize) {
    let vjoy = vjoy.to_ascii_lowercase();
    let mut einfuegen: Vec<(usize, String)> = Vec::new();
    for (c, d) in fahr_kontexte(xml) {
        let alle = eintraege(xml, c, d);
        for (key, idx) in ZIEL {
            let mit_key: Vec<&(usize, usize)> = alle.iter().filter(|(i, j)| attr(&xml[*i..*j], "Key") == Some(key)).collect();
            if mit_key.iter().any(|(i, j)| attr(&xml[*i..*j], "VidPid").map(|v| v.to_ascii_lowercase()) == Some(vjoy.clone())) { continue; }
            // die belegte Wippe (Knopf eines anderen Geräts) – nur dann lohnt die zweite Belegung
            let Some((i, j)) = mit_key.iter().find(|(i, j)| {
                let t = &xml[*i..*j];
                attr(t, "InputType") == Some("Button") && attr(t, "Index").is_some_and(|x| !x.is_empty())
            }) else { continue };
            let tag = &xml[*i..*j];
            let vid_alt = attr(tag, "VidPid").unwrap_or("");
            let neu = tag.replacen(&format!("VidPid=\"{vid_alt}\""), &format!("VidPid=\"{vjoy}\""), 1)
                .replacen(&format!("Index=\"{}\"", attr(tag, "Index").unwrap_or("")), &format!("Index=\"{idx}\""), 1);
            // gleiche Einrückung wie die Zeile davor
            let zeile = xml[..*i].rfind('\n').map(|n| n + 1).unwrap_or(0);
            let einr = &xml[zeile..*i];
            let einr = if einr.chars().all(char::is_whitespace) { einr } else { "" };
            einfuegen.push((*j, format!("\n{einr}<!-- ForzaDashboard: vJoy -->\n{einr}{neu}")));
        }
    }
    let n = einfuegen.len();
    let mut out = xml.to_string();
    einfuegen.sort_by(|a, b| b.0.cmp(&a.0));   // von hinten, damit die Positionen stimmen
    for (p, t) in einfuegen { out.insert_str(p, &t); }
    (out, n)
}

/// Rückgängig: alle von vjoy_dazu ergänzten Einträge entfernen (am Kommentar erkannt)
pub fn vjoy_weg(xml: &str) -> (String, usize) {
    const MARKE: &str = "<!-- ForzaDashboard: vJoy -->";
    let mut out = xml.to_string();
    let mut n = 0;
    // genau den Block entfernen, den vjoy_dazu eingefügt hat: "\n<Einrückung><Marke>\n<Einrückung><Value … />"
    while let Some(m) = out.find(MARKE) {
        let a = out[..m].rfind('\n').unwrap_or(m);
        let Some(e) = out[m..].find("/>").map(|i| i + m + 2) else { break };
        out.replace_range(a..e, "");
        n += 1;
    }
    (out, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFIL: &str = r#"<Profiles>
  <RawGameControllerInputMappingProfile Version="1" Id="abc" UserFacingName="Mein Lenkrad" IsDefaultProfile="false" PrimaryDeviceVidPid="0x346e0004" FFBDeviceVidPid="0x346e0004" FFBMotorIndex="0">
    <Context Version="1" Context="INPUTCONTEXT_RACING">
      <Value Version="1" Key="INPUTCMD_HORN" VidPid="0x346e0004" InputType="Button" Index="0" />
      <Value Version="1" Key="INPUTCMD_SHIFTUP" VidPid="0x346e0004" InputType="Button" Index="13" />
      <Value Version="1" Key="INPUTCMD_SHIFTDOWN" VidPid="0x346e0004" InputType="Button" Index="12" />
    </Context>
    <Context Version="1" Context="INPUTCONTEXT_UI">
      <Value Version="1" Key="INPUTCMD_SHIFTUP" VidPid="0x346e0004" InputType="Button" Index="5" />
    </Context>
  </RawGameControllerInputMappingProfile>
  <RawGameControllerInputMappingProfile Version="1" Id="std" UserFacingName="Standard" IsDefaultProfile="true" PrimaryDeviceVidPid="0x346e0004">
    <Context Version="1" Context="INPUTCONTEXT_RACING">
      <Value Version="1" Key="INPUTCMD_SHIFTUP" VidPid="0x346e0004" InputType="Button" Index="13" />
    </Context>
  </RawGameControllerInputMappingProfile>
</Profiles>
"#;

    #[test]
    fn ergaenzt_nur_eigenes_profil_im_fahr_kontext() {
        let (neu, n) = vjoy_dazu(PROFIL, VJOY_VIDPID);
        assert_eq!(n, 2);
        assert!(neu.contains(r#"<Value Version="1" Key="INPUTCMD_SHIFTUP" VidPid="0x1234bead" InputType="Button" Index="0" />"#));
        assert!(neu.contains(r#"<Value Version="1" Key="INPUTCMD_SHIFTDOWN" VidPid="0x1234bead" InputType="Button" Index="1" />"#));
        // Wippen bleiben unverändert belegt
        assert!(neu.contains(r#"Key="INPUTCMD_SHIFTUP" VidPid="0x346e0004" InputType="Button" Index="13""#));
        // Standard-Profil und UI-Kontext unberührt
        assert_eq!(neu.matches("0x1234bead").count(), 2);
        // eingerückt wie die Nachbarzeilen
        assert!(neu.contains("\n      <Value Version=\"1\" Key=\"INPUTCMD_SHIFTUP\" VidPid=\"0x1234bead\""));
    }

    #[test]
    fn zweimal_aufrufen_aendert_nichts_mehr() {
        let (a, _) = vjoy_dazu(PROFIL, VJOY_VIDPID);
        let (b, n) = vjoy_dazu(&a, VJOY_VIDPID);
        assert_eq!(n, 0); assert_eq!(a, b);
    }

    #[test]
    fn rueckgaengig_ergibt_das_original() {
        let (a, _) = vjoy_dazu(PROFIL, VJOY_VIDPID);
        let (b, n) = vjoy_weg(&a);
        assert_eq!(n, 2); assert_eq!(b, PROFIL);
    }

    #[test]
    fn unbelegte_wippe_und_kennung_lesen() {
        let leer = PROFIL.replace(r#"Index="13""#, r#"Index="""#);
        let (neu, n) = vjoy_dazu(&leer, VJOY_VIDPID);
        assert_eq!(n, 1, "nur Runter ist belegt");
        assert!(!neu.contains(r#"Key="INPUTCMD_SHIFTUP" VidPid="0x1234bead""#));
        assert_eq!(vjoy_kennung(&neu).as_deref(), Some("0x1234bead"));
        assert_eq!(vjoy_kennung(PROFIL), None);
    }
}
