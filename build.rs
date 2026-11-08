//! Embeds `assets/app.ico` into the executable as icon resource #1, and stamps
//! the crate version into the PE version resource.
//!
//! gpui's Windows backend gives a window its icon by calling
//! `LoadImageW(module, MAKEINTRESOURCE(1), IMAGE_ICON, ..)` on the module
//! itself, so the icon has to live in the PE resources — `WindowOptions.icon`
//! is not consulted on Windows. The same resource is what Explorer and the
//! taskbar show for the executable.
//!
//! The version block is there so `package.ps1` can read the version off the
//! file instead of running it: the release build is a GUI-subsystem program
//! (`windows_subsystem = "windows"`), and its stdout is not something a
//! PowerShell one-liner can reliably capture.
//!
//! `rc.exe` comes from the Windows SDK. We look it up by path instead of going
//! through `embed-resource`/`winres`, because both of those probe the registry
//! (`reg.exe`), which is unavailable in some build sandboxes.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Preferred SDK versions, newest first. 10.0.26100 occasionally hangs when
/// spawned from a build script, so 22621 is tried before it.
const PREFERRED: [&str; 2] = ["10.0.22621.0", "10.0.26100.0"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // Only the MSVC toolchain understands .res files.
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let icon = manifest.join("assets").join("app.ico");
    if !icon.is_file() {
        println!(
            "cargo:warning=assets/app.ico is missing; run `python tools/make_icon.py` to build the icon"
        );
        return;
    }

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found; the executable keeps its default icon");
        return;
    };

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let [a, b, c, d] = version_quad(&version);

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let script = out.join("meowdown.rc");
    // rc.exe wants backslashes escaped inside a quoted string.
    let escaped = icon.display().to_string().replace('\\', "\\\\");
    let contents = format!(
        "1 ICON \"{escaped}\"\n\
         \n\
         1 VERSIONINFO\n\
         FILEVERSION {a},{b},{c},{d}\n\
         PRODUCTVERSION {a},{b},{c},{d}\n\
         FILEOS 0x40004L\n\
         FILETYPE 0x1L\n\
         BEGIN\n\
         BLOCK \"StringFileInfo\"\n\
         BEGIN\n\
         BLOCK \"080404b0\"\n\
         BEGIN\n\
         VALUE \"CompanyName\", \"xarzhi\"\n\
         VALUE \"FileDescription\", \"MeowDown\"\n\
         VALUE \"FileVersion\", \"{version}\"\n\
         VALUE \"InternalName\", \"MeowDown\"\n\
         VALUE \"OriginalFilename\", \"MeowDown.exe\"\n\
         VALUE \"ProductName\", \"MeowDown\"\n\
         VALUE \"ProductVersion\", \"{version}\"\n\
         END\n\
         END\n\
         BLOCK \"VarFileInfo\"\n\
         BEGIN\n\
         VALUE \"Translation\", 0x804, 1200\n\
         END\n\
         END\n"
    );
    if let Err(err) = fs::write(&script, contents) {
        println!("cargo:warning=could not write meowdown.rc: {err}");
        return;
    }

    let res = out.join("meowdown.res");
    match Command::new(&rc).arg("/nologo").arg("/fo").arg(&res).arg(&script).output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            println!(
                "cargo:warning=rc.exe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return;
        }
        Err(err) => {
            println!("cargo:warning=could not run {}: {err}", rc.display());
            return;
        }
    }

    let res = res.display().to_string();
    if res.contains(' ') {
        // A quoted path would be split by the compiler driver.
        println!("cargo:warning=ignoring exe icon: build path contains a space");
        return;
    }
    println!("cargo:rustc-link-arg-bins={res}");
}

/// `0.2.0` and `0.2.0-beta.1` both become `0,2,0,0`: `FILEVERSION` takes four
/// plain numbers and would otherwise reject the resource.
fn version_quad(version: &str) -> [u16; 4] {
    let numeric = version
        .split(['-', '+'])
        .next()
        .unwrap_or("0");
    let mut parts = [0u16; 4];
    for (slot, piece) in parts.iter_mut().zip(numeric.split('.')) {
        *slot = piece.parse().unwrap_or(0);
    }
    parts
}

fn find_rc() -> Option<PathBuf> {
    if let Some(explicit) = env::var_os("RC_EXE") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }

    // A vcvars64 prompt exports both of these.
    if let (Ok(dir), Ok(version)) = (env::var("WindowsSdkDir"), env::var("WindowsSDKVersion")) {
        let candidate = Path::new(&dir)
            .join("bin")
            .join(version.trim_end_matches('\\'))
            .join("x64")
            .join("rc.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut roots = vec![
        PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin"),
        PathBuf::from(r"D:\Windows Kits\10\bin"),
        PathBuf::from(r"C:\Program Files\Windows Kits\10\bin"),
    ];
    if let Some(sdk) = env::var_os("DEFAULT_SDK_ROOT") {
        roots.push(PathBuf::from(sdk).join("bin"));
    }

    let mut found: Vec<(u32, PathBuf)> = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let version = entry.file_name().to_string_lossy().to_string();
            let rc = entry.path().join("x64").join("rc.exe");
            if !rc.is_file() {
                continue;
            }
            // Rank known-good versions first, then anything else newest-first.
            let rank = match PREFERRED.iter().position(|p| *p == version) {
                Some(index) => index as u32,
                None => 100,
            };
            found.push((rank, rc));
        }
    }
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, path)| path).next()
}
