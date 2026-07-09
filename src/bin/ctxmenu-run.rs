//! cmrsRun — executes a menu config outside the COM host.
//!
//! Used for the classic (old) context menu, whose registry verbs can only run
//! command lines, and doubles as the package's declared Executable.
//!
//! Usage: cmrsRun.exe <config-file-name.json> <path> [more paths...]
//!
//! Compiled with the windows subsystem so no console window flashes.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(non_snake_case)] // crate name matches the camelCase exe name

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return;
    }
    let config_name = &args[0];
    let mut paths: Vec<String> = args[1..].to_vec();

    // Explorer passes drive roots as "C:\" — normalize trailing-quote damage
    // ("C:\"" from %V expansion) just in case.
    for p in &mut paths {
        if p.ends_with('"') {
            p.pop();
            p.push('\\');
        }
    }

    let dir = cmrs::config_dir();
    let file = dir.join(config_name);
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let Ok(cfg) =
        cmrs::config::MenuConfig::parse(text.trim_start_matches('\u{feff}'))
    else {
        return;
    };
    cmrs::exec::run(&cfg, &paths);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("cmrsRun only works on Windows");
}
