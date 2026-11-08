// Tasten an Lenkrad/Gamepad lesen – auch wenn Forza im Vordergrund ist (Windows „Raw Input“ mit INPUTSINK).
// Jeder neu gedrückte Knopf geht als Ereignis „pad“ an das Dashboard: {"dev":"046D:C266","name":"Logitech","btn":5}
// Steuerkreuz/Hat-Schalter kommen als btn 1001–1008 (oben, oben rechts, … im Uhrzeigersinn).
// Was eine Taste tut, legt das Dashboard fest (Einstellungen → Lenkrad-Tasten); hier wird nur gemeldet.
use crate::state::Shared;
use crate::util::log;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static STATE: OnceLock<Arc<Shared>> = OnceLock::new();
static PREV: Mutex<Option<HashMap<isize, Vec<u16>>>> = Mutex::new(None);
static INFO: Mutex<Option<HashMap<isize, (String, String, Vec<u8>)>>> = Mutex::new(None);

fn vendor(v: u32) -> &'static str {
    match v {
        0x046D => "Logitech",
        0x0EB7 => "Fanatec",
        0x044F => "Thrustmaster",
        0x045E => "Microsoft",
        0x054C => "Sony",
        0x346E => "Moza",
        0x3416 => "Cammus",
        0x16D0 => "Simucube",
        _ => "Controller",
    }
}

pub fn start(s: Arc<Shared>) {
    if STATE.set(s).is_err() { return; }
    std::thread::spawn(|| unsafe { run() });
}

unsafe fn run() {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WNDCLASSW, WINDOW_STYLE};
    let Ok(inst) = GetModuleHandleW(None) else { return };
    let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: w!("FDLenkrad"), ..Default::default() };
    RegisterClassW(&wc);
    let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("FDLenkrad"), w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(inst.into()), None) else { log("Lenkrad-Tasten: kein Empfangsfenster"); return };
    // Joystick (Lenkräder) und Gamepad, auch im Hintergrund
    let devs = [0x04u16, 0x05, 0x08].map(|u| RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: u, dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY, hwndTarget: hwnd });
    if RegisterRawInputDevices(&devs, std::mem::size_of::<RAWINPUTDEVICE>() as u32).is_err() { log("Lenkrad-Tasten: Anmeldung bei Windows fehlgeschlagen"); return; }
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() { DispatchMessageW(&msg); }
}

unsafe extern "system" fn wndproc(hwnd: windows::Win32::Foundation::HWND, msg: u32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, WM_INPUT};
    if msg == WM_INPUT { let _ = std::panic::catch_unwind(|| handle(lp)); }
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe fn handle(lp: windows::Win32::Foundation::LPARAM) {
    use windows::Win32::Devices::HumanInterfaceDevice::{HidP_GetUsageValue, HidP_GetUsages, HidP_Input, HidP_MaxUsageListLength, PHIDP_PREPARSED_DATA};
    use windows::Win32::UI::Input::{GetRawInputData, GetRawInputDeviceInfoW, HRAWINPUT, RAWINPUT, RAWINPUTHEADER, RID_DEVICE_INFO, RIDI_DEVICEINFO, RIDI_PREPARSEDDATA, RID_INPUT, RIM_TYPEHID};
    let h = HRAWINPUT(lp.0 as _);
    let mut size = 0u32;
    let hs = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    GetRawInputData(h, RID_INPUT, None, &mut size, hs);
    if size == 0 || size > 65536 { return; }
    let mut buf = vec![0u8; size as usize];
    if GetRawInputData(h, RID_INPUT, Some(buf.as_mut_ptr() as _), &mut size, hs) != size { return; }
    let ri = &*(buf.as_ptr() as *const RAWINPUT);
    if ri.header.dwType != RIM_TYPEHID.0 { return; }
    let dev = ri.header.hDevice;
    let key = dev.0 as isize;
    // Geräteinfos (Name, vorbereitete HID-Beschreibung) einmal je Gerät holen
    let mut info = INFO.lock().unwrap();
    let map = info.get_or_insert_with(HashMap::new);
    if !map.contains_key(&key) {
        let mut di = RID_DEVICE_INFO { cbSize: std::mem::size_of::<RID_DEVICE_INFO>() as u32, ..Default::default() };
        let mut n = di.cbSize;
        GetRawInputDeviceInfoW(Some(dev), RIDI_DEVICEINFO, Some(&mut di as *mut _ as _), &mut n);
        let (vid, pid) = (di.Anonymous.hid.dwVendorId, di.Anonymous.hid.dwProductId);
        let mut pn = 0u32;
        GetRawInputDeviceInfoW(Some(dev), RIDI_PREPARSEDDATA, None, &mut pn);
        let mut prep = vec![0u8; pn as usize];
        if pn == 0 || GetRawInputDeviceInfoW(Some(dev), RIDI_PREPARSEDDATA, Some(prep.as_mut_ptr() as _), &mut pn) == u32::MAX { return; }
        map.insert(key, (format!("{vid:04X}:{pid:04X}"), vendor(vid).to_string(), prep));
    }
    let (id, name, prep) = map.get(&key).cloned().unwrap();
    drop(info);
    let pp = PHIDP_PREPARSED_DATA(prep.as_ptr() as isize);
    let hid = &ri.data.hid;
    let (sz, cnt) = (hid.dwSizeHid as usize, hid.dwCount as usize);
    if sz == 0 || cnt == 0 { return; }
    let base = std::ptr::addr_of!(hid.bRawData) as *const u8;
    // nur den letzten Bericht des Pakets auswerten (aktuellster Zustand)
    let mut report = std::slice::from_raw_parts(base.add(sz * (cnt - 1)), sz).to_vec();
    let mut pressed: Vec<u16> = Vec::new();
    let max = HidP_MaxUsageListLength(HidP_Input, Some(0x09), pp);
    if max > 0 {
        let mut list = vec![0u16; max as usize];
        let mut len = max;
        if HidP_GetUsages(HidP_Input, 0x09, None, list.as_mut_ptr(), &mut len, pp, &mut report).is_ok() { pressed.extend_from_slice(&list[..len as usize]); }
    }
    // Hat-Schalter (Steuerkreuz): 0–7 = Richtungen, sonst nicht gedrückt
    let mut hat = 0u32;
    if HidP_GetUsageValue(HidP_Input, 0x01, None, 0x39, &mut hat, pp, &mut report).is_ok() && hat < 8 { pressed.push(1001 + hat as u16); }
    pressed.sort_unstable();
    let mut prev = PREV.lock().unwrap();
    let pm = prev.get_or_insert_with(HashMap::new);
    let old = pm.insert(key, pressed.clone()).unwrap_or_default();
    drop(prev);
    for b in pressed.iter().filter(|b| !old.contains(b)) {
        if let Some(s) = STATE.get() { s.emit(Some("pad"), json!({"dev": id, "name": name, "btn": b}).to_string()); }
    }
}
