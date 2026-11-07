//! Downloading images that live on the web.
//!
//! The document usually points at a picture host once an upload finishes, and
//! `gpui`'s built-in loader does not pick those up here, so the bytes are
//! fetched over `ureq` and handed to the renderer as a decoded-ready image.

use gpui_kit::ImageFormat;

/// Fetch `url`, returning its bytes together with the format `gpui` needs to
/// decode them.
pub fn fetch(url: &str) -> Option<(Vec<u8>, ImageFormat)> {
    let response = ureq::get(url)
        .set("User-Agent", "MeowDown/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let format = sniff(&bytes, url)?;
    Some((bytes, format))
}

/// Identify the format from the magic bytes, falling back to the URL suffix.
fn sniff(bytes: &[u8], url: &str) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some(ImageFormat::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.starts_with(b"GIF8") {
        return Some(ImageFormat::Gif);
    }
    if bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    if bytes.starts_with(b"BM") {
        return Some(ImageFormat::Bmp);
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)])
        .trim_start()
        .to_string();
    if head.starts_with("<svg") || head.starts_with("<?xml") {
        return Some(ImageFormat::Svg);
    }
    let lower = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    for (suffix, format) in [
        (".png", ImageFormat::Png),
        (".jpg", ImageFormat::Jpeg),
        (".jpeg", ImageFormat::Jpeg),
        (".gif", ImageFormat::Gif),
        (".webp", ImageFormat::Webp),
        (".bmp", ImageFormat::Bmp),
        (".svg", ImageFormat::Svg),
    ] {
        if lower.ends_with(suffix) {
            return Some(format);
        }
    }
    None
}

/// A stable id for the image cache: FNV-1a over the bytes.
pub fn image_id(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes.iter().step_by(7) {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
