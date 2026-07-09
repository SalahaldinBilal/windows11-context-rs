//! windows11-context-rs — Windows 11 modern context-menu shell extension.
//!
//! Menu entries are JSON config files in `%LOCALAPPDATA%\ContextMenuRs\custom_commands`;
//! `"runAsAdmin": true` launches the command elevated (UAC prompt).
//!
//! One registered sparse/loose package = one CLSID = one top-level menu entry.
//! The installer generates a package per config file, which is how this project
//! works around Windows 11's one-top-level-entry-per-app rule. The CLSID -> config
//! mapping lives in `%LOCALAPPDATA%\ContextMenuRs\packages.json`:
//!
//! ```json
//! {
//!   "clsids": {
//!     "A1B2C3D4-....": { "title": "Open in Terminal (Admin)", "files": ["Open in Terminal (Admin).json"] }
//!   }
//! }
//! ```
//!
//! A CLSID that isn't listed serves ALL config files under one flyout (fallback,
//! and the "grouped" mode). `files` with several entries makes a named flyout group.

pub mod config;
pub mod setup;

#[cfg(windows)]
pub mod com;
#[cfg(windows)]
pub mod exec;
#[cfg(windows)]
pub mod icons;

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Root data dir: %LOCALAPPDATA%\ContextMenuRs
pub fn data_root() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("ContextMenuRs")
}

/// Where the menu *.json files live.
pub fn config_dir() -> PathBuf {
    data_root().join("custom_commands")
}

/// Where generated (shield-badged) icons live.
pub fn icons_dir() -> PathBuf {
    data_root().join("icons")
}

/// Append to debug.log, but only if the file already exists (opt-in logging:
/// create an empty debug.log to enable, delete it to disable).
pub fn debug_log(msg: &str) {
    let path = data_root().join("debug.log");
    if !path.exists() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(f, "[{}] {}", std::process::id(), msg);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MappingEntry {
    #[serde(default = "default_group_title")]
    pub title: String,
    /// Config file names served by this CLSID. None => all files.
    #[serde(default)]
    pub files: Option<Vec<String>>,
}

fn default_group_title() -> String {
    "Custom Menu".to_string()
}

impl Default for MappingEntry {
    fn default() -> Self {
        Self {
            title: default_group_title(),
            files: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Mapping {
    #[serde(default)]
    clsids: HashMap<String, MappingEntry>,
}

/// Uppercase, no braces: "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"
#[cfg(windows)]
fn format_guid(g: &windows::core::GUID) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

#[cfg(windows)]
fn lookup_entry(clsid: &windows::core::GUID) -> MappingEntry {
    let key = format_guid(clsid);
    let path = data_root().join("packages.json");
    let mapping: Mapping = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default();
    mapping
        .clsids
        .iter()
        .find(|(k, _)| k.trim_matches(['{', '}']).eq_ignore_ascii_case(&key))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- DLL exports

#[cfg(windows)]
mod exports {
    use super::*;
    use windows::core::{Interface, GUID, HRESULT};
    use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER, S_FALSE};

    #[no_mangle]
    pub unsafe extern "system" fn DllGetClassObject(
        rclsid: *const GUID,
        riid: *const GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> HRESULT {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        let clsid = unsafe { *rclsid };
        let entry = lookup_entry(&clsid);
        debug_log(&format!(
            "DllGetClassObject clsid={} title={}",
            format_guid(&clsid),
            entry.title
        ));
        let factory = com::make_factory(entry);
        let hr = unsafe { factory.query(riid, ppv) };
        if hr.is_err() {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        hr
    }

    #[no_mangle]
    pub extern "system" fn DllCanUnloadNow() -> HRESULT {
        S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_parses() {
        let m: Mapping = serde_json::from_str(
            r#"{"clsids":{"11111111-2222-3333-4444-555555555555":{"title":"T","files":["a.json"]}}}"#,
        )
        .unwrap();
        assert_eq!(m.clsids.len(), 1);
        let e = &m.clsids["11111111-2222-3333-4444-555555555555"];
        assert_eq!(e.title, "T");
        assert_eq!(e.files.as_ref().unwrap()[0], "a.json");
    }

    #[test]
    fn mapping_entry_defaults() {
        let e: MappingEntry = serde_json::from_str("{}").unwrap();
        assert_eq!(e.title, "Custom Menu");
        assert!(e.files.is_none());
    }
}
