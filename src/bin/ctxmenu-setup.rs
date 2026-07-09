//! cmrsSetup — installs / syncs / uninstalls the shell extension:
//! one loose MSIX package per menu config, plus classic-menu registry entries.
#![allow(non_snake_case)] // crate name matches the camelCase exe name

#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("cmrsSetup only works on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
mod app {
    use cmrs::config::MenuConfig;
    use cmrs::exec::expand_env;
    use cmrs::icons::generate_icon;
    use cmrs::setup::*;
    use cmrs::{config_dir, data_root, icons_dir};
    use std::path::{Path, PathBuf};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Foundation::Uri;
    use windows::Management::Deployment::{DeploymentOptions, PackageManager};
    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE,
        REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_DWORD,
    };
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteExW, SHELLEXECUTEINFOW};
    use windows_collections::IIterable;

    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
    const DEV_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock";
    const DEV_VALUE: &str = "AllowDevelopmentWithoutDevLicense";
    const CLASSIC_ROOTS: [&str; 4] = [
        r"Software\Classes\Directory\shell",
        r"Software\Classes\Directory\Background\shell",
        r"Software\Classes\Drive\shell",
        r"Software\Classes\*\shell",
    ];

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // ---------------------------------------------------------------- cli

    pub fn run() -> i32 {
        let mut cmd: Option<String> = None;
        let mut purge = false;
        let mut dll_arg: Option<PathBuf> = None;
        let mut exe_arg: Option<PathBuf> = None;

        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "install" | "sync" | "uninstall" | "list" | "enable-devmode" => cmd = Some(a),
                "--purge" => purge = true,
                "--dll" => dll_arg = args.next().map(PathBuf::from),
                "--exe" => exe_arg = args.next().map(PathBuf::from),
                "-h" | "--help" | "help" => {
                    usage();
                    return 0;
                }
                other => {
                    eprintln!("Unknown argument: {other}\n");
                    usage();
                    return 2;
                }
            }
        }

        let Some(cmd) = cmd else {
            usage();
            return 0;
        };

        if cmd == "enable-devmode" {
            return if enable_dev_mode() { 0 } else { 1 };
        }

        if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            eprintln!("COM initialization failed: {e}");
            return 1;
        }

        let result = match cmd.as_str() {
            "uninstall" => uninstall(purge),
            "sync" => sync(),
            "list" => list(),
            _ => install(dll_arg, exe_arg),
        };
        match result {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    fn usage() {
        println!(
            "cmrsSetup — installer for windows11-context-rs

Usage: cmrsSetup [install|sync|uninstall|list] [options]

  install         install binaries + register every menu config
  sync            regenerate + re-register packages after editing configs
  uninstall       remove packages + registry entries (keeps configs)
  list            show every config and whether it's installed

Options:
  --purge         with uninstall: also delete configs and all data
  --dll <path>    cmrs.dll location (default: next to this exe)
  --exe <path>    cmrsRun.exe location (default: next to this exe)

Configs live in %LOCALAPPDATA%\\ContextMenuRs\\custom_commands (one JSON file per top-level menu entry)."
        );
    }

    // ---------------------------------------------------------------- paths

    fn bin_dir() -> PathBuf {
        data_root().join("bin")
    }

    fn pkg_root() -> PathBuf {
        data_root().join("packages")
    }

    fn installed_dll() -> PathBuf {
        bin_dir().join("cmrs.dll")
    }

    fn installed_runner() -> PathBuf {
        bin_dir().join("cmrsRun.exe")
    }

    fn ioerr(what: &str, path: &Path, e: std::io::Error) -> String {
        format!("{what} {} failed: {e}", path.display())
    }

    /// Copy overwriting `dest` even while it is loaded in a process (Explorer /
    /// the dllhost surrogate): a mapped image can't be overwritten, but it can
    /// be renamed aside.
    fn replace_file(src: &Path, dest: &Path) -> Result<(), String> {
        let stale = dest.with_file_name(format!(
            "{}.old",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
        let _ = std::fs::remove_file(&stale);
        match std::fs::copy(src, dest) {
            Ok(_) => Ok(()),
            Err(e) if e.raw_os_error() == Some(32) => {
                std::fs::rename(dest, &stale).map_err(|e| ioerr("moving aside", dest, e))?;
                std::fs::copy(src, dest)
                    .map(drop)
                    .map_err(|e| ioerr("copying to", dest, e))
            }
            Err(e) => Err(ioerr("copying to", dest, e)),
        }
    }

    // ---------------------------------------------------------------- commands

    fn install(dll_arg: Option<PathBuf>, exe_arg: Option<PathBuf>) -> Result<(), String> {
        ensure_dev_mode()?;
        for d in [bin_dir(), config_dir(), pkg_root()] {
            std::fs::create_dir_all(&d).map_err(|e| ioerr("creating", &d, e))?;
        }

        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let dll = dll_arg.unwrap_or_else(|| exe_dir.join("cmrs.dll"));
        let runner = exe_arg.unwrap_or_else(|| exe_dir.join("cmrsRun.exe"));
        for f in [&dll, &runner] {
            if !f.is_file() {
                return Err(format!(
                    "{} not found. Build first (cargo build --release) or pass --dll/--exe.",
                    f.display()
                ));
            }
        }
        replace_file(&dll, &installed_dll())?;
        replace_file(&runner, &installed_runner())?;
        println!("Binaries installed to {}", bin_dir().display());

        sync_configs()
    }

    fn sync() -> Result<(), String> {
        ensure_dev_mode()?;
        for f in [installed_dll(), installed_runner()] {
            if !f.is_file() {
                return Err(format!(
                    "missing {} — run `cmrsSetup install` first.",
                    f.display()
                ));
            }
        }
        sync_configs()
    }

    fn uninstall(purge: bool) -> Result<(), String> {
        println!("Uninstalling windows11-context-rs...");
        let pm = PackageManager::new().map_err(|e| format!("PackageManager: {e}"))?;
        remove_stale_packages(&pm, &[]);
        remove_stale_classic_entries(&[]);
        for dir in [pkg_root(), icons_dir()] {
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| ioerr("removing", &dir, e))?;
            }
        }
        if purge {
            if data_root().exists() {
                std::fs::remove_dir_all(data_root())
                    .map_err(|e| ioerr("removing", &data_root(), e))?;
            }
            println!("Removed all data including configs.");
        } else {
            println!("Configs kept in {}", config_dir().display());
        }
        println!("Done. Restart Explorer if entries are still visible.");
        Ok(())
    }

    fn list() -> Result<(), String> {
        let dir = config_dir();
        if !dir.is_dir() {
            println!("No configs found in {} (not installed yet).", dir.display());
            return Ok(());
        }
        let configs = list_configs()?;
        if configs.is_empty() {
            println!("No configs found in {}.", dir.display());
            return Ok(());
        }

        let pm = PackageManager::new().map_err(|e| format!("PackageManager: {e}"))?;
        let installed = registered_identities(&pm);

        for file_name in &configs {
            let path = dir.join(file_name);
            let status = match std::fs::read_to_string(&path) {
                Err(e) => format!("error reading file: {e}"),
                Ok(text) => match MenuConfig::parse(text.trim_start_matches('\u{feff}')) {
                    Err(e) => format!("invalid config: {e}"),
                    Ok(cfg) => {
                        let identity = format!("{ID_PREFIX}{}", slug(file_name));
                        let item_types = item_types_xml(cfg.dir_flag(), cfg.file_flag(), &identity);
                        if item_types.is_empty() {
                            "skipped (matches nothing)".to_string()
                        } else if installed.contains(&identity) {
                            "installed".to_string()
                        } else {
                            "not installed (run `cmrsSetup sync`)".to_string()
                        }
                    }
                },
            };
            println!("  {file_name}  ->  {status}");
        }
        Ok(())
    }

    // ---------------------------------------------------------------- sync core

    fn sync_configs() -> Result<(), String> {
        let pm = PackageManager::new().map_err(|e| format!("PackageManager: {e}"))?;
        std::fs::create_dir_all(icons_dir()).map_err(|e| ioerr("creating", &icons_dir(), e))?;

        let configs = list_configs()?;
        if configs.is_empty() {
            println!("No configs found in {}.", config_dir().display());
            println!("Add *.json files there (see the repo's examples folder), then run `cmrsSetup sync`.");
        }
        let mut mapping: Vec<(String, String, String)> = Vec::new();
        let mut live_identities: Vec<String> = Vec::new();
        let mut live_icons: Vec<String> = Vec::new();
        let mut registered = 0usize;

        for file_name in &configs {
            let path = config_dir().join(file_name);
            let text = std::fs::read_to_string(&path).map_err(|e| ioerr("reading", &path, e))?;
            let cfg = MenuConfig::parse(text.trim_start_matches('\u{feff}'))
                .map_err(|e| format!("failed to parse {file_name}: {e}"))?;

            let slug = slug(file_name);
            let clsid = deterministic_clsid(file_name);
            let identity = format!("{ID_PREFIX}{slug}");
            live_identities.push(identity.clone());
            let title = if cfg.title.trim().is_empty() {
                slug.clone()
            } else {
                cfg.title.clone()
            };

            let item_types = item_types_xml(cfg.dir_flag(), cfg.file_flag(), &clsid);
            if item_types.is_empty() {
                println!("  skipping {file_name} (matches nothing)");
                continue;
            }

            let pkg_dir = pkg_root().join(&slug);
            let assets = pkg_dir.join("Assets");
            std::fs::create_dir_all(&assets).map_err(|e| ioerr("creating", &assets, e))?;
            std::fs::write(assets.join("logo.png"), LOGO_PNG)
                .map_err(|e| ioerr("writing", &assets.join("logo.png"), e))?;
            for (src, name) in [
                (installed_dll(), "cmrs.dll"),
                (installed_runner(), "cmrsRun.exe"),
            ] {
                replace_file(&src, &pkg_dir.join(name))?;
            }
            let manifest_path = pkg_dir.join("AppxManifest.xml");
            std::fs::write(
                &manifest_path,
                manifest_xml(&identity, &title, &clsid, &item_types),
            )
            .map_err(|e| ioerr("writing", &manifest_path, e))?;

            mapping.push((clsid.clone(), title.clone(), file_name.clone()));

            println!("  registering {identity}  ->  \"{title}\"  [{clsid}]");
            register_package(&pm, &identity, &manifest_path)
                .map_err(|e| format!("registering {identity}: {e}"))?;
            registered += 1;

            let mut menu_icon = expand_env(cfg.resolve_icon_spec(cfg.icon.trim_matches('"')));
            if cfg.uses_generated_icon() {
                let badge_expanded = cfg
                    .badge_spec()
                    .map(|s| expand_env(cfg.resolve_icon_spec(s)));
                let badge = badge_expanded.as_deref().map(|s| (s, cfg.badge_corner()));
                if badge.is_some() || !cfg.icon.trim().is_empty() {
                    let name = generated_icon_name(&slug, false);
                    let dest = icons_dir().join(&name);
                    if generate_icon(&expand_env(cfg.resolve_icon_spec(&cfg.icon)), &dest, badge) {
                        menu_icon = dest.to_string_lossy().into_owned();
                        live_icons.push(name);
                    } else {
                        println!("  warning: could not generate icon for {file_name}");
                    }
                }
                if !cfg.icon_dark.trim().is_empty() {
                    let name = generated_icon_name(&slug, true);
                    if generate_icon(
                        &expand_env(cfg.resolve_icon_spec(&cfg.icon_dark)),
                        &icons_dir().join(&name),
                        badge,
                    ) {
                        live_icons.push(name);
                    }
                }
            }

            write_classic_entries(&slug, &title, &cfg, file_name, &menu_icon)?;
        }

        let mapping_path = data_root().join("packages.json");
        std::fs::write(&mapping_path, packages_json(&mapping))
            .map_err(|e| ioerr("writing", &mapping_path, e))?;

        remove_stale_packages(&pm, &live_identities);
        remove_stale_classic_entries(&live_identities);
        remove_stale_icons(&live_icons);

        println!();
        println!(
            "Done - {registered} menu entr{} registered.",
            if registered == 1 { "y" } else { "ies" }
        );
        println!("If entries are not visible yet, restart Explorer (Task Manager > Windows Explorer > Restart).");
        println!(
            "Edit configs in {} then re-run `cmrsSetup sync`.",
            config_dir().display()
        );
        Ok(())
    }

    fn list_configs() -> Result<Vec<String>, String> {
        let dir = config_dir();
        let entries = std::fs::read_dir(&dir).map_err(|e| ioerr("reading", &dir, e))?;
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_ascii_lowercase().ends_with(".json"))
            .collect();
        names.sort();
        Ok(names)
    }

    // ---------------------------------------------------------------- msix

    fn file_uri(p: &Path) -> String {
        let s = p.to_string_lossy().replace('\\', "/");
        let mut enc = String::new();
        for c in s.chars() {
            match c {
                '%' => enc.push_str("%25"),
                ' ' => enc.push_str("%20"),
                '#' => enc.push_str("%23"),
                '?' => enc.push_str("%3F"),
                _ => enc.push(c),
            }
        }
        format!("file:///{}", enc.trim_start_matches('/'))
    }

    /// Dev-mode registration refuses a same-version package whose contents changed.
    const PACKAGE_ALREADY_EXISTS: i32 = 0x8007_3CFB_u32 as i32;

    fn try_register(pm: &PackageManager, uri: &Uri) -> Result<(), (i32, String)> {
        let options =
            DeploymentOptions::DevelopmentMode | DeploymentOptions::ForceApplicationShutdown;
        let op = pm
            .RegisterPackageAsync(uri, None::<&IIterable<Uri>>, options)
            .map_err(|e| (e.code().0, e.to_string()))?;
        match op.join() {
            Ok(res) if res.IsRegistered().unwrap_or(false) => Ok(()),
            Ok(res) => Err((
                res.ExtendedErrorCode().map(|h| h.0).unwrap_or(0),
                res.ErrorText()
                    .map(|t| t.to_string())
                    .unwrap_or_else(|_| "registration failed".into()),
            )),
            Err(e) => Err((e.code().0, format!("{e} (is Developer Mode enabled?)"))),
        }
    }

    fn register_package(
        pm: &PackageManager,
        identity: &str,
        manifest: &Path,
    ) -> Result<(), String> {
        let uri = Uri::CreateUri(&HSTRING::from(file_uri(manifest))).map_err(|e| e.to_string())?;
        match try_register(pm, &uri) {
            Err((PACKAGE_ALREADY_EXISTS, _)) => {
                println!("  package contents changed - replacing the registered package");
                remove_package(pm, identity);
                try_register(pm, &uri).map_err(|(_, msg)| msg)
            }
            other => other.map_err(|(_, msg)| msg),
        }
    }

    fn remove_package(pm: &PackageManager, identity: &str) {
        let Ok(packages) = pm.FindPackagesByUserSecurityId(&HSTRING::new()) else {
            return;
        };
        for pkg in packages {
            let Ok(id) = pkg.Id() else { continue };
            let name = id.Name().map(|n| n.to_string()).unwrap_or_default();
            if name.eq_ignore_ascii_case(identity) {
                if let Ok(full) = id.FullName() {
                    if let Ok(op) = pm.RemovePackageAsync(&full) {
                        let _ = op.join();
                    }
                }
            }
        }
    }

    fn registered_identities(pm: &PackageManager) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        let Ok(packages) = pm.FindPackagesByUserSecurityId(&HSTRING::new()) else {
            return out;
        };
        for pkg in packages {
            let Ok(id) = pkg.Id() else { continue };
            let name = id.Name().map(|n| n.to_string()).unwrap_or_default();
            if name.starts_with(ID_PREFIX) {
                out.insert(name);
            }
        }
        out
    }

    fn remove_stale_packages(pm: &PackageManager, keep_identities: &[String]) {
        let Ok(packages) = pm.FindPackagesByUserSecurityId(&HSTRING::new()) else {
            return;
        };
        for pkg in packages {
            let Ok(id) = pkg.Id() else { continue };
            let name = id.Name().map(|n| n.to_string()).unwrap_or_default();
            if !name.starts_with(ID_PREFIX)
                || keep_identities
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            println!("  removing package {name}");
            if let Ok(full) = id.FullName() {
                if let Ok(op) = pm.RemovePackageAsync(&full) {
                    let _ = op.join();
                }
            }
        }
    }

    // ---------------------------------------------------------------- classic menu

    fn classic_targets(cfg: &MenuConfig) -> Vec<(&'static str, &'static str)> {
        let mut v = Vec::new();
        if cfg.dir_flag() & 1 != 0 {
            v.push((CLASSIC_ROOTS[0], "%V"));
        }
        if cfg.dir_flag() & 2 != 0 {
            v.push((CLASSIC_ROOTS[1], "%V"));
        }
        if cfg.dir_flag() & 8 != 0 {
            v.push((CLASSIC_ROOTS[2], "%V"));
        }
        if cfg.file_flag() > 0 {
            v.push((CLASSIC_ROOTS[3], "%1"));
        }
        v
    }

    fn write_classic_entries(
        slug: &str,
        title: &str,
        cfg: &MenuConfig,
        file_name: &str,
        icon: &str,
    ) -> Result<(), String> {
        let runner = installed_runner();
        for (root, arg) in classic_targets(cfg) {
            let key = format!("{root}\\{ID_PREFIX}{slug}");
            set_reg_sz(&key, None, title)?;
            if !icon.is_empty() {
                set_reg_sz(&key, Some("Icon"), icon)?;
            }
            let command = format!("\"{}\" \"{file_name}\" \"{arg}\"", runner.display());
            set_reg_sz(&format!("{key}\\command"), None, &command)?;
        }
        Ok(())
    }

    fn remove_stale_icons(keep: &[String]) {
        let Ok(entries) = std::fs::read_dir(icons_dir()) else {
            return;
        };
        for e in entries.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    fn remove_stale_classic_entries(keep_identities: &[String]) {
        for root in CLASSIC_ROOTS {
            for name in reg_subkeys(root) {
                if name.starts_with(ID_PREFIX)
                    && !keep_identities
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(&name))
                {
                    let sub = wide(&format!("{root}\\{name}"));
                    let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr())) };
                }
            }
        }
    }

    // ---------------------------------------------------------------- registry helpers

    fn set_reg_sz(subkey: &str, value_name: Option<&str>, data: &str) -> Result<(), String> {
        let sub_w = wide(subkey);
        let mut hkey = HKEY::default();
        let err = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub_w.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            )
        };
        if err != ERROR_SUCCESS {
            return Err(format!("creating registry key {subkey} failed ({})", err.0));
        }
        let name_w = value_name.map(wide);
        let data_w = wide(data);
        let bytes =
            unsafe { std::slice::from_raw_parts(data_w.as_ptr() as *const u8, data_w.len() * 2) };
        let err = unsafe {
            RegSetValueExW(
                hkey,
                name_w
                    .as_ref()
                    .map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())),
                None,
                REG_SZ,
                Some(bytes),
            )
        };
        let _ = unsafe { RegCloseKey(hkey) };
        if err != ERROR_SUCCESS {
            return Err(format!(
                "writing registry value in {subkey} failed ({})",
                err.0
            ));
        }
        Ok(())
    }

    fn reg_subkeys(subkey: &str) -> Vec<String> {
        let mut out = Vec::new();
        let sub_w = wide(subkey);
        let mut hkey = HKEY::default();
        let err = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub_w.as_ptr()),
                None,
                KEY_READ,
                &mut hkey,
            )
        };
        if err != ERROR_SUCCESS {
            return out;
        }
        let mut index = 0u32;
        loop {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            let err = unsafe {
                RegEnumKeyExW(
                    hkey,
                    index,
                    Some(PWSTR(buf.as_mut_ptr())),
                    &mut len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if err != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
            index += 1;
        }
        let _ = unsafe { RegCloseKey(hkey) };
        out
    }

    // ---------------------------------------------------------------- developer mode

    fn dev_mode_enabled() -> bool {
        let key_w = wide(DEV_KEY);
        let val_w = wide(DEV_VALUE);
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let err = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_w.as_ptr()),
                PCWSTR(val_w.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut data as *mut u32 as *mut _),
                Some(&mut size),
            )
        };
        err == ERROR_SUCCESS && data == 1
    }

    fn enable_dev_mode() -> bool {
        let key_w = wide(DEV_KEY);
        let mut hkey = HKEY::default();
        let err = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_w.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            )
        };
        if err != ERROR_SUCCESS {
            return false;
        }
        let val_w = wide(DEV_VALUE);
        let err = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(val_w.as_ptr()),
                None,
                REG_DWORD,
                Some(&1u32.to_le_bytes()),
            )
        };
        let _ = unsafe { RegCloseKey(hkey) };
        err == ERROR_SUCCESS
    }

    fn ensure_dev_mode() -> Result<(), String> {
        if dev_mode_enabled() {
            return Ok(());
        }
        println!("Developer Mode is required for unsigned package registration.");
        if unsafe { IsUserAnAdmin() }.as_bool() {
            if enable_dev_mode() {
                println!("  enabled Developer Mode.");
                return Ok(());
            }
        } else {
            println!("  requesting elevation to enable it (UAC prompt)...");
            if run_self_elevated("enable-devmode") == Some(0) && dev_mode_enabled() {
                println!("  enabled Developer Mode.");
                return Ok(());
            }
        }
        Err("could not enable Developer Mode. Enable it manually: Settings > System > For developers > Developer Mode, then re-run.".into())
    }

    /// Re-run this exe elevated with `args`; returns its exit code (None if
    /// launch failed, e.g. the UAC prompt was declined).
    fn run_self_elevated(args: &str) -> Option<u32> {
        let exe = std::env::current_exe().ok()?;
        let exe_w = wide(&exe.to_string_lossy());
        let verb_w = wide("runas");
        let args_w = wide(args);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: PCWSTR(verb_w.as_ptr()),
            lpFile: PCWSTR(exe_w.as_ptr()),
            lpParameters: PCWSTR(args_w.as_ptr()),
            nShow: 0, // SW_HIDE: the elevated child is console-less work
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }.ok()?;
        if info.hProcess == HANDLE::default() {
            return None;
        }
        let mut code = 1u32;
        unsafe {
            let _ = WaitForSingleObject(info.hProcess, INFINITE);
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
        }
        Some(code)
    }
}
