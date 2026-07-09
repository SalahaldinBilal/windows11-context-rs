//! Command execution via ShellExecuteExW: env-var expansion, working directory,
//! show-window mapping, and elevation ("runas") for runAsAdmin entries.
#![cfg(windows)]

use crate::config::{build_param, substitute, MenuConfig, PathVars, MULTI_EACH};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};

const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { p.to_string().unwrap_or_default() }
}

/// Expand %EnvVars% (input returned unchanged if there are none).
pub fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let wide = to_wide(s);
    unsafe {
        let needed = ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), None);
        if needed == 0 {
            return s.to_string();
        }
        let mut buf = vec![0u16; needed as usize + 1];
        let written = ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), Some(&mut buf));
        if written == 0 {
            return s.to_string();
        }
        String::from_utf16_lossy(&buf[..written.saturating_sub(1) as usize])
    }
}

/// showWindowFlag (-1 Hide, 0 Normal, 1 Minimized, 2 Maximized) -> SW_* value.
fn show_cmd(flag: i32) -> i32 {
    match flag {
        -1 => 0, // SW_HIDE
        1 => 2,  // SW_SHOWMINIMIZED
        2 => 3,  // SW_SHOWMAXIMIZED
        _ => 1,  // SW_SHOWNORMAL
    }
}

fn strip_quotes(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// Run one ShellExecuteExW invocation.
fn shell_execute(cfg: &MenuConfig, exe: &str, params: &str, workdir: &str) -> windows::core::Result<()> {
    let verb = to_wide(if cfg.run_as_admin { "runas" } else { "open" });
    let file = to_wide(exe);
    let params_w = to_wide(params);
    let dir_w = to_wide(workdir);

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: if params.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(params_w.as_ptr())
        },
        lpDirectory: if workdir.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(dir_w.as_ptr())
        },
        nShow: show_cmd(cfg.show_window_flag),
        ..Default::default()
    };
    // A cancelled UAC prompt returns an error; callers ignore it deliberately.
    unsafe { ShellExecuteExW(&mut info) }
}

/// Execute `cfg` for the selected `paths`.
/// EACH mode runs once per path; JOIN/single runs once.
pub fn run(cfg: &MenuConfig, paths: &[String]) {
    let exe_raw = strip_quotes(&cfg.exe).to_string();
    let exe = expand_env(&exe_raw);

    let run_one = |paths_for_param: &[String]| {
        let params = build_param(cfg, paths_for_param);
        let workdir = if cfg.working_directory.is_empty() {
            String::new()
        } else {
            let first = PathVars::from_path(paths_for_param.first().map(String::as_str).unwrap_or(""));
            expand_env(&substitute(&cfg.working_directory, &first, &first))
        };
        // Ignore failures (e.g. user cancelled the UAC prompt).
        let _ = shell_execute(cfg, &exe, &params, &workdir);
    };

    if paths.len() > 1 && cfg.accept_multiple_files_flag == MULTI_EACH {
        for p in paths {
            run_one(std::slice::from_ref(p));
        }
    } else {
        run_one(paths);
    }
}
