//! Embeds `assets/app.ico` into the executable as icon resource #1.
//!
//! gpui's Windows backend gives a window its icon by calling
//! `LoadImageW(module, MAKEINTRESOURCE(1), IMAGE_ICON, ..)` on the module
//! itself, so the icon has to live in the PE resources — `WindowOptions.icon`
//! is not consulted on Windows. The same resource is what Explorer and the
//! taskbar show for the executable.
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

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let script = out.join("icon.rc");
    // rc.exe wants backslashes escaped inside a quoted string.
    let escaped = icon.display().to_string().replace('\\', "\\\\");
    if let Err(err) = fs::write(&script, format!("1 ICON \"{escaped}\"\n")) {
        println!("cargo:warning=could not write icon.rc: {err}");
        return;
    }

    let res = out.join("icon.res");
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
