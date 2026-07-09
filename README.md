# windows11-context-rs

A Rust shell extension that adds **custom entries to the Windows 11 modern (top-level) context menu**, defined by simple JSON config files:

- **One config file = one top-level entry** — Windows only allows one top-level menu entry per app, so the installer generates a tiny loose MSIX package (unique identity + CLSID) *per config file*. Each config becomes its own true top-level button. The DLL knows which config to serve from the CLSID it was created with (`packages.json` mapping).
- **Flexible matching** — entries can target folders, folder backgrounds, the desktop, drives, or files (by extension list or regex), and handle multi-selection by running once per item or once with all paths joined.
- **Command and icon control** — commands can run elevated, in a chosen window state and working directory; menu icons can be snapshotted locally and badged with a corner overlay (e.g. the UAC shield).

Entries also work in the classic ("Show more options") menu via `cmrsRun.exe` registry verbs.

## How it works

```
custom_commands\*.json  ──installer──►  one loose package per file
                                        (AppxManifest + CLSID + dll copy)
                                              │
Explorer (new menu) ──► dllhost surrogate ──► cmrs.dll
                                              │  CLSID → packages.json → config file
                                              │  match folder/file/drive rules
                                              └► ShellExecuteExW ("runas" if runAsAdmin)

Classic menu ──► HKCU\...\shell\CtxRs.* ──► cmrsRun.exe <config.json> <path>
```

- Exactly **one** matching entry per package → rendered directly with its own title/icon (no flyout).
- A CLSID mapped to **multiple** files (or an unmapped CLSID) → one flyout serving those files.

## Build

```powershell
cargo build --release        # needs Rust (MSVC toolchain) on Windows
cargo test                   # config/matching/substitution unit tests
```

Or push to GitHub — `.github/workflows/build.yml` runs the tests and uploads `cmrs.dll` + `cmrsRun.exe` + `cmrsSetup.exe` as an artifact (and attaches them to releases on `v*` tags).

## Install

```powershell
# from the repo root after building (or from the extracted CI artifact folder)
.\target\release\cmrsSetup.exe install
```

The installer:

1. Enables **Developer Mode** if needed (unsigned loose packages require it; one-time UAC prompt).
2. Copies binaries to `%LOCALAPPDATA%\ContextMenuRs\bin`.
3. Generates + registers one package per `*.json` in `%LOCALAPPDATA%\ContextMenuRs\custom_commands`.
4. Adds matching classic-menu registry entries.

No configs yet? Copy one from [examples/](examples/) into `%LOCALAPPDATA%\ContextMenuRs\custom_commands`, then run `cmrsSetup sync`.

After editing/adding/removing configs: `cmrsSetup sync`
See what's installed: `cmrsSetup list` (each config plus installed / skipped / not-yet-synced status).
Uninstall: `cmrsSetup uninstall` (add `--purge` to also delete configs).

If a new entry doesn't show up, restart Explorer (Task Manager → Windows Explorer → Restart).

## Config format

Every `*.json` file in `%LOCALAPPDATA%\ContextMenuRs\custom_commands` is one top-level menu entry. Only `title` and `exe` are required; everything else has sensible defaults.

Configs written for [ContextMenuForWindows11](https://github.com/ikas-mc/ContextMenuForWindows11) also work unchanged — its numeric flag values and its deprecated `acceptDirectory`/`acceptFile` booleans are still accepted.

### Editor autocomplete

Add a `"$schema"` field to get validation and autocomplete in editors that support JSON Schema (VS Code, JetBrains IDEs, ...):

```json
"$schema": "https://raw.githubusercontent.com/SalahaldinBilal/windows11-context-rs/main/menu.schema.json"
```

The schema lives at [menu.schema.json](menu.schema.json).

### Example — [Open in Terminal (Admin)](examples/Open%20in%20Terminal%20%28Admin%29.json)

```json
{
  "$schema": "https://raw.githubusercontent.com/SalahaldinBilal/windows11-context-rs/main/menu.schema.json",
  "title": "Open in Terminal (Admin)",
  "exe": "wt.exe",
  "param": "-d \"{path}\"",
  "icon": "exe",
  "acceptDirectoryFlag": "all",
  "acceptFileFlag": "none",
  "runAsAdmin": true,
  "smallIcon": { "icon": "uac" },
  "showWindowFlag": "normal"
}
```

`"icon": "exe"` reuses Windows Terminal's own icon, badged with the system UAC shield from `smallIcon`.

### Every field, explained

> The comments below are documentation only — **real config files must be plain JSON without comments.**

```jsonc
{
  // ------------------------------------------------- what the entry looks like

  // Menu text. Required.
  "title": "Open With VS Code",

  // Menu icon (optional). Either an .ico/.png file, "file.exe,N" where N is
  // the icon index inside the exe/dll (negative N = resource id), or the
  // special value "exe" to reuse "exe" below as the icon source.
  // %EnvVars% are expanded. Set "copyIcon" below to also snapshot the icon
  // locally, so a moving source path can't break it.
  "icon": "%LocalAppData%\\Programs\\Microsoft VS Code\\Code.exe,0",

  // Alternative icon used when Windows is in dark mode (optional).
  "iconDark": "",

  // Snapshot the icon into a local .ico when you run `cmrsSetup sync`, and
  // point the menu at that copy. The entry then keeps its icon even if the
  // source moves - e.g. Store apps (wt.exe) change paths on every update.
  "copyIcon": true,

  // Composite a half-size badge onto the corner of the icon.
  //   icon:     any icon spec like above, or the special values "uac" (system
  //             UAC shield) / "exe" (this entry's own exe).
  //   location: "topLeft" | "topRight" | "bottomLeft" | "bottomRight"
  //             (optional, default "bottomRight").
  // With no base "icon", the badge is shown on its own at full size.
  "smallIcon": { "icon": "uac", "location": "bottomRight" },

  // Sort position among entries in the same flyout (lower = higher up).
  // Windows controls the order of separate top-level entries, not this field.
  "index": 0,

  // ------------------------------------------------- when the entry shows up

  // For folder-like right-click targets. Any combination of:
  //   "directory"  - right-clicking a folder
  //   "background" - right-clicking the empty background inside a folder
  //   "desktop"    - right-clicking the desktop background
  //   "drive"      - right-clicking a drive root (C:, D:, ...)
  // or the shortcuts "all" / "none". A single string also works.
  // (Numeric bitmask kept for compatibility: 1|2|4|8, 15 = all.)
  "acceptDirectoryFlag": ["directory", "background", "drive"],

  // For file right-click targets. One of:
  //   "none"          - never show on files
  //   "extension"     - fuzzy match: file's extension appears in acceptExts
  //   "regex"         - acceptFileRegex must match the file NAME
  //   "extensionList" - exact match against the '|'-separated acceptExts list
  //   "all"           - show on every file
  // (Numeric kept for compatibility: 0 / 1 / 2 / 3 / 4.)
  "acceptFileFlag": "extensionList",

  // Extensions for the "extension" / "extensionList" modes (with leading dot).
  "acceptExts": ".txt|.md|.log",

  // Regular expression for the "regex" mode, tested against the file name.
  "acceptFileRegex": "",

  // What happens when SEVERAL items are selected:
  //   "none" - entry only appears for single-item selections
  //   "each" - run the command once per selected item
  //   "join" - run once, with all paths joined into {path}
  // (Numeric kept for compatibility: 0 / 1 / 2.)
  "acceptMultipleFilesFlag": "join",

  // "join" mode: separator between the quoted paths (default: one space).
  "pathDelimiter": " ",

  // "join" mode: alternative param template used when 2+ items are selected
  // (falls back to "param" if empty).
  "paramForMultipleFiles": "--files {path}",

  // ------------------------------------------------- how the command runs

  // Program to launch. Required. %EnvVars% ARE expanded here, and surrounding
  // quotes are tolerated.
  "exe": "\"%LocalAppData%\\Programs\\Microsoft VS Code\\Code.exe\"",

  // Command-line arguments. {variables} are substituted (see table below).
  "param": "\"{path}\"",

  // Working directory for the program; {variables} and %EnvVars% are expanded.
  "workingDirectory": "{parent}",

  // Launch elevated (UAC prompt).
  "runAsAdmin": false,

  // Program window state: "hide" | "normal" | "minimized" | "maximized"
  // (numeric kept for compatibility: -1 / 0 / 1 / 2).
  "showWindowFlag": "normal"
}
```

### Variables

Usable in `param`, `paramForMultipleFiles` and `workingDirectory`:

| Variable | Meaning (for `C:\test\Report.pdf`) |
|---|---|
| `{path}` | full path — `C:\test\Report.pdf` (in `join` mode: all selected paths, quoted and joined) |
| `{parent}` | containing folder — `C:\test` |
| `{name}` | file name — `Report.pdf` |
| `{nameNoExt}` | name without extension — `Report` |
| `{extension}` | extension with dot — `.pdf` |
| `{path0}` `{name0}` `{nameNoExt0}` `{extension0}` | same, but always for the *first* selected item |

## Known limitations

- **Unsigned packages / Developer Mode**: without a code-signing certificate the packages register unsigned, which requires Developer Mode to stay enabled.
- **Ordering**: Windows decides the relative order of top-level entries from different packages — `index` only orders items *within* a flyout.
- **Classic-menu file filtering**: registry verbs can't do regex/extension matching, so file-type entries appear under `*\shell` for all files in the old menu (the new menu filters correctly).
- **Icon paths**: Store-app icon paths (`wt.exe` under `WindowsApps\...`) break when the app updates — set `"copyIcon": true` to snapshot the icon locally, or re-run `cmrsSetup sync` after fixing the path. `%EnvVars%` are expanded in `icon`/`iconDark`.
- After a major Windows update, re-run the installer if entries disappear.
