//! Menu configuration: parsing and matching for one context-menu entry
//! (documented in README.md and menu.schema.json). Numeric flag values and the
//! deprecated accept* booleans must keep parsing so configs written for
//! ContextMenuForWindows11 continue to work.
//!
//! This module is pure std + serde + regex, so `cargo test` works on any platform.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::path::Path; // used by load_configs (directory I/O only, not path parsing)

// acceptDirectoryFlag bits
pub const DIR_DIRECTORY: u32 = 1;
pub const DIR_BACKGROUND: u32 = 2;
pub const DIR_DESKTOP: u32 = 4;
pub const DIR_DRIVE: u32 = 8;

// acceptFileFlag values
pub const FILE_NONE: u32 = 0;
pub const FILE_EXT: u32 = 1; // fuzzy extension match ("contains")
pub const FILE_REGEX: u32 = 2; // regex on file name
pub const FILE_EXT_LIST: u32 = 3; // exact list, '|' separated
pub const FILE_ALL: u32 = 4;

// acceptMultipleFilesFlag values
pub const MULTI_NONE: u32 = 0;
pub const MULTI_EACH: u32 = 1;
pub const MULTI_JOIN: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Background,
    Desktop,
    Drive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Lowercased, separators stripped: "top-left" / "TOP_LEFT" -> "topleft".
fn norm_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

impl Corner {
    /// Tolerant of case and separators: "topLeft", "top-left", "TOP_LEFT", ...
    pub fn parse(s: &str) -> Corner {
        match norm_key(s).as_str() {
            "topleft" => Corner::TopLeft,
            "topright" => Corner::TopRight,
            "bottomleft" => Corner::BottomLeft,
            _ => Corner::BottomRight,
        }
    }
}

// Readable flag values; numeric forms stay supported for compatibility.

fn dir_bits(s: &str) -> Result<u32, String> {
    Ok(match norm_key(s).as_str() {
        "directory" | "dir" | "folder" => DIR_DIRECTORY,
        "background" | "directorybackground" | "folderbackground" => DIR_BACKGROUND,
        "desktop" => DIR_DESKTOP,
        "drive" => DIR_DRIVE,
        "all" => DIR_DIRECTORY | DIR_BACKGROUND | DIR_DESKTOP | DIR_DRIVE,
        "none" => 0,
        other => return Err(format!("unknown acceptDirectoryFlag value: {other:?}")),
    })
}

fn de_dir_flag<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        Num(u32),
        One(String),
        Many(Vec<String>),
    }
    let bits = match V::deserialize(d)? {
        V::Num(n) => n,
        V::One(s) => dir_bits(&s).map_err(D::Error::custom)?,
        V::Many(v) => {
            let mut acc = 0;
            for s in &v {
                acc |= dir_bits(s).map_err(D::Error::custom)?;
            }
            acc
        }
    };
    Ok(Some(bits))
}

fn de_file_flag<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        Num(u32),
        Name(String),
    }
    Ok(Some(match V::deserialize(d)? {
        V::Num(n) => n,
        V::Name(s) => match norm_key(&s).as_str() {
            "none" => FILE_NONE,
            "extension" | "ext" | "exts" => FILE_EXT,
            "regex" => FILE_REGEX,
            "extensionlist" | "extlist" | "list" => FILE_EXT_LIST,
            "all" => FILE_ALL,
            other => {
                return Err(D::Error::custom(format!(
                    "unknown acceptFileFlag value: {other:?}"
                )))
            }
        },
    }))
}

fn de_multi_flag<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        Num(u32),
        Name(String),
    }
    Ok(match V::deserialize(d)? {
        V::Num(n) => n,
        V::Name(s) => match norm_key(&s).as_str() {
            "none" => MULTI_NONE,
            "each" => MULTI_EACH,
            "join" => MULTI_JOIN,
            other => {
                return Err(D::Error::custom(format!(
                    "unknown acceptMultipleFilesFlag value: {other:?}"
                )))
            }
        },
    })
}

fn de_show_window<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum V {
        Num(i32),
        Name(String),
    }
    Ok(match V::deserialize(d)? {
        V::Num(n) => n,
        V::Name(s) => match norm_key(&s).as_str() {
            "hide" | "hidden" => -1,
            "normal" | "show" => 0,
            "minimized" | "min" => 1,
            "maximized" | "max" => 2,
            other => {
                return Err(D::Error::custom(format!(
                    "unknown showWindowFlag value: {other:?}"
                )))
            }
        },
    })
}

/// `smallIcon`: badge composited onto the entry's icon.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SmallIcon {
    /// Icon spec ("path,index"), "uac" for the system UAC shield, or "exe"
    /// for the entry's own exe.
    #[serde(default)]
    pub icon: String,
    /// Corner, default bottomRight.
    #[serde(default)]
    pub location: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MenuConfig {
    pub title: String,
    pub exe: String,
    #[serde(default)]
    pub param: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default, rename = "iconDark")]
    pub icon_dark: String,
    #[serde(default, rename = "acceptDirectory")]
    pub accept_directory: Option<bool>,
    #[serde(default, rename = "acceptDirectoryFlag", deserialize_with = "de_dir_flag")]
    pub accept_directory_flag: Option<u32>,
    #[serde(default, rename = "acceptFile")]
    pub accept_file: Option<bool>,
    #[serde(default, rename = "acceptFileFlag", deserialize_with = "de_file_flag")]
    pub accept_file_flag: Option<u32>,
    #[serde(default, rename = "acceptExts")]
    pub accept_exts: String,
    #[serde(default, rename = "acceptFileRegex")]
    pub accept_file_regex: String,
    #[serde(default, rename = "acceptMultipleFilesFlag", deserialize_with = "de_multi_flag")]
    pub accept_multiple_files_flag: u32,
    #[serde(default, rename = "pathDelimiter")]
    pub path_delimiter: String,
    #[serde(default, rename = "paramForMultipleFiles")]
    pub param_for_multiple_files: String,
    #[serde(default)]
    pub index: i32,
    #[serde(default, rename = "showWindowFlag", deserialize_with = "de_show_window")]
    pub show_window_flag: i32,
    #[serde(default, rename = "workingDirectory")]
    pub working_directory: String,
    /// Run the command elevated (UAC prompt).
    #[serde(default, rename = "runAsAdmin")]
    pub run_as_admin: bool,
    /// Snapshot the icon into a local .ico at sync time, so the entry
    /// survives the source path changing (e.g. Store apps on update).
    #[serde(default, rename = "copyIcon")]
    pub copy_icon: bool,
    /// Corner badge for the entry's icon.
    #[serde(default, rename = "smallIcon")]
    pub small_icon: Option<SmallIcon>,
    /// The config file name this entry was loaded from (not part of JSON).
    #[serde(skip)]
    pub source_file: String,
}

impl MenuConfig {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Effective directory-matching bitmask.
    /// Priority: acceptDirectoryFlag > deprecated acceptDirectory bool > schema default (15).
    pub fn dir_flag(&self) -> u32 {
        if let Some(f) = self.accept_directory_flag {
            return f & 15;
        }
        match self.accept_directory {
            Some(true) => 15,
            Some(false) => 0,
            None => 15,
        }
    }

    /// Effective file-matching mode.
    /// Priority: acceptFile==false > acceptFileFlag > acceptExts heuristic > schema default (All).
    pub fn file_flag(&self) -> u32 {
        if self.accept_file == Some(false) {
            return FILE_NONE;
        }
        if let Some(f) = self.accept_file_flag {
            return f.min(4);
        }
        // Compat heuristic: acceptExts set without a flag means fuzzy ext matching.
        if !self.accept_exts.trim().is_empty() && self.accept_exts.trim() != "*" {
            return FILE_EXT;
        }
        FILE_ALL
    }

    /// Does this entry match a single item of `file_type` with file `name` / `ext`
    /// (ext lowercase, with leading dot, empty for folders or dotfiles)?
    pub fn accepts(&self, file_type: FileType, name: &str, ext: &str) -> bool {
        match file_type {
            FileType::Directory => self.dir_flag() & DIR_DIRECTORY != 0,
            FileType::Background => self.dir_flag() & DIR_BACKGROUND != 0,
            FileType::Desktop => self.dir_flag() & DIR_DESKTOP != 0,
            FileType::Drive => self.dir_flag() & DIR_DRIVE != 0,
            FileType::File => match self.file_flag() {
                FILE_NONE => false,
                FILE_ALL => true,
                FILE_EXT => {
                    let exts = self.accept_exts.trim().to_lowercase();
                    exts.is_empty() || exts == "*" || (!ext.is_empty() && exts.contains(ext))
                }
                FILE_REGEX => match regex::Regex::new(&self.accept_file_regex) {
                    Ok(re) => re.is_match(name),
                    Err(_) => false,
                },
                FILE_EXT_LIST => self
                    .accept_exts
                    .to_lowercase()
                    .split('|')
                    .any(|e| e.trim() == ext),
                _ => false,
            },
        }
    }

    /// Effective badge icon spec: from `smallIcon`, if set. None = no badge.
    pub fn badge_spec(&self) -> Option<&str> {
        let si = self.small_icon.as_ref()?;
        let s = si.icon.trim();
        if s.is_empty() {
            return None;
        }
        Some(s)
    }

    pub fn badge_corner(&self) -> Corner {
        Corner::parse(self.small_icon.as_ref().map_or("", |s| s.location.as_str()))
    }

    /// Resolves the "exe" icon-spec shorthand (this entry's own exe) to the
    /// `exe` field. Any other spec, including empty, passes through unchanged.
    pub fn resolve_icon_spec<'a>(&'a self, spec: &'a str) -> &'a str {
        if spec.trim().eq_ignore_ascii_case("exe") {
            self.exe.trim().trim_matches('"')
        } else {
            spec
        }
    }

    /// Whether the menu icon comes from a generated file in the icons dir
    /// (badged and/or locally copied) instead of the raw `icon` spec.
    pub fn uses_generated_icon(&self) -> bool {
        self.badge_spec().is_some() || self.copy_icon
    }

    /// Multi-selection support (when >1 items are selected).
    pub fn accepts_multiple(&self) -> bool {
        self.accept_multiple_files_flag == MULTI_EACH
            || self.accept_multiple_files_flag == MULTI_JOIN
    }

    /// Effective delimiter for JOIN mode (schema default: a space).
    pub fn delimiter(&self) -> &str {
        if self.path_delimiter.is_empty() {
            " "
        } else {
            &self.path_delimiter
        }
    }
}

/// Derived path components for substitution variables.
pub struct PathVars {
    pub path: String,
    pub parent: String,
    pub name: String,
    pub name_no_ext: String,
    pub extension: String,
}

impl PathVars {
    pub fn from_path(p: &str) -> Self {
        // Manual splitting (not std::path) so behavior is identical on every
        // platform — required for Windows-style paths in cross-platform tests.
        let trimmed = p.trim_end_matches(['\\', '/']);
        let (parent, name) = match trimmed.rfind(['\\', '/']) {
            Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
            None => ("", trimmed),
        };
        // Direct children of a drive root: parent "C:" -> "C:\"
        let parent = if parent.ends_with(':') {
            format!("{parent}\\")
        } else {
            parent.to_string()
        };
        let name = name.to_string();
        // Dotfiles (".gitignore") have no extension.
        let (name_no_ext, extension) = if name.starts_with('.') {
            (name.clone(), String::new())
        } else {
            match name.rfind('.') {
                Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
                _ => (name.clone(), String::new()),
            }
        };
        Self {
            path: p.to_string(),
            parent,
            name,
            name_no_ext,
            extension,
        }
    }

    pub fn ext_lower(&self) -> String {
        self.extension.to_lowercase()
    }
}

/// Substitute template variables for a single path.
/// `first` supplies {path0}/{name0}/{nameNoExt0}/{extension0} (first selected item).
pub fn substitute(template: &str, vars: &PathVars, first: &PathVars) -> String {
    template
        .replace("{path}", &vars.path)
        .replace("{parent}", &vars.parent)
        .replace("{nameNoExt}", &vars.name_no_ext)
        .replace("{name}", &vars.name)
        .replace("{extension}", &vars.extension)
        .replace("{path0}", &first.path)
        .replace("{nameNoExt0}", &first.name_no_ext)
        .replace("{name0}", &first.name)
        .replace("{extension0}", &first.extension)
}

/// JOIN mode: every path quoted, joined with the delimiter, then substituted as {path}.
pub fn join_paths(paths: &[String], delimiter: &str) -> String {
    paths
        .iter()
        .map(|p| format!("\"{p}\""))
        .collect::<Vec<_>>()
        .join(delimiter)
}

/// Build the final parameter string for an invocation.
/// For JOIN mode, `paths` has all items; otherwise exactly one.
pub fn build_param(cfg: &MenuConfig, paths: &[String]) -> String {
    if paths.is_empty() {
        return cfg.param.clone();
    }
    let first = PathVars::from_path(&paths[0]);
    if paths.len() > 1 && cfg.accept_multiple_files_flag == MULTI_JOIN {
        let template = if cfg.param_for_multiple_files.is_empty() {
            &cfg.param
        } else {
            &cfg.param_for_multiple_files
        };
        let joined = join_paths(paths, cfg.delimiter());
        // {path} receives the pre-quoted joined list; other vars come from the first item.
        let mut joined_vars = PathVars::from_path(&paths[0]);
        joined_vars.path = joined;
        substitute(template, &joined_vars, &first)
    } else {
        substitute(&cfg.param, &first, &first)
    }
}

/// Load every *.json in a directory, sorted by `index` then file name.
pub fn load_configs(dir: &Path, only_files: Option<&[String]>) -> Vec<MenuConfig> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map(|e| e.eq_ignore_ascii_case("json")) == Some(true)
                && p.is_file()
        })
        .collect();
    files.sort();
    for f in files {
        let fname = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(filter) = only_files {
            if !filter.iter().any(|x| x.eq_ignore_ascii_case(&fname)) {
                continue;
            }
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        // Tolerate a UTF-8 BOM.
        let text = text.trim_start_matches('\u{feff}');
        if let Ok(mut cfg) = MenuConfig::parse(text) {
            cfg.source_file = fname;
            out.push(cfg);
        }
    }
    out.sort_by(|a, b| {
        a.index
            .cmp(&b.index)
            .then_with(|| a.source_file.cmp(&b.source_file))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "title": "Open With VScode",
        "index": 21,
        "exe": "\"%LocalAppData%\\Programs\\Microsoft VS Code\\Code.exe\"",
        "param": "\"{path}\"",
        "icon": "\"%LocalAppData%\\Programs\\Microsoft VS Code\\Code.exe\",0",
        "acceptDirectoryFlag": 3,
        "acceptFileFlag": 4,
        "acceptExts": "*",
        "acceptFileRegex": "",
        "acceptMultipleFilesFlag": 1,
        "pathDelimiter": "",
        "paramForMultipleFiles": "",
        "showWindowFlag": 0
    }"#;

    #[test]
    fn parses_numeric_flags_sample() {
        let c = MenuConfig::parse(SAMPLE).unwrap();
        assert_eq!(c.title, "Open With VScode");
        assert_eq!(c.index, 21);
        assert_eq!(c.dir_flag(), 3);
        assert_eq!(c.file_flag(), FILE_ALL);
        assert!(c.accepts(FileType::Directory, "", ""));
        assert!(c.accepts(FileType::Background, "", ""));
        assert!(!c.accepts(FileType::Desktop, "", ""));
        assert!(!c.accepts(FileType::Drive, "", ""));
        assert!(c.accepts(FileType::File, "a.txt", ".txt"));
        assert!(!c.run_as_admin);
    }

    #[test]
    fn run_as_admin_extension() {
        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"wt.exe","runAsAdmin":true,"smallIcon":{"icon":"uac"},"acceptDirectoryFlag":15,"acceptFileFlag":0}"#,
        )
        .unwrap();
        assert!(c.run_as_admin);
        assert!(c.uses_generated_icon());
        assert!(!c.accepts(FileType::File, "a.txt", ".txt"));
        assert!(c.accepts(FileType::Drive, "", ""));
    }

    #[test]
    fn generated_icon_rules() {
        let c = MenuConfig::parse(r#"{"title":"T","exe":"x","copyIcon":true}"#).unwrap();
        assert!(c.copy_icon);
        assert!(c.uses_generated_icon());
        assert!(c.badge_spec().is_none());
        let c = MenuConfig::parse(r#"{"title":"T","exe":"x"}"#).unwrap();
        assert!(!c.uses_generated_icon());
    }

    #[test]
    fn small_icon_badge() {
        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"x","smallIcon":{"icon":"C:\\b.ico","location":"top-left"}}"#,
        )
        .unwrap();
        assert_eq!(c.badge_spec(), Some("C:\\b.ico"));
        assert_eq!(c.badge_corner(), Corner::TopLeft);
        assert!(c.uses_generated_icon());

        // location is optional and defaults to bottom right
        let c =
            MenuConfig::parse(r#"{"title":"T","exe":"x","smallIcon":{"icon":"uac"}}"#).unwrap();
        assert_eq!(c.badge_spec(), Some("uac"));
        assert_eq!(c.badge_corner(), Corner::BottomRight);

        // "exe" is a valid badge spec too, resolved by resolve_icon_spec
        let c =
            MenuConfig::parse(r#"{"title":"T","exe":"x","smallIcon":{"icon":"exe"}}"#).unwrap();
        assert_eq!(c.badge_spec(), Some("exe"));

        // empty smallIcon object means no badge
        let c = MenuConfig::parse(r#"{"title":"T","exe":"x","smallIcon":{}}"#).unwrap();
        assert_eq!(c.badge_spec(), None);
    }

    #[test]
    fn exe_icon_token() {
        let c = MenuConfig::parse(r#"{"title":"T","exe":" \"C:\\a b\\app.exe\" "}"#).unwrap();
        assert_eq!(c.resolve_icon_spec("exe"), r"C:\a b\app.exe");
        assert_eq!(c.resolve_icon_spec("EXE"), r"C:\a b\app.exe");
        assert_eq!(c.resolve_icon_spec("  exe  "), r"C:\a b\app.exe");
        // anything else, including empty, passes through unchanged.
        assert_eq!(c.resolve_icon_spec("C:\\i.ico,2"), "C:\\i.ico,2");
        assert_eq!(c.resolve_icon_spec(""), "");
    }

    #[test]
    fn readable_flags() {
        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"x",
                "acceptDirectoryFlag":["directory","drive"],
                "acceptFileFlag":"extensionList",
                "acceptExts":".apk|.apkx",
                "acceptMultipleFilesFlag":"join",
                "showWindowFlag":"hide"}"#,
        )
        .unwrap();
        assert_eq!(c.dir_flag(), DIR_DIRECTORY | DIR_DRIVE);
        assert_eq!(c.file_flag(), FILE_EXT_LIST);
        assert_eq!(c.accept_multiple_files_flag, MULTI_JOIN);
        assert_eq!(c.show_window_flag, -1);

        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"x","acceptDirectoryFlag":"all","acceptFileFlag":"none",
                "acceptMultipleFilesFlag":"each","showWindowFlag":"maximized"}"#,
        )
        .unwrap();
        assert_eq!(c.dir_flag(), 15);
        assert_eq!(c.file_flag(), FILE_NONE);
        assert_eq!(c.accept_multiple_files_flag, MULTI_EACH);
        assert_eq!(c.show_window_flag, 2);

        // tolerant of case/separators; unknown names are an error
        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"x","acceptDirectoryFlag":["Folder-Background"],"showWindowFlag":"MIN"}"#,
        )
        .unwrap();
        assert_eq!(c.dir_flag(), DIR_BACKGROUND);
        assert_eq!(c.show_window_flag, 1);
        assert!(
            MenuConfig::parse(r#"{"title":"T","exe":"x","acceptDirectoryFlag":"everything"}"#)
                .is_err()
        );

        // numeric values keep working exactly as before
        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"x","acceptDirectoryFlag":3,"acceptFileFlag":4,
                "acceptMultipleFilesFlag":2,"showWindowFlag":-1}"#,
        )
        .unwrap();
        assert_eq!(c.dir_flag(), 3);
        assert_eq!(c.file_flag(), FILE_ALL);
        assert_eq!(c.accept_multiple_files_flag, MULTI_JOIN);
        assert_eq!(c.show_window_flag, -1);
    }

    #[test]
    fn corner_parsing() {
        assert_eq!(Corner::parse("topLeft"), Corner::TopLeft);
        assert_eq!(Corner::parse("TOP_RIGHT"), Corner::TopRight);
        assert_eq!(Corner::parse("bottom-left"), Corner::BottomLeft);
        assert_eq!(Corner::parse("bottomright"), Corner::BottomRight);
        assert_eq!(Corner::parse(""), Corner::BottomRight);
        assert_eq!(Corner::parse("garbage"), Corner::BottomRight);
    }

    #[test]
    fn unknown_fields_ignored() {
        let c = MenuConfig::parse(r#"{"title":"T","exe":"x","futureField":123}"#).unwrap();
        assert_eq!(c.exe, "x");
    }

    #[test]
    fn deprecated_accept_directory() {
        let c = MenuConfig::parse(r#"{"title":"T","exe":"x","acceptDirectory":false}"#).unwrap();
        assert_eq!(c.dir_flag(), 0);
        let c = MenuConfig::parse(r#"{"title":"T","exe":"x","acceptDirectory":true}"#).unwrap();
        assert_eq!(c.dir_flag(), 15);
    }

    #[test]
    fn ext_list_matching() {
        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"x","acceptFileFlag":3,"acceptExts":".apkx|.apk"}"#,
        )
        .unwrap();
        assert!(c.accepts(FileType::File, "a.apk", ".apk"));
        assert!(c.accepts(FileType::File, "b.APKX", ".apkx"));
        assert!(!c.accepts(FileType::File, "c.txt", ".txt"));
    }

    #[test]
    fn regex_matching() {
        let c = MenuConfig::parse(
            r#"{"title":"T","exe":"x","acceptFileFlag":2,"acceptFileRegex":".+?\\.txt"}"#,
        )
        .unwrap();
        assert!(c.accepts(FileType::File, "note.txt", ".txt"));
        assert!(!c.accepts(FileType::File, "note.md", ".md"));
    }

    #[test]
    fn exts_heuristic_without_flag() {
        let c = MenuConfig::parse(r#"{"title":"T","exe":"x","acceptExts":".apkx .mapk"}"#).unwrap();
        assert_eq!(c.file_flag(), FILE_EXT);
        assert!(c.accepts(FileType::File, "a.apkx", ".apkx"));
        assert!(!c.accepts(FileType::File, "a.zip", ".zip"));
    }

    #[test]
    fn path_vars() {
        let v = PathVars::from_path(r"C:\test\MpDlpCmd.exe");
        assert_eq!(v.path, r"C:\test\MpDlpCmd.exe");
        assert_eq!(v.parent, r"C:\test");
        assert_eq!(v.name, "MpDlpCmd.exe");
        assert_eq!(v.name_no_ext, "MpDlpCmd");
        assert_eq!(v.extension, ".exe");

        let dot = PathVars::from_path(r"C:\x\.gitignore");
        assert_eq!(dot.extension, "");
        assert_eq!(dot.name_no_ext, ".gitignore");
    }

    #[test]
    fn substitution_examples() {
        let v = PathVars::from_path(r"C:\test\MpDlpCmd.exe");
        assert_eq!(substitute("{path}", &v, &v), r"C:\test\MpDlpCmd.exe");
        assert_eq!(substitute("\"{path}\"", &v, &v), "\"C:\\test\\MpDlpCmd.exe\"");
        assert_eq!(
            substitute("cmd.exe /s /k pushd \"{path}\"", &v, &v),
            "cmd.exe /s /k pushd \"C:\\test\\MpDlpCmd.exe\""
        );
        assert_eq!(
            substitute("a -ad \"{parent}\\{nameNoExt}.7z\" \"{path}\"", &v, &v),
            "a -ad \"C:\\test\\MpDlpCmd.7z\" \"C:\\test\\MpDlpCmd.exe\""
        );
    }

    #[test]
    fn join_mode() {
        let cfg = MenuConfig::parse(
            r#"{"title":"T","exe":"x","param":"{path}","acceptMultipleFilesFlag":2,"pathDelimiter":","}"#,
        )
        .unwrap();
        let paths = vec!["p1.txt".to_string(), "p2.txt".to_string()];
        assert_eq!(build_param(&cfg, &paths), "\"p1.txt\",\"p2.txt\"");
    }

    #[test]
    fn join_default_delimiter_is_space() {
        let cfg = MenuConfig::parse(
            r#"{"title":"T","exe":"x","param":"{path}","acceptMultipleFilesFlag":2}"#,
        )
        .unwrap();
        let paths = vec!["a".to_string(), "b".to_string()];
        assert_eq!(build_param(&cfg, &paths), "\"a\" \"b\"");
    }

    #[test]
    fn param_for_multiple_files_used_in_join() {
        let cfg = MenuConfig::parse(
            r#"{"title":"T","exe":"x","param":"single {path}","paramForMultipleFiles":"multi {path}","acceptMultipleFilesFlag":2}"#,
        )
        .unwrap();
        assert_eq!(
            build_param(&cfg, &["a".into(), "b".into()]),
            "multi \"a\" \"b\""
        );
        assert_eq!(build_param(&cfg, &["a".into()]), "single a");
    }
}
