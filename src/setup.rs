//! Pure package-generation logic for ctxmenu-setup (no OS calls, tested on any
//! platform). The identity/CLSID scheme is pinned by the reference vectors in
//! the tests below and must never change, or existing installs break.

use crate::config::{
    ext_list, MenuConfig, DIR_BACKGROUND, DIR_DIRECTORY, DIR_DRIVE, FILE_EXT_LIST, FILE_NONE,
};

/// Package identity prefix; also the classic-menu registry key prefix.
pub const ID_PREFIX: &str = "CtxRs.";

/// 1x1 transparent PNG used as the package logo placeholder.
pub const LOGO_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
    0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 96, 248, 95, 15,
    0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// Alphanumeric slug of a config file name, max 30 chars, fallback "entry".
pub fn slug(file_name: &str) -> String {
    let stem = if file_name.len() >= 5 && file_name[file_name.len() - 5..].eq_ignore_ascii_case(".json")
    {
        &file_name[..file_name.len() - 5]
    } else {
        file_name
    };
    let s: String = stem.chars().filter(|c| c.is_ascii_alphanumeric()).take(30).collect();
    if s.is_empty() {
        "entry".to_string()
    } else {
        s
    }
}

/// Deterministic CLSID for a config file name: MD5 of the lowercased name,
/// interpreted with .NET `Guid(byte[])` semantics (first three fields
/// little-endian), formatted "D" uppercase. Matches the PowerShell installer's
/// `Get-DeterministicGuid` exactly.
pub fn deterministic_clsid(file_name: &str) -> String {
    let d = md5::compute(file_name.to_lowercase().as_bytes()).0;
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
        u16::from_le_bytes([d[4], d[5]]),
        u16::from_le_bytes([d[6], d[7]]),
        d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]
    )
}

/// File name of a generated (copied / shield-badged) icon for a config slug.
pub fn generated_icon_name(slug: &str, dark: bool) -> String {
    format!("{slug}{}.ico", if dark { ".dark" } else { "" })
}

/// Split an icon resource spec (`C:\x.exe,0`, `"C:\a b.exe",-3`, `C:\x.ico`)
/// into path + icon index.
pub fn parse_icon_spec(spec: &str) -> (String, i32) {
    let s = spec.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            let tail = rest[end + 1..].trim_start().trim_start_matches(',');
            return (rest[..end].to_string(), tail.trim().parse().unwrap_or(0));
        }
    }
    if let Some((p, i)) = s.rsplit_once(',') {
        if let Ok(n) = i.trim().parse::<i32>() {
            return (p.trim().trim_matches('"').to_string(), n);
        }
    }
    (s.trim_matches('"').to_string(), 0)
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The `<desktop*:ItemType>` block for a config's effective flags.
/// Empty result = the config matches nothing and should be skipped.
pub fn item_types_xml(dir_flag: u32, file_flag: u32, clsid: &str) -> String {
    let mut out = String::new();
    let mut push = |ns: &str, item_type: &str| {
        out.push_str(&format!(
            "            <{ns}:ItemType Type=\"{item_type}\"><{ns}:Verb Id=\"Cmd\" Clsid=\"{clsid}\"/></{ns}:ItemType>\r\n"
        ));
    };
    if dir_flag & 1 != 0 {
        push("desktop5", "Directory");
    }
    if dir_flag & (2 | 4) != 0 {
        push("desktop5", "Directory\\Background");
    }
    if file_flag > 0 {
        push("desktop5", "*");
    }
    if dir_flag & 8 != 0 {
        push("desktop10", "Drive");
    }
    out
}

pub fn manifest_xml(identity: &str, title: &str, clsid: &str, item_types: &str) -> String {
    let title = xml_escape(title);
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:desktop4="http://schemas.microsoft.com/appx/manifest/desktop/windows10/4"
  xmlns:desktop5="http://schemas.microsoft.com/appx/manifest/desktop/windows10/5"
  xmlns:desktop10="http://schemas.microsoft.com/appx/manifest/desktop/windows10/10"
  xmlns:com="http://schemas.microsoft.com/appx/manifest/com/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  IgnorableNamespaces="uap desktop4 desktop5 desktop10 com rescap">
  <Identity Name="{identity}" Publisher="CN=ContextMenuRs" Version="1.0.0.0" ProcessorArchitecture="x64" />
  <Properties>
    <DisplayName>{title}</DisplayName>
    <PublisherDisplayName>windows11-context-rs</PublisherDisplayName>
    <Logo>Assets\logo.png</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.22000.0" MaxVersionTested="10.0.26100.0" />
  </Dependencies>
  <Resources>
    <Resource Language="en-us" />
  </Resources>
  <Applications>
    <Application Id="App" Executable="cmrsRun.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements
        DisplayName="{title}"
        Description="Custom context menu entry"
        BackgroundColor="transparent"
        Square150x150Logo="Assets\logo.png"
        Square44x44Logo="Assets\logo.png"
        AppListEntry="none" />
      <Extensions>
        <desktop4:Extension Category="windows.fileExplorerContextMenus">
          <desktop4:FileExplorerContextMenus>
{item_types}          </desktop4:FileExplorerContextMenus>
        </desktop4:Extension>
        <com:Extension Category="windows.comServer">
          <com:ComServer>
            <com:SurrogateServer DisplayName="{title}">
              <com:Class Id="{clsid}" Path="cmrs.dll" ThreadingModel="STA" />
            </com:SurrogateServer>
          </com:ComServer>
        </com:Extension>
      </Extensions>
    </Application>
  </Applications>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>
</Package>
"#
    )
}

/// HKCU-relative parents of classic-menu verbs: folders, backgrounds, drives, all files.
pub const CLASSIC_ROOTS: [&str; 4] = [
    r"Software\Classes\Directory\shell",
    r"Software\Classes\Directory\Background\shell",
    r"Software\Classes\Drive\shell",
    r"Software\Classes\*\shell",
];

/// Per-extension verbs live under `<this>\<.ext>\shell`.
pub const FILE_ASSOC_ROOT: &str = r"Software\Classes\SystemFileAssociations";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicTarget {
    /// HKCU-relative key the verb is created under.
    pub root: String,
    /// Explorer's path placeholder for this kind of target.
    pub arg: &'static str,
    pub title: String,
}

fn ext_shell_root(ext: &str) -> Option<String> {
    let valid = ext.len() > 1 && ext.starts_with('.') && !ext.contains(['\\', '/']);
    valid.then(|| format!(r"{FILE_ASSOC_ROOT}\{ext}\shell"))
}

/// Where a config's classic-menu verbs go. Explorer shows a same-named
/// per-extension verb instead of the `*` one, so title rules override per type.
pub fn classic_targets(cfg: &MenuConfig, title: &str) -> Vec<ClassicTarget> {
    let mut targets = Vec::new();
    if !cfg.classic_menu {
        return targets;
    }
    let mut push = |root: &str, arg: &'static str, title: &str| {
        targets.push(ClassicTarget {
            root: root.to_string(),
            arg,
            title: title.to_string(),
        })
    };

    let dir_flag = cfg.dir_flag();
    if dir_flag & DIR_DIRECTORY != 0 {
        push(CLASSIC_ROOTS[0], "%V", title);
    }
    if dir_flag & DIR_BACKGROUND != 0 {
        push(CLASSIC_ROOTS[1], "%V", title);
    }
    if dir_flag & DIR_DRIVE != 0 {
        push(CLASSIC_ROOTS[2], "%V", title);
    }

    let per_ext: Vec<String> = match cfg.file_flag() {
        FILE_NONE => Vec::new(),
        FILE_EXT_LIST => ext_list(&cfg.accept_exts).collect(),
        _ => {
            push(CLASSIC_ROOTS[3], "%1", title);
            cfg.title_rules
                .iter()
                .flat_map(|rule| ext_list(&rule.accept_exts))
                .collect()
        }
    };

    let mut seen = std::collections::HashSet::new();
    for ext in per_ext {
        let Some(root) = ext_shell_root(&ext) else {
            continue;
        };
        if seen.insert(ext.clone()) {
            let ext_title = cfg.rule_for_ext(&ext).map_or(title, |rule| &rule.title);
            push(&root, "%1", ext_title);
        }
    }
    targets
}

/// packages.json content for the DLL: CLSID -> { title, files }.
pub fn packages_json(entries: &[(String, String, String)]) -> String {
    let mut clsids = serde_json::Map::new();
    for (clsid, title, file) in entries {
        clsids.insert(
            clsid.clone(),
            serde_json::json!({ "title": title, "files": [file] }),
        );
    }
    serde_json::to_string_pretty(&serde_json::json!({ "clsids": clsids })).expect("static json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clsid_reference_vectors() {
        // Pinned values (originally derived from the retired PowerShell installer);
        // existing installs depend on these never changing.
        assert_eq!(
            deterministic_clsid("Open in Terminal (Admin).json"),
            "F45A1604-346E-8227-E86D-48177CBE7295"
        );
        assert_eq!(deterministic_clsid("a.json"), "5CACDDE5-73ED-D7ED-BFED-38B88716496E");
    }

    #[test]
    fn clsid_is_case_insensitive() {
        assert_eq!(deterministic_clsid("A.JSON"), deterministic_clsid("a.json"));
    }

    #[test]
    fn slug_rules() {
        assert_eq!(slug("Open in Terminal (Admin).json"), "OpeninTerminalAdmin");
        assert_eq!(slug("a.JSON"), "a");
        assert_eq!(slug("---.json"), "entry");
        assert_eq!(slug(&format!("{}.json", "x".repeat(40))), "x".repeat(30));
    }

    #[test]
    fn item_types_follow_flags() {
        let all = item_types_xml(15, 4, "G");
        assert!(all.contains("Type=\"Directory\""));
        assert!(all.contains("Type=\"Directory\\Background\""));
        assert!(all.contains("Type=\"*\""));
        assert!(all.contains("desktop10:ItemType Type=\"Drive\""));

        assert_eq!(item_types_xml(0, 0, "G"), "");
        let bg_only = item_types_xml(4, 0, "G");
        assert!(bg_only.contains("Directory\\Background"));
        assert!(!bg_only.contains("Type=\"Directory\"><"));
    }

    #[test]
    fn manifest_escapes_title() {
        let m = manifest_xml("CtxRs.x", "A & B <\"'>", "G", "");
        assert!(m.contains("<DisplayName>A &amp; B &lt;&quot;&apos;&gt;</DisplayName>"));
        assert!(m.contains("Name=\"CtxRs.x\""));
    }

    #[test]
    fn icon_spec_parsing() {
        assert_eq!(parse_icon_spec(r"C:\x\wt.exe,0"), (r"C:\x\wt.exe".into(), 0));
        assert_eq!(parse_icon_spec(r"C:\i.ico"), (r"C:\i.ico".into(), 0));
        assert_eq!(
            parse_icon_spec(r#""C:\a b\app.exe",-78"#),
            (r"C:\a b\app.exe".into(), -78)
        );
        assert_eq!(
            parse_icon_spec(r"C:\dir,with,commas\i.ico"),
            (r"C:\dir,with,commas\i.ico".into(), 0)
        );
        assert_eq!(parse_icon_spec(" \"C:\\q.exe\" , 3 "), (r"C:\q.exe".into(), 3));
    }

    #[test]
    fn generated_icon_names() {
        assert_eq!(generated_icon_name("Slug", false), "Slug.ico");
        assert_eq!(generated_icon_name("Slug", true), "Slug.dark.ico");
    }

    fn roots_and_titles(targets: &[ClassicTarget]) -> Vec<(&str, &str)> {
        targets
            .iter()
            .map(|t| (t.root.as_str(), t.title.as_str()))
            .collect()
    }

    #[test]
    fn classic_targets_all_files_with_rules() {
        let cfg = MenuConfig::parse(
            r#"{"title":"Upload file","exe":"x","acceptDirectoryFlag":"none","acceptFileFlag":"all",
                "titleRules":[
                    {"acceptExts":".png|.jpg","title":"Upload image"},
                    {"acceptExts":".PNG|.mp4","title":"Upload video"}
                ]}"#,
        )
        .unwrap();
        let targets = classic_targets(&cfg, "Upload file");
        assert_eq!(
            roots_and_titles(&targets),
            [
                (r"Software\Classes\*\shell", "Upload file"),
                (r"Software\Classes\SystemFileAssociations\.png\shell", "Upload image"),
                (r"Software\Classes\SystemFileAssociations\.jpg\shell", "Upload image"),
                (r"Software\Classes\SystemFileAssociations\.mp4\shell", "Upload video"),
            ]
        );
        assert!(targets.iter().all(|t| t.arg == "%1"));
    }

    #[test]
    fn classic_targets_ext_list_skips_the_all_files_root() {
        let cfg = MenuConfig::parse(
            r#"{"title":"T","exe":"x","acceptDirectoryFlag":["directory","drive"],
                "acceptFileFlag":"extensionList","acceptExts":".txt|.md|bad|",
                "titleRules":[{"acceptExts":".md","title":"Markdown"}]}"#,
        )
        .unwrap();
        assert_eq!(
            roots_and_titles(&classic_targets(&cfg, "T")),
            [
                (r"Software\Classes\Directory\shell", "T"),
                (r"Software\Classes\Drive\shell", "T"),
                (r"Software\Classes\SystemFileAssociations\.txt\shell", "T"),
                (r"Software\Classes\SystemFileAssociations\.md\shell", "Markdown"),
            ]
        );
    }

    #[test]
    fn classic_targets_disabled() {
        let cfg =
            MenuConfig::parse(r#"{"title":"T","exe":"x","classicMenu":false}"#).unwrap();
        assert!(classic_targets(&cfg, "T").is_empty());
    }

    #[test]
    fn packages_json_round_trips() {
        let s = packages_json(&[("G1".into(), "T1".into(), "f1.json".into())]);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["clsids"]["G1"]["title"], "T1");
        assert_eq!(v["clsids"]["G1"]["files"][0], "f1.json");
    }
}
