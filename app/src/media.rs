// Spotify / Medien: Titel aus dem Spotify-Fenster, Medientasten, Spotify-Lautstärke im Windows-Mixer
use serde::Serialize;
use std::collections::HashMap;
use windows::core::{Interface, BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYBD_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId};

#[derive(Clone, Default, Serialize)]
pub struct Media {
    pub running: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    #[serde(skip)]
    pub vol: Option<f64>,
}

fn process_name(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 520];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
        let _ = CloseHandle(h);
        if !ok { return String::new(); }
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        full.rsplit('\\').next().unwrap_or("").to_string()
    }
}

fn is_spotify(pid: u32, cache: &mut HashMap<u32, bool>) -> bool {
    *cache.entry(pid).or_insert_with(|| process_name(pid).eq_ignore_ascii_case("Spotify.exe"))
}

unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> BOOL {
    let list = &mut *(lp.0 as *mut Vec<(HWND, u32)>);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    list.push((hwnd, pid));
    BOOL(1)
}

// Spotify zeigt beim Abspielen "Interpret - Titel" als Fenstertitel
fn spotify_window(cache: &mut HashMap<u32, bool>) -> (bool, String) {
    let mut wins: Vec<(HWND, u32)> = Vec::new();
    unsafe { let _ = EnumWindows(Some(collect), LPARAM(&mut wins as *mut _ as isize)); }
    let mut running = false;
    let mut title = String::new();
    for (hwnd, pid) in wins {
        if pid == 0 || !is_spotify(pid, cache) { continue; }
        running = true;
        let mut buf = [0u16; 512];
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
        let t = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if !matches!(t.as_str(), "" | "N/A" | "Spotify" | "Spotify Premium" | "Spotify Free") && t.contains(" - ") { title = t; }
    }
    (running, title)
}

// Lautstärke der Spotify-App lesen (delta = None) oder ändern; None, wenn Spotify keinen Ton-Kanal hat
pub fn spotify_volume(delta: Option<f32>) -> Option<f64> {
    let mut cache = HashMap::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let mgr: IAudioSessionManager2 = dev.Activate(CLSCTX_ALL, None).ok()?;
        let sessions = mgr.GetSessionEnumerator().ok()?;
        let mut vols: Vec<ISimpleAudioVolume> = Vec::new();
        for i in 0..sessions.GetCount().ok()? {
            let Ok(c) = sessions.GetSession(i) else { continue };
            let Ok(c2) = c.cast::<IAudioSessionControl2>() else { continue };
            let Ok(pid) = c2.GetProcessId() else { continue };
            if pid != 0 && is_spotify(pid, &mut cache) {
                if let Ok(v) = c.cast::<ISimpleAudioVolume>() { vols.push(v); }
            }
        }
        let mut v = vols.first()?.GetMasterVolume().ok()?;
        if let Some(d) = delta {
            v = (v + d).clamp(0.0, 1.0);
            for x in &vols { let _ = x.SetMasterVolume(v, std::ptr::null()); }
        }
        Some((v as f64 * 1000.0).round() / 1000.0)
    }
}

pub fn poll(spv_on: bool) -> Media {
    let mut cache = HashMap::new();
    let (running, title) = spotify_window(&mut cache);
    let mut m = Media { running, ..Default::default() };
    if let Some(i) = title.find(" - ").filter(|&i| i > 0) {
        m.playing = true;
        m.artist = title[..i].to_string();
        m.title = title[i + 3..].to_string();
    }
    if spv_on && running { m.vol = spotify_volume(None); }
    m
}

fn key_input(vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

pub fn press_key(vk: u16, times: usize) {
    for _ in 0..times {
        let inputs = [key_input(vk, KEYEVENTF_EXTENDEDKEY), key_input(vk, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP)];
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32); }
    }
}

pub fn key_for(cmd: &str) -> Option<u16> {
    Some(match cmd {
        "playpause" => 0xB3,
        "next" => 0xB0,
        "prev" => 0xB1,
        "volup" => 0xAF,
        "voldown" => 0xAE,
        _ => return None,
    })
}
