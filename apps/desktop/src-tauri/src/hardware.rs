//! Lightweight Windows hardware and OS prober without up-front FFmpeg benchmarks
//! (ARCHITECTURE §8; PROGRESS_AND_ETA §4; M1.1d-lite Part 3).
//!
//! Collects CPU model + core count, physical RAM, display adapters + driver versions,
//! Windows version/build/N-KN edition status, and AC/battery power plan metadata purely
//! via Win32 APIs and Registry reads (never spawning external binaries).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HardwareError {
    #[error("Failed to probe hardware: {0}")]
    ProbeFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct GpuAdapterInfo {
    pub name: String,
    pub driver_version: Option<String>,
    pub vendor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct HardwareProfile {
    pub cpu_model: String,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub ram_bytes: Option<f64>,
    pub windows_product: String,
    pub windows_display_version: Option<String>,
    pub windows_build: String,
    pub is_windows_n_kn: bool,
    pub webview2_version: Option<String>,
    pub system_locale: String,
    pub timezone: String,
    pub ac_power: Option<bool>,
    pub power_plan: String,
    pub gpus: Vec<GpuAdapterInfo>,
    pub fingerprint: String,
}

impl HardwareProfile {
    /// Returns physical RAM as exact `u64` bytes when known.
    pub fn ram_bytes_u64(&self) -> Option<u64> {
        self.ram_bytes.and_then(|v| {
            if v.is_finite() && v >= 0.0 {
                Some(v as u64)
            } else {
                None
            }
        })
    }

    /// Returns a stable coarse device profile bucket for `stage_stats` indexing.
    pub fn device_profile_key(&self) -> String {
        let ram_gb = self
            .ram_bytes_u64()
            .map(|b| b / (1024 * 1024 * 1024))
            .unwrap_or(0);
        let ram_tier = if ram_gb >= 16 {
            "ram16p"
        } else if ram_gb >= 8 {
            "ram8"
        } else if ram_gb > 0 {
            "ram_low"
        } else {
            "ram_unknown"
        };
        format!(
            "win-{}-c{}-{}",
            ram_tier,
            self.logical_cores,
            &self.fingerprint[..self.fingerprint.len().min(8)]
        )
    }
}

/// Computes a deterministic fingerprint over GPUs, driver versions, and OS build so GPU/driver
/// updates automatically trigger a lazy encoder re-probe.
pub fn compute_hardware_fingerprint(gpus: &[GpuAdapterInfo], windows_build: &str) -> String {
    let mut items: Vec<String> = gpus
        .iter()
        .map(|g| {
            format!(
                "{}|{}",
                g.name.trim().to_ascii_lowercase(),
                g.driver_version
                    .as_deref()
                    .unwrap_or("unknown")
                    .trim()
                    .to_ascii_lowercase()
            )
        })
        .collect();
    items.sort();

    let mut hasher = Sha256::new();
    hasher.update(windows_build.trim().as_bytes());
    for item in items {
        hasher.update(b"\n");
        hasher.update(item.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Detects Windows N / KN editions lacking Inbox Media Foundation codecs unless Media Feature Pack is installed.
pub fn detect_is_windows_n_kn(edition_id: Option<&str>, product_name: &str) -> bool {
    if let Some(ed) = edition_id {
        let upper = ed.trim().to_ascii_uppercase();
        if upper == "KN" || upper.ends_with('N') {
            return true;
        }
    }
    let tokens: Vec<String> = product_name
        .split_whitespace()
        .map(|t| t.to_ascii_uppercase())
        .collect();
    tokens.iter().any(|t| t == "N" || t == "KN")
}

/// Maps well-known Windows Power Scheme GUIDs to readable names.
pub fn map_power_scheme_guid(guid: &str) -> String {
    let norm = guid
        .trim()
        .trim_matches(&['{', '}'][..])
        .to_ascii_lowercase();
    match norm.as_str() {
        "381b4222-f694-41f0-9685-ff5bb260df2e" => "Balanced".to_string(),
        "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c" => "High performance".to_string(),
        "a1841308-3541-4fab-bc81-f71556f20b4a" => "Power saver".to_string(),
        "e9a42b02-d5df-448d-aa00-03f14749eb61" => "Ultimate Performance".to_string(),
        "" => "Unknown".to_string(),
        other => format!("Custom ({})", other),
    }
}

/// Probes current hardware & Windows environment passively.
#[derive(Default)]
pub struct HardwareProber;

impl HardwareProber {
    pub fn new() -> Self {
        Self
    }

    pub fn probe(&self) -> HardwareProfile {
        probe_hardware_profile()
    }
}

/// Derives accurate Windows product name from build number (`>= 22000` => Windows 11),
/// ignoring Windows 11's legacy registry `ProductName` value ("Windows 10 ...").
pub fn derive_windows_product_name(registry_product: &str, build_number: &str) -> String {
    let major_build: u64 = build_number
        .split('.')
        .next()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let trimmed = registry_product.trim();
    if major_build >= 22_000 {
        if let Some(idx) = trimmed.to_ascii_lowercase().find("windows 10") {
            let prefix = &trimmed[..idx];
            let suffix = &trimmed[idx + "windows 10".len()..];
            return format!("{}Windows 11{}", prefix, suffix);
        }
        if trimmed.is_empty() {
            return "Windows 11".to_string();
        }
        if !trimmed.contains("Windows 11") {
            return format!("Windows 11 ({})", trimmed);
        }
    }
    if trimmed.is_empty() {
        "Windows".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Formats registry `ActiveTimeBias` (minutes west of UTC) + `TimeZoneKeyName`.
pub fn format_timezone_label(active_time_bias_raw: Option<u32>, tz_key: Option<&str>) -> String {
    let tz_name = tz_key.map(str::trim).filter(|s| !s.is_empty());
    let offset_str = active_time_bias_raw.map(|raw| {
        let bias_west = raw as i32;
        let offset_east = -bias_west;
        let sign = if offset_east >= 0 { '+' } else { '-' };
        let abs = offset_east.abs();
        format!("UTC{}{:02}:{:02}", sign, abs / 60, abs % 60)
    });
    match (offset_str, tz_name) {
        (Some(off), Some(name)) => format!("{} ({})", off, name),
        (Some(off), None) => off,
        (None, Some(name)) => name.to_string(),
        (None, None) => "Unknown".to_string(),
    }
}

/// Reads passive hardware, OS, GPU driver, RAM, WebView2, locale, timezone, and power metadata.
#[cfg(windows)]
pub fn probe_hardware_profile() -> HardwareProfile {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let logical_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);

    let ram_bytes: Option<u64> = unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) != 0 && status.ullTotalPhys > 0 {
            Some(status.ullTotalPhys)
        } else {
            None
        }
    };

    let ac_power: Option<bool> = unsafe {
        let mut sps: SYSTEM_POWER_STATUS = std::mem::zeroed();
        if GetSystemPowerStatus(&mut sps) != 0 {
            match sps.ACLineStatus {
                1 => Some(true),
                0 => Some(false),
                _ => None,
            }
        } else {
            None
        }
    };

    let cpu_model = reg_read_string(
        HKEY_LOCAL_MACHINE,
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "Unknown CPU".to_string());

    let raw_windows_product = reg_read_string(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "ProductName",
    )
    .unwrap_or_else(|| "Windows".to_string());
    let windows_display_version = reg_read_string(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "DisplayVersion",
    );
    let build_number = reg_read_string(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
    )
    .unwrap_or_else(|| "unknown".to_string());
    let windows_product = derive_windows_product_name(&raw_windows_product, &build_number);
    let ubr = reg_read_dword(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "UBR",
    );
    let windows_build = match ubr {
        Some(u) => format!("{}.{}", build_number, u),
        None => build_number,
    };
    let edition_id = reg_read_string(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "EditionID",
    );
    let is_windows_n_kn = detect_is_windows_n_kn(edition_id.as_deref(), &windows_product);

    const WEBVIEW2_CLIENT_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let webview2_version = reg_read_string(
        HKEY_LOCAL_MACHINE,
        &format!(
            r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{}",
            WEBVIEW2_CLIENT_GUID
        ),
        "pv",
    )
    .or_else(|| {
        reg_read_string(
            HKEY_LOCAL_MACHINE,
            &format!(
                r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{}",
                WEBVIEW2_CLIENT_GUID
            ),
            "pv",
        )
    })
    .or_else(|| {
        reg_read_string(
            HKEY_CURRENT_USER,
            &format!(
                r"Software\Microsoft\EdgeUpdate\Clients\{}",
                WEBVIEW2_CLIENT_GUID
            ),
            "pv",
        )
    })
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());

    let system_locale = reg_read_string(
        HKEY_CURRENT_USER,
        r"Control Panel\International",
        "LocaleName",
    )
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "unknown".to_string());

    let tz_key = reg_read_string(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation",
        "TimeZoneKeyName",
    );
    let active_bias = reg_read_dword(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation",
        "ActiveTimeBias",
    );
    let timezone = format_timezone_label(active_bias, tz_key.as_deref());

    let power_scheme_guid = reg_read_string(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes",
        "ActivePowerScheme",
    )
    .unwrap_or_default();
    let power_plan = map_power_scheme_guid(&power_scheme_guid);

    let mut gpus = Vec::new();
    for idx in 0..16u32 {
        let subkey = format!(
            r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{:04}",
            idx
        );
        let Some(name) = reg_read_string(HKEY_LOCAL_MACHINE, &subkey, "DriverDesc") else {
            continue;
        };
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        let driver_version = reg_read_string(HKEY_LOCAL_MACHINE, &subkey, "DriverVersion");
        let vendor = reg_read_string(HKEY_LOCAL_MACHINE, &subkey, "ProviderName");
        gpus.push(GpuAdapterInfo {
            name: trimmed.to_string(),
            driver_version,
            vendor,
        });
    }

    let fingerprint = compute_hardware_fingerprint(&gpus, &windows_build);

    HardwareProfile {
        cpu_model,
        physical_cores: logical_cores.div_ceil(2).max(1),
        logical_cores,
        ram_bytes: ram_bytes.map(|b| b as f64),
        windows_product,
        windows_display_version,
        windows_build,
        is_windows_n_kn,
        webview2_version,
        system_locale,
        timezone,
        ac_power,
        power_plan,
        gpus,
        fingerprint,
    }
}

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn reg_read_string(
    root: windows_sys::Win32::System::Registry::HKEY,
    subkey: &str,
    value_name: &str,
) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, KEY_READ, REG_EXPAND_SZ, REG_SZ,
    };

    let subkey_w = to_wide_null(subkey);
    let val_w = to_wide_null(value_name);
    let mut hkey: HKEY = std::ptr::null_mut();

    let open_res = unsafe { RegOpenKeyExW(root, subkey_w.as_ptr(), 0, KEY_READ, &mut hkey) };
    if open_res != 0 {
        return None;
    }

    let mut val_type: u32 = 0;
    let mut byte_len: u32 = 0;
    let probe = unsafe {
        RegQueryValueExW(
            hkey,
            val_w.as_ptr(),
            std::ptr::null_mut(),
            &mut val_type,
            std::ptr::null_mut(),
            &mut byte_len,
        )
    };

    if probe != 0 || byte_len == 0 || (val_type != REG_SZ && val_type != REG_EXPAND_SZ) {
        unsafe {
            RegCloseKey(hkey);
        }
        return None;
    }

    let word_count = (byte_len as usize).div_ceil(2);
    let mut buf: Vec<u16> = vec![0; word_count];
    let mut actual_bytes = byte_len;
    let read_res = unsafe {
        RegQueryValueExW(
            hkey,
            val_w.as_ptr(),
            std::ptr::null_mut(),
            &mut val_type,
            buf.as_mut_ptr() as *mut u8,
            &mut actual_bytes,
        )
    };
    unsafe {
        RegCloseKey(hkey);
    }

    if read_res != 0 {
        return None;
    }

    let actual_words = (actual_bytes as usize / 2).min(buf.len());
    let trimmed = &buf[..actual_words];
    let end = trimmed
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(trimmed.len());
    Some(String::from_utf16_lossy(&trimmed[..end]))
}

#[cfg(windows)]
fn reg_read_dword(
    root: windows_sys::Win32::System::Registry::HKEY,
    subkey: &str,
    value_name: &str,
) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, KEY_READ, REG_DWORD,
    };

    let subkey_w = to_wide_null(subkey);
    let val_w = to_wide_null(value_name);
    let mut hkey: HKEY = std::ptr::null_mut();

    let open_res = unsafe { RegOpenKeyExW(root, subkey_w.as_ptr(), 0, KEY_READ, &mut hkey) };
    if open_res != 0 {
        return None;
    }

    let mut val_type: u32 = 0;
    let mut data: u32 = 0;
    let mut byte_len: u32 = std::mem::size_of::<u32>() as u32;
    let read_res = unsafe {
        RegQueryValueExW(
            hkey,
            val_w.as_ptr(),
            std::ptr::null_mut(),
            &mut val_type,
            (&mut data as *mut u32) as *mut u8,
            &mut byte_len,
        )
    };
    unsafe {
        RegCloseKey(hkey);
    }

    if read_res == 0 && val_type == REG_DWORD {
        Some(data)
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn probe_hardware_profile() -> HardwareProfile {
    let logical_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    let gpus = vec![];
    let fingerprint = compute_hardware_fingerprint(&gpus, "non-windows");
    HardwareProfile {
        cpu_model: "Unknown CPU".to_string(),
        physical_cores: logical_cores.div_ceil(2).max(1),
        logical_cores,
        ram_bytes: None,
        windows_product: std::env::consts::OS.to_string(),
        windows_display_version: None,
        windows_build: "non-windows".to_string(),
        is_windows_n_kn: false,
        webview2_version: None,
        system_locale: "en-US".to_string(),
        timezone: "UTC+00:00".to_string(),
        ac_power: None,
        power_plan: "Unknown".to_string(),
        gpus,
        fingerprint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_windows_product_name_uses_build_number() {
        assert_eq!(
            derive_windows_product_name("Windows 10 Pro", "26200"),
            "Windows 11 Pro"
        );
        assert_eq!(
            derive_windows_product_name("Windows 10 Home", "22000.1"),
            "Windows 11 Home"
        );
        assert_eq!(
            derive_windows_product_name("Windows 10 Pro", "19045.4046"),
            "Windows 10 Pro"
        );
        assert_eq!(
            format_timezone_label(Some(4294966876), Some("SE Asia Standard Time")),
            "UTC+07:00 (SE Asia Standard Time)"
        );
    }

    #[test]
    fn test_windows_n_kn_edition_detection() {
        assert!(detect_is_windows_n_kn(Some("CoreN"), "Windows 11 Home N"));
        assert!(detect_is_windows_n_kn(
            Some("ProfessionalKN"),
            "Windows 11 Pro KN"
        ));
        assert!(detect_is_windows_n_kn(None, "Windows 10 Enterprise N"));
        assert!(!detect_is_windows_n_kn(
            Some("Professional"),
            "Windows 11 Pro"
        ));
        assert!(!detect_is_windows_n_kn(
            Some("Core"),
            "Windows 11 Home Single Language"
        ));
    }

    #[test]
    fn test_fingerprint_changes_on_gpu_or_driver_update() {
        let gpus_a = vec![GpuAdapterInfo {
            name: "NVIDIA GeForce RTX 3050".into(),
            driver_version: Some("32.0.15.6636".into()),
            vendor: Some("NVIDIA".into()),
        }];
        let gpus_b = vec![GpuAdapterInfo {
            name: "NVIDIA GeForce RTX 3050".into(),
            driver_version: Some("32.0.15.7000".into()),
            vendor: Some("NVIDIA".into()),
        }];
        assert_ne!(
            compute_hardware_fingerprint(&gpus_a, "26200"),
            compute_hardware_fingerprint(&gpus_b, "26200")
        );
        assert_eq!(
            compute_hardware_fingerprint(&gpus_a, "26200"),
            compute_hardware_fingerprint(&gpus_a, "26200")
        );
    }

    #[test]
    fn test_live_hardware_probe_succeeds() {
        let hw = probe_hardware_profile();
        assert!(!hw.cpu_model.is_empty());
        assert!(hw.logical_cores >= 1);
        assert!(!hw.fingerprint.is_empty());
        assert!(hw.ram_bytes_u64().unwrap_or(1) > 0);
    }
}
