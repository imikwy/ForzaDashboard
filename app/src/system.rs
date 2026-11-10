// PC-Leistung für Dashboard und Overlay: Prozessor- und Grafikkarten-Auslastung, Arbeits- und Grafikspeicher.
// Nur lesend über Windows (Leistungsindikatoren „PDH“ wie im Task-Manager, DXGI für Name/Speicher der Grafikkarte).
// Jede Sekunde ein Ereignis „sys“: {"cpu":23,"gpu":87,"ram":[11.2,31.9],"vram":[6.1,12.0],"gname":"NVIDIA …"}
use crate::state::Shared;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

static LAST: Mutex<Option<Value>> = Mutex::new(None);

pub fn last() -> Value { LAST.lock().unwrap().clone().unwrap_or(json!({})) }

pub fn start(s: Arc<Shared>) {
    std::thread::spawn(move || unsafe { run(s) });
}

#[cfg(windows)]
unsafe fn ft(f: &windows::Win32::Foundation::FILETIME) -> u64 { ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64 }

// Hauptgrafikkarte: die mit dem meisten eigenen Speicher (keine Software-Adapter)
#[cfg(windows)]
unsafe fn main_gpu() -> Option<(String, f64, String)> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
    let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
    let mut best: Option<(String, f64, String)> = None;
    let mut best_mem = 0usize;
    for i in 0.. {
        let Ok(a) = f.EnumAdapters1(i) else { break };
        let Ok(d) = a.GetDesc1() else { continue };
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || d.DedicatedVideoMemory <= best_mem { continue; }
        best_mem = d.DedicatedVideoMemory;
        let n = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').trim().to_string();
        let luid = format!("luid_0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
        best = Some((n, d.DedicatedVideoMemory as f64 / 1073741824.0, luid));
    }
    best
}

// alle Instanzen eines Zählers mit Wildcard (z. B. jede Grafik-Engine jedes Prozesses)
#[cfg(windows)]
unsafe fn arr(c: windows::Win32::System::Performance::PDH_HCOUNTER) -> Vec<(String, f64)> {
    use windows::Win32::System::Performance::*;
    let (mut size, mut n) = (0u32, 0u32);
    if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut n, None) != PDH_MORE_DATA || size == 0 { return vec![]; }
    let mut buf = vec![0u64; (size as usize + 7) / 8];
    if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut n, Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W)) != 0 { return vec![]; }
    let items = std::slice::from_raw_parts(buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, n as usize);
    items.iter().filter(|it| it.FmtValue.CStatus == 0).map(|it| (it.szName.to_string().unwrap_or_default().to_ascii_lowercase(), it.FmtValue.Anonymous.doubleValue)).collect()
}

#[cfg(windows)]
unsafe fn run(s: Arc<Shared>) {
    use windows::core::w;
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Performance::*;
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::Threading::GetSystemTimes;
    let gpu = main_gpu();
    let mut q = PDH_HQUERY::default();
    let (mut c_eng, mut c_mem) = (PDH_HCOUNTER::default(), PDH_HCOUNTER::default());
    let pdh = PdhOpenQueryW(None, 0, &mut q) == 0;
    if pdh {
        PdhAddEnglishCounterW(q, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut c_eng);
        PdhAddEnglishCounterW(q, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"), 0, &mut c_mem);
        PdhCollectQueryData(q);
    }
    let mut prev: Option<(u64, u64)> = None;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        // Prozessor: Anteil der Zeit, die nicht im Leerlauf war (alle Kerne zusammen)
        let (mut idle, mut kern, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        let mut cpu = None;
        if GetSystemTimes(Some(&mut idle), Some(&mut kern), Some(&mut user)).is_ok() {
            let (i, t) = (ft(&idle), ft(&kern) + ft(&user));
            if let Some((pi, pt)) = prev { let dt = t.saturating_sub(pt); if dt > 0 { cpu = Some((100.0 * (1.0 - i.saturating_sub(pi) as f64 / dt as f64)).clamp(0.0, 100.0)); } }
            prev = Some((i, t));
        }
        let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let ram = if GlobalMemoryStatusEx(&mut ms).is_ok() { let g = 1073741824.0; Some([((ms.ullTotalPhys - ms.ullAvailPhys) as f64 / g * 10.0).round() / 10.0, (ms.ullTotalPhys as f64 / g * 10.0).round() / 10.0]) } else { None };
        // Grafikkarte: wie der Task-Manager – je Engine-Art (3D, Kopieren, Video …) alle Prozesse zusammenzählen, die stärkste zählt
        let (mut gpu_pct, mut vram) = (None, None);
        if pdh && PdhCollectQueryData(q) == 0 {
            let luid = gpu.as_ref().map(|g| g.2.clone()).unwrap_or_default();
            let mut by_type: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
            for (name, v) in arr(c_eng) {
                if !luid.is_empty() && !name.contains(&luid) { continue; }
                let ty = name.rsplit("engtype_").next().unwrap_or("").to_string();
                *by_type.entry(ty).or_default() += v;
            }
            if !by_type.is_empty() { gpu_pct = Some(by_type.values().cloned().fold(0.0, f64::max).clamp(0.0, 100.0)); }
            let used: f64 = arr(c_mem).into_iter().filter(|(n, _)| luid.is_empty() || n.contains(&luid)).map(|(_, v)| v).sum();
            if used > 0.0 { vram = Some([(used / 1073741824.0 * 10.0).round() / 10.0, gpu.as_ref().map(|g| (g.1 * 10.0).round() / 10.0).unwrap_or(0.0)]); }
        }
        let v = json!({"cpu": cpu.map(|c| c.round()), "gpu": gpu_pct.map(|g| g.round()), "ram": ram, "vram": vram, "gname": gpu.as_ref().map(|g| g.0.clone())});
        s.emit(Some("sys"), v.to_string());
        *LAST.lock().unwrap() = Some(v);
    }
}

#[cfg(not(windows))]
unsafe fn run(_s: Arc<Shared>) {}
