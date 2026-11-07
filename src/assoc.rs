//! Register MeowDown as a Markdown handler in the current user's registry.
//!
//! Everything is written under `HKCU`, so it needs no administrator rights and
//! can be undone from the settings dialog — or from the command line, with
//! `meowdown.exe --unregister`.
//!
//! Two things have to be true for a double-clicked `.md` to land here, and they
//! are not the same thing:
//!
//! * `Software\Classes\.md` must name our ProgID. This is what a program is
//!   allowed to write for itself, and it is the whole answer on older Windows.
//! * `Explorer\FileExts\.md\UserChoice` must name it too. From Windows 10 on,
//!   *this* is the one Explorer reads, and it is deliberately write-protected:
//!   only the user, through Settings, can set it. A program that writes it
//!   behind Explorer's back gets its value reset and its hash rejected.
//!
//! So this module writes everything it is allowed to write, including the
//! `Capabilities` block that puts MeowDown in the "默认应用" list with a name
//! and an icon instead of a bare path — the last step is a click the user has
//! to make, and `open_default_apps` takes them to the exact page for it.

/// ProgID owned by this application.
const PROGID: &str = "MeowDown.Markdown";
const EXTENSIONS: [&str; 3] = [".md", ".markdown", ".mdown"];
/// Where the windows "默认应用" page looks for our name and icon.
const CAPABILITIES: &str = "Software\\MeowDown\\Capabilities";
/// The name the registered-application list knows us by, and the one the
/// `ms-settings:defaultapps?registeredAppUser=` link has to repeat.
const APP_KEY: &str = "MeowDown";
/// Marks the one-time registration a fresh copy of MeowDown does for itself.
const ASKED: &str = "Software\\MeowDown";

#[cfg(windows)]
pub fn register(exe: &std::path::Path) -> Result<(), String> {
    imp::register(exe)
}

#[cfg(windows)]
pub fn unregister() -> Result<(), String> {
    imp::unregister()
}

/// A short human-readable description of the current association.
#[cfg(windows)]
pub fn status() -> String {
    imp::status()
}

/// Whether a double-clicked `.md` already lands in MeowDown.
#[cfg(windows)]
pub fn is_default() -> bool {
    imp::default_progid().as_deref() == Some(PROGID)
}

/// Whether MeowDown has already offered to take the association. A copy that
/// has never run must not be told "already asked" — the flag is written only
/// after a registration attempt.
#[cfg(windows)]
pub fn has_asked() -> bool {
    imp::has_asked()
}

/// Opens the Windows page that lists MeowDown among the apps a `.md` can be
/// opened with. The last click of an association cannot be scripted, so the
/// program at least walks the user to the right screen.
#[cfg(windows)]
pub fn open_default_apps() {
    let _ = std::process::Command::new("cmd")
        .args([
            "/C",
            "start",
            "",
            "ms-settings:defaultapps?registeredAppUser=MeowDown",
        ])
        .spawn();
}

#[cfg(not(windows))]
pub fn register(_exe: &std::path::Path) -> Result<(), String> {
    Err("目前只在 Windows 上支持文件关联".to_string())
}

#[cfg(not(windows))]
pub fn unregister() -> Result<(), String> {
    Err("目前只在 Windows 上支持文件关联".to_string())
}

#[cfg(not(windows))]
pub fn status() -> String {
    "仅 Windows 支持".to_string()
}

#[cfg(not(windows))]
pub fn is_default() -> bool {
    false
}

#[cfg(not(windows))]
pub fn has_asked() -> bool {
    true
}

#[cfg(not(windows))]
pub fn open_default_apps() {}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    use super::{APP_KEY, ASKED, CAPABILITIES, EXTENSIONS, PROGID};

    fn classes(write: bool) -> Result<RegKey, String> {
        let flags = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Software\\Classes", flags)
            .map_err(|err| format!("打开注册表失败：{err}"))
    }

    /// The ProgID Explorer will actually use for `.md`, whichever of the two
    /// places that is.
    ///
    /// `UserChoice` wins whenever it exists: from Windows 10 on it is set for
    /// every extension the user has ever told Windows how to open, and it keeps
    /// winning even when `Software\Classes\.md` says something else.
    pub fn default_progid() -> Option<String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.md\\UserChoice",
        ) {
            if let Ok(progid) = key.get_value::<String, _>("ProgId") {
                if !progid.is_empty() {
                    return Some(progid);
                }
            }
        }
        hkcu.open_subkey("Software\\Classes\\.md")
            .and_then(|key| key.get_value::<String, _>(""))
            .ok()
            .filter(|progid| !progid.is_empty())
    }

    pub fn register(exe: &Path) -> Result<(), String> {
        let classes = classes(true)?;
        let exe_text = exe.to_string_lossy().to_string();
        let command = format!("\"{exe_text}\" \"%1\"");

        // The document class itself.
        let (prog, _) = classes
            .create_subkey(PROGID)
            .map_err(|err| format!("创建 ProgID 失败：{err}"))?;
        prog.set_value("", &"Markdown 文档")
            .map_err(|err| err.to_string())?;
        prog.set_value("FriendlyTypeName", &"Markdown 文档")
            .map_err(|err| err.to_string())?;
        prog.create_subkey("DefaultIcon")
            .and_then(|(key, _)| key.set_value("", &format!("{exe_text},0")))
            .map_err(|err| err.to_string())?;
        prog.create_subkey("shell\\open\\command")
            .and_then(|(key, _)| key.set_value("", &command))
            .map_err(|err| err.to_string())?;

        // Every Markdown extension points at it. The previous default is kept
        // so "取消关联" can put it back instead of leaving nothing behind.
        let mut previous = String::new();
        for ext in EXTENSIONS {
            let (key, _) = classes
                .create_subkey(ext)
                .map_err(|err| format!("创建 {ext} 关联失败：{err}"))?;
            let current: String = key.get_value("").unwrap_or_default();
            if ext == ".md" && current != PROGID && !current.is_empty() {
                previous = current;
            }
            key.set_value("", &PROGID).map_err(|err| err.to_string())?;
            key.create_subkey("OpenWithProgids")
                .and_then(|(key, _)| key.set_value(PROGID, &""))
                .map_err(|err| err.to_string())?;
        }
        if !previous.is_empty() {
            prog.set_value("PreviousMdDefault", &previous)
                .map_err(|err| err.to_string())?;
        }

        // Make the app appear under "打开方式" and in Windows' default-app list.
        let app_name = exe
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "meowdown.exe".to_string());
        let (app, _) = classes
            .create_subkey(format!("Applications\\{app_name}"))
            .map_err(|err| err.to_string())?;
        app.set_value("FriendlyAppName", &"MeowDown")
            .map_err(|err| err.to_string())?;
        app.create_subkey("shell\\open\\command")
            .and_then(|(key, _)| key.set_value("", &command))
            .map_err(|err| err.to_string())?;
        for ext in EXTENSIONS {
            app.create_subkey("SupportedTypes")
                .and_then(|(key, _)| key.set_value(ext, &""))
                .ok();
        }

        // Without this block Windows has a path and a command line but no
        // *application*: `.md` would show up as an anonymous handler, and the
        // "默认应用" page would not list MeowDown at all. `RegisteredApplications`
        // is what turns the block into an entry there.
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (caps, _) = hkcu
            .create_subkey(CAPABILITIES)
            .map_err(|err| format!("写入默认应用信息失败：{err}"))?;
        caps.set_value("ApplicationName", &"MeowDown")
            .map_err(|err| err.to_string())?;
        caps.set_value(
            "ApplicationDescription",
            &"所见即所得的 Markdown 编辑器，双击 .md 直接打开",
        )
        .map_err(|err| err.to_string())?;
        caps.set_value("ApplicationIcon", &format!("{exe_text},0"))
            .map_err(|err| err.to_string())?;
        let (assoc, _) = caps.create_subkey("FileAssociations").map_err(|err| err.to_string())?;
        for ext in EXTENSIONS {
            assoc.set_value(ext, &PROGID).map_err(|err| err.to_string())?;
        }
        hkcu.create_subkey("Software\\RegisteredApplications")
            .and_then(|(key, _)| key.set_value(APP_KEY, &CAPABILITIES))
            .map_err(|err| format!("注册默认应用失败：{err}"))?;

        mark_asked();
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        let classes = classes(true)?;
        // Put back whatever was the default before, if anything was recorded.
        let previous: String = classes
            .open_subkey(PROGID)
            .and_then(|key| key.get_value("PreviousMdDefault"))
            .unwrap_or_default();

        for ext in EXTENSIONS {
            if let Ok(key) = classes.open_subkey_with_flags(ext, KEY_READ | KEY_WRITE) {
                let current: String = key.get_value("").unwrap_or_default();
                if current == PROGID {
                    if ext == ".md" && !previous.is_empty() {
                        let _ = key.set_value("", &previous);
                    } else {
                        let _ = key.delete_value("");
                    }
                }
                // The ProgID is a *value* under `OpenWithProgids`, not a
                // subkey: deleting a subkey of that name leaves the entry on
                // the "打开方式" menu for good.
                if let Ok(open_with) =
                    key.open_subkey_with_flags("OpenWithProgids", KEY_READ | KEY_WRITE)
                {
                    let _ = open_with.delete_value(PROGID);
                }
            }
        }
        let _ = classes.delete_subkey_all(PROGID);

        // The "打开方式" list is keyed by the executable's file name, which is
        // the only way to find the entry this copy of MeowDown wrote.
        if let Some(name) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.file_name().map(|name| name.to_string_lossy().to_string()))
        {
            let _ = classes.delete_subkey_all(format!("Applications\\{name}"));
        }

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey_with_flags("Software\\RegisteredApplications", KEY_WRITE) {
            let _ = key.delete_value(APP_KEY);
        }
        let _ = hkcu.delete_subkey_all("Software\\MeowDown\\Capabilities");
        mark_asked();
        Ok(())
    }

    pub fn has_asked() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(ASKED)
            .and_then(|key| key.get_value::<u32, _>("AssociationAsked"))
            .map(|value| value != 0)
            .unwrap_or(false)
    }

    pub fn mark_asked() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok((key, _)) = hkcu.create_subkey(ASKED) {
            let _ = key.set_value("AssociationAsked", &1u32);
        }
    }

    pub fn status() -> String {
        match default_progid() {
            Some(progid) if progid == PROGID => ".md 默认由 MeowDown 打开".to_string(),
            Some(progid) => format!("Windows 目前用 {progid} 打开 .md"),
            None => ".md 尚未关联".to_string(),
        }
    }
}
