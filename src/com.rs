//! COM implementation: IExplorerCommand root (one per registered package/CLSID),
//! sub-commands, enumerator, and class factory.
//!
//! - exactly one matching entry  -> rendered directly (its own title/icon, no flyout)
//! - multiple matching entries   -> one flyout using the package title
//! - hidden in the classic menu (except the left tree view), because the installer
//!   registers separate classic-menu entries via cmrsRun.exe.
#![cfg(windows)]

use crate::config::{load_configs, FileType, MenuConfig, PathVars};
use crate::{config_dir, debug_log, MappingEntry};
use std::sync::RwLock;
use windows::core::{implement, Interface, Ref, Result, GUID, PWSTR};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_NOTIMPL, E_POINTER, HWND, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    IBindCtx, IClassFactory, IClassFactory_Impl, IServiceProvider, Urlmon::E_PENDING,
};
use windows::Win32::System::Ole::{IObjectWithSite, IObjectWithSite_Impl, IOleWindow};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER, SFGAO_STREAM};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, IEnumExplorerCommand, IEnumExplorerCommand_Impl, IExplorerCommand,
    IExplorerCommand_Impl, IFolderView, IShellItem, IShellItemArray, SHGetKnownFolderPath,
    ECF_DEFAULT, ECF_HASSUBCOMMANDS, ECS_ENABLED, ECS_HIDDEN, KNOWN_FOLDER_FLAG, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

// ---------------------------------------------------------------- helpers

/// Allocate a CoTaskMem PWSTR copy of `s` (caller/COM consumer frees it).
fn co_pwstr(s: &str) -> Result<PWSTR> {
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let bytes = wide.len() * 2;
        let mem = windows::Win32::System::Com::CoTaskMemAlloc(bytes) as *mut u16;
        if mem.is_null() {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_OUTOFMEMORY,
            ));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), mem, wide.len());
        Ok(PWSTR(mem))
    }
}

fn apps_use_dark_theme() -> bool {
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut data: u32 = 1;
    let mut size: u32 = 4;
    unsafe {
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(subkey.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        );
        if err.is_err() {
            return false;
        }
    }
    data == 0
}

fn item_path(item: &IShellItem) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        if p.is_null() {
            return None;
        }
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn is_directory(item: &IShellItem) -> bool {
    unsafe {
        match item.GetAttributes(SFGAO_FILESYSTEM | SFGAO_FOLDER | SFGAO_STREAM) {
            Ok(attrs) => {
                let a = attrs.0;
                (a & SFGAO_FILESYSTEM.0) != 0
                    && (a & SFGAO_FOLDER.0) != 0
                    && (a & SFGAO_STREAM.0) == 0
            }
            Err(_) => false,
        }
    }
}

fn is_drive_root(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() == 2 || b.len() == 3)
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b.len() == 2 || b[2] == b'\\')
}

fn desktop_path() -> Option<String> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Classify a single selected item / background folder.
fn classify(path: &str, is_dir: bool, is_background: bool) -> FileType {
    if !is_dir {
        return FileType::File;
    }
    if is_background {
        if let Some(desk) = desktop_path() {
            if desk.eq_ignore_ascii_case(path) {
                return FileType::Desktop;
            }
        }
        return FileType::Background;
    }
    if is_drive_root(path) {
        FileType::Drive
    } else {
        FileType::Directory
    }
}

fn name_and_ext(path: &str, file_type: FileType) -> (String, String) {
    let vars = PathVars::from_path(path);
    match file_type {
        FileType::File => (vars.name.clone(), vars.ext_lower()),
        _ => (vars.name.clone(), String::new()),
    }
}

// ---------------------------------------------------------------- sub command

#[implement(IExplorerCommand, Agile = false)]
struct SubCommand {
    cfg: MenuConfig,
    title: String,
    paths: Vec<String>,
    dark: bool,
}

impl SubCommand {
    fn icon_of(cfg: &MenuConfig, dark: bool) -> String {
        // Copied/shield-badged icons are pre-generated by ctxmenu-setup; only a
        // path lookup happens here (this code can run inside Explorer).
        if cfg.uses_generated_icon() {
            let slug = crate::setup::slug(&cfg.source_file);
            if dark && !cfg.icon_dark.is_empty() {
                let p = crate::icons_dir().join(crate::setup::generated_icon_name(&slug, true));
                if p.is_file() {
                    return p.to_string_lossy().into_owned();
                }
            }
            let p = crate::icons_dir().join(crate::setup::generated_icon_name(&slug, false));
            if p.is_file() {
                return p.to_string_lossy().into_owned();
            }
        }
        if dark && !cfg.icon_dark.is_empty() {
            crate::exec::expand_env(cfg.resolve_icon_spec(&cfg.icon_dark))
        } else {
            crate::exec::expand_env(cfg.resolve_icon_spec(&cfg.icon))
        }
    }
}

impl IExplorerCommand_Impl for SubCommand_Impl {
    fn GetTitle(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        co_pwstr(&self.title)
    }

    fn GetIcon(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        let icon = SubCommand::icon_of(&self.cfg, self.dark);
        if icon.is_empty() {
            // Never S_OK + null: the classic menu hosts this in-proc in Explorer,
            // and shell32 dereferences the string without checking (crash).
            Err(windows::core::Error::from_hresult(E_NOTIMPL))
        } else {
            co_pwstr(&icon)
        }
    }

    fn GetToolTip(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        Err(windows::core::Error::from_hresult(E_NOTIMPL))
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(GUID::zeroed())
    }

    fn GetState(
        &self,
        _items: Ref<'_, IShellItemArray>,
        _slow: windows::core::BOOL,
    ) -> Result<u32> {
        Ok(ECS_ENABLED.0 as u32)
    }

    fn Invoke(&self, _items: Ref<'_, IShellItemArray>, _bc: Ref<'_, IBindCtx>) -> Result<()> {
        crate::exec::run(&self.cfg, &self.paths);
        Ok(())
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(windows::core::Error::from_hresult(E_NOTIMPL))
    }
}

// ---------------------------------------------------------------- enumerator

#[implement(IEnumExplorerCommand, Agile = false)]
struct CommandEnum {
    items: Vec<IExplorerCommand>,
    pos: RwLock<usize>,
}

impl IEnumExplorerCommand_Impl for CommandEnum_Impl {
    fn Next(
        &self,
        celt: u32,
        puicommand: *mut Option<IExplorerCommand>,
        pceltfetched: *mut u32,
    ) -> windows::core::HRESULT {
        if puicommand.is_null() {
            return E_POINTER;
        }
        let mut pos = self.pos.write().unwrap();
        let mut fetched: u32 = 0;
        while fetched < celt && *pos < self.items.len() {
            unsafe {
                std::ptr::write(
                    puicommand.add(fetched as usize),
                    Some(self.items[*pos].clone()),
                );
            }
            *pos += 1;
            fetched += 1;
        }
        if !pceltfetched.is_null() {
            unsafe { *pceltfetched = fetched };
        }
        if fetched == celt {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, celt: u32) -> Result<()> {
        let mut pos = self.pos.write().unwrap();
        *pos = (*pos + celt as usize).min(self.items.len());
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        *self.pos.write().unwrap() = 0;
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumExplorerCommand> {
        let e: IEnumExplorerCommand = CommandEnum {
            items: self.items.clone(),
            pos: RwLock::new(*self.pos.read().unwrap()),
        }
        .into();
        Ok(e)
    }
}

// ---------------------------------------------------------------- root command

#[derive(Default)]
struct RootState {
    matched: Vec<MenuConfig>,
    paths: Vec<String>,
    /// Per selected item: lowercase extension with dot, empty for folders.
    exts: Vec<String>,
    dark: bool,
}

#[implement(IExplorerCommand, IObjectWithSite, Agile = false)]
pub struct RootCommand {
    title: String,
    files: Option<Vec<String>>,
    state: RwLock<RootState>,
    site: RwLock<Option<windows::core::IUnknown>>,
}

impl RootCommand {
    pub fn new(entry: &MappingEntry) -> Self {
        Self {
            title: entry.title.clone(),
            files: entry.files.clone(),
            state: RwLock::new(RootState::default()),
            site: RwLock::new(None),
        }
    }
}

impl RootCommand_Impl {
    /// True if we're being shown in the classic ("Show more options") menu,
    /// except for Explorer's left tree view.
    fn in_classic_menu(&self) -> bool {
        let site = self.site.read().unwrap();
        let Some(site) = site.as_ref() else {
            return false;
        };
        let Ok(ole) = site.cast::<IOleWindow>() else {
            return false;
        };
        let hwnd: HWND = match unsafe { ole.GetWindow() } {
            Ok(h) => h,
            Err(_) => return false,
        };
        let mut buf = [0u16; 260];
        let n = unsafe { GetClassNameW(hwnd, &mut buf) };
        if n <= 0 {
            return true;
        }
        let class = String::from_utf16_lossy(&buf[..n as usize]);
        class != "NamespaceTreeControl"
    }

    /// Background right-click: resolve the current folder from the site.
    fn folder_from_site(&self) -> Option<String> {
        let site = self.site.read().unwrap();
        let site = site.as_ref()?;
        let sp: IServiceProvider = site.cast().ok()?;
        unsafe {
            let view: IFolderView = sp.QueryService(&IFolderView::IID).ok()?;
            let folder: IShellItem = view.GetFolder().ok()?;
            let path = item_path(&folder)?;
            // Reject non-filesystem/virtual locations ("This PC", zip contents, ...).
            if is_directory(&folder) {
                Some(path)
            } else {
                None
            }
        }
    }

    /// Populate state from the current selection; returns whether anything matched.
    fn compute(&self, items: Option<&IShellItemArray>) -> bool {
        let mut paths: Vec<String> = Vec::new();
        let mut types: Vec<FileType> = Vec::new();

        let count = items
            .and_then(|a| unsafe { a.GetCount().ok() })
            .unwrap_or(0);

        if count == 0 {
            // Folder background or desktop.
            let Some(path) = self.folder_from_site() else {
                return false;
            };
            types.push(classify(&path, true, true));
            paths.push(path);
        } else {
            let array = items.unwrap();
            for i in 0..count {
                let Ok(item) = (unsafe { array.GetItemAt(i) }) else {
                    continue;
                };
                let Some(path) = item_path(&item) else {
                    continue;
                };
                types.push(classify(&path, is_directory(&item), false));
                paths.push(path);
            }
        }

        if paths.is_empty() {
            return false;
        }

        let configs = load_configs(&config_dir(), self.files.as_deref());
        let multiple = paths.len() > 1;
        let names_and_exts: Vec<(String, String)> = paths
            .iter()
            .zip(&types)
            .map(|(p, t)| name_and_ext(p, *t))
            .collect();

        let matched: Vec<MenuConfig> = configs
            .into_iter()
            .filter(|cfg| {
                if multiple && !cfg.accepts_multiple() {
                    return false;
                }
                // Every selected item must be acceptable.
                types
                    .iter()
                    .zip(&names_and_exts)
                    .all(|(t, (name, ext))| cfg.accepts(*t, name, ext))
            })
            .collect();

        debug_log(&format!(
            "GetState: {} path(s), {} matched",
            paths.len(),
            matched.len()
        ));

        let any = !matched.is_empty();
        *self.state.write().unwrap() = RootState {
            matched,
            paths,
            exts: names_and_exts.into_iter().map(|(_, ext)| ext).collect(),
            dark: apps_use_dark_theme(),
        };
        any
    }
}

impl IExplorerCommand_Impl for RootCommand_Impl {
    fn GetTitle(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        let state = self.state.read().unwrap();
        if state.matched.len() == 1 {
            co_pwstr(state.matched[0].title_for(&state.exts))
        } else {
            co_pwstr(&self.title)
        }
    }

    fn GetIcon(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        let state = self.state.read().unwrap();
        if state.matched.len() == 1 {
            let icon = SubCommand::icon_of(&state.matched[0], state.dark);
            if !icon.is_empty() {
                return co_pwstr(&icon);
            }
        }
        // Never S_OK + null (see SubCommand::GetIcon).
        Err(windows::core::Error::from_hresult(E_NOTIMPL))
    }

    fn GetToolTip(&self, _items: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        Err(windows::core::Error::from_hresult(E_NOTIMPL))
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(GUID::zeroed())
    }

    fn GetState(
        &self,
        items: Ref<'_, IShellItemArray>,
        oktobeslow: windows::core::BOOL,
    ) -> Result<u32> {
        if !oktobeslow.as_bool() {
            return Err(windows::core::Error::from_hresult(E_PENDING));
        }
        if self.in_classic_menu() {
            return Ok(ECS_HIDDEN.0 as u32);
        }
        if self.compute(items.as_ref()) {
            Ok(ECS_ENABLED.0 as u32)
        } else {
            Ok(ECS_HIDDEN.0 as u32)
        }
    }

    fn Invoke(&self, _items: Ref<'_, IShellItemArray>, _bc: Ref<'_, IBindCtx>) -> Result<()> {
        let state = self.state.read().unwrap();
        if state.matched.len() == 1 {
            crate::exec::run(&state.matched[0], &state.paths);
        }
        Ok(())
    }

    fn GetFlags(&self) -> Result<u32> {
        let state = self.state.read().unwrap();
        if state.matched.len() > 1 {
            Ok(ECF_HASSUBCOMMANDS.0 as u32)
        } else {
            Ok(ECF_DEFAULT.0 as u32)
        }
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        let state = self.state.read().unwrap();
        if state.matched.len() <= 1 {
            return Err(windows::core::Error::from_hresult(E_NOTIMPL));
        }
        let items: Vec<IExplorerCommand> = state
            .matched
            .iter()
            .map(|cfg| {
                SubCommand {
                    cfg: cfg.clone(),
                    title: cfg.title_for(&state.exts).to_string(),
                    paths: state.paths.clone(),
                    dark: state.dark,
                }
                .into()
            })
            .collect();
        let e: IEnumExplorerCommand = CommandEnum {
            items,
            pos: RwLock::new(0),
        }
        .into();
        Ok(e)
    }
}

impl IObjectWithSite_Impl for RootCommand_Impl {
    fn SetSite(&self, punksite: Ref<'_, windows::core::IUnknown>) -> Result<()> {
        *self.site.write().unwrap() = punksite.as_ref().cloned();
        Ok(())
    }

    fn GetSite(&self, riid: *const GUID, ppvsite: *mut *mut core::ffi::c_void) -> Result<()> {
        if riid.is_null() || ppvsite.is_null() {
            return Err(windows::core::Error::from_hresult(E_POINTER));
        }
        let site = self.site.read().unwrap();
        match site.as_ref() {
            Some(s) => unsafe { s.query(riid, ppvsite).ok() },
            None => Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            )),
        }
    }
}

// ---------------------------------------------------------------- class factory

#[implement(IClassFactory, Agile = false)]
pub struct ClassFactory {
    pub entry: MappingEntry,
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, windows::core::IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if punkouter.as_ref().is_some() {
            return Err(windows::core::Error::from_hresult(CLASS_E_NOAGGREGATION));
        }
        let cmd: IExplorerCommand = RootCommand::new(&self.entry).into();
        unsafe { cmd.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, _flock: windows::core::BOOL) -> Result<()> {
        Ok(())
    }
}

/// Build the class factory as an IClassFactory interface.
pub fn make_factory(entry: MappingEntry) -> IClassFactory {
    ClassFactory { entry }.into()
}
