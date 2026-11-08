//! File-system chores that only the Windows shell can do properly: putting a
//! file in the Recycle Bin instead of deleting it for good, and reading a file
//! time back as a local wall-clock string.
//!
//! Both are a handful of `extern "system"` declarations rather than a new
//! dependency: the app already talks to `winreg` directly, and pulling in the
//! whole `windows` crate for two structs would cost more than it saves.

use std::path::Path;
use std::time::SystemTime;

/// Put `path` in the Recycle Bin, file or folder.
///
/// Deleting a note is the one thing in this editor that cannot be undone with
/// Ctrl+Z, so it goes where Explorer puts things: the user can still get it
/// back from the bin. A path the shell refuses is reported rather than forced.
pub fn recycle(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::recycle(path)
    }
    #[cfg(not(windows))]
    {
        let result = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        result.map_err(|err| err.to_string())
    }
}

/// When `path` was last written, as `2026-11-08 12:03`, in local time.
pub fn modified(path: &Path) -> Option<String> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    local_time(time)
}

/// When `path` was created, in the same shape as [`modified`].
pub fn created(path: &Path) -> Option<String> {
    let time = std::fs::metadata(path).ok()?.created().ok()?;
    local_time(time)
}

/// A `SystemTime` as `2026-11-08 12:03`, local time.
///
/// `SystemTime` counts from 1970 in UTC and has no time zone of its own, so the
/// conversion goes through the same two calls Explorer uses: FILETIME, then
/// local FILETIME, then broken-down local time.
pub fn local_time(time: SystemTime) -> Option<String> {
    #[cfg(windows)]
    {
        win::local_time(time)
    }
    #[cfg(not(windows))]
    {
        let _ = time;
        None
    }
}

/// `1536` → `1.5 KB`. Sizes are shown in the properties card, where the exact
/// byte count is also printed, so one decimal is enough here.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} 字节");
    }
    let mut value = bytes as f64 / 1024.;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::time::SystemTime;

    /// `SHFILEOPSTRUCTW`. Field order matches the Win32 header exactly; the
    /// padding `#[repr(C)]` inserts is the padding the API expects (56 bytes on
    /// x64).
    #[repr(C)]
    struct ShFileOp {
        hwnd: *mut core::ffi::c_void,
        w_func: u32,
        p_from: *const u16,
        p_to: *const u16,
        f_flags: u16,
        aborted: i32,
        name_mappings: *mut core::ffi::c_void,
        progress_title: *const u16,
    }

    const FO_DELETE: u32 = 3;
    const FOF_SILENT: u16 = 0x0004;
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    const FOF_ALLOWUNDO: u16 = 0x0040;
    const FOF_NOERRORUI: u16 = 0x0400;

    unsafe extern "system" {
        fn SHFileOperationW(op: *mut ShFileOp) -> i32;
    }

    pub fn recycle(path: &Path) -> Result<(), String> {
        // The source list is double-NUL terminated even when it holds one path,
        // and it has to stay alive for the length of the call.
        let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
        from.push(0);
        from.push(0);

        let mut op = ShFileOp {
            hwnd: std::ptr::null_mut(),
            w_func: FO_DELETE,
            p_from: from.as_ptr(),
            p_to: std::ptr::null(),
            // No confirmation and no error UI: this editor asked already, in
            // its own dialog, and a second Windows prompt would only confuse.
            f_flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI,
            aborted: 0,
            name_mappings: std::ptr::null_mut(),
            progress_title: std::ptr::null(),
        };

        // SAFETY: `op` is a fully initialised SHFILEOPSTRUCTW whose `p_from`
        // points at a double-NUL terminated UTF-16 buffer that outlives the
        // call; the pointer fields are null, which the API accepts for a
        // one-item delete.
        let code = unsafe { SHFileOperationW(&mut op) };
        if code == 0 && op.aborted == 0 {
            Ok(())
        } else if code == 0 {
            Err("删除被取消".to_string())
        } else {
            Err(format!("资源管理器返回错误码 0x{code:02X}"))
        }
    }

    #[repr(C)]
    struct FileTimeBits {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct SystemTimeBits {
        year: u16,
        month: u16,
        weekday: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    unsafe extern "system" {
        fn FileTimeToLocalFileTime(from: *const FileTimeBits, to: *mut FileTimeBits) -> i32;
        fn FileTimeToSystemTime(from: *const FileTimeBits, to: *mut SystemTimeBits) -> i32;
    }

    /// Seconds between 1601 (where FILETIME starts) and 1970 (where
    /// `SystemTime` starts).
    const EPOCH_GAP: u64 = 11_644_473_600;

    pub fn local_time(time: SystemTime) -> Option<String> {
        let since = time.duration_since(SystemTime::UNIX_EPOCH).ok()?;
        let ticks = (since.as_secs() + EPOCH_GAP)
            .checked_mul(10_000_000)?
            .checked_add(since.subsec_nanos() as u64 / 100)?;
        let file_time = FileTimeBits {
            low: ticks as u32,
            high: (ticks >> 32) as u32,
        };
        let mut local = FileTimeBits { low: 0, high: 0 };
        let mut parts = SystemTimeBits {
            year: 0,
            month: 0,
            weekday: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        // SAFETY: both calls take pointers to structs this function owns and
        // fully initialises, and return non-zero on success.
        unsafe {
            if FileTimeToLocalFileTime(&file_time, &mut local) == 0 {
                return None;
            }
            if FileTimeToSystemTime(&local, &mut parts) == 0 {
                return None;
            }
        }
        Some(format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            parts.year, parts.month, parts.day, parts.hour, parts.minute
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn sizes_read_as_a_person_would_say_them() {
        assert_eq!(format_size(0), "0 字节");
        assert_eq!(format_size(1023), "1023 字节");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
