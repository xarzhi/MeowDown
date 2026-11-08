//! Application settings and the image-host upload pipeline.
//!
//! Settings live in a small JSON file under the user config directory, so a
//! restart keeps the image-host token and the path strategy.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which image host to talk to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageHost {
    /// PicGo desktop app's local HTTP server, no token required.
    PicGo,
    /// sm.ms / sm.ms-compatible endpoint using the `smfile` field.
    SmMs,
    /// Any endpoint that accepts a multipart upload and returns JSON.
    Custom,
}

impl ImageHost {
    pub fn label(self) -> &'static str {
        match self {
            Self::PicGo => "PicGo 本地服务",
            Self::SmMs => "SM.MS",
            Self::Custom => "自定义",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::PicGo => "先启动 PicGo 并开启「PicGo 设置 → 服务端」，默认端口 36677。",
            Self::SmMs => "在 sm.ms 个人中心生成 API Token，填入下方。",
            Self::Custom => "按表单 POST 一个文件，再从 JSON 响应里取图片地址。",
        }
    }
}

/// How a local image path is written into the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMode {
    /// Copy next to the document into `assets/` and write a relative path.
    Relative,
    /// Write the absolute path of the picked file.
    Absolute,
}

impl ImageMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Relative => "复制到 assets 目录",
            Self::Absolute => "使用绝对路径",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Write the document back to disk a moment after typing stops.
    pub auto_save: bool,
    /// Upload every inserted local image and store the remote URL instead.
    pub auto_upload: bool,
    pub host: ImageHost,
    pub endpoint: String,
    pub token: String,
    /// Multipart form field name.
    pub field: String,
    /// Dotted path to the URL inside the JSON response, e.g. `data.url`.
    pub url_path: String,
    pub image_mode: ImageMode,
    /// Folder name created next to the document for relative images.
    pub assets_dir: String,
    /// Where PicGo is installed. Empty means "look for it", which is what an
    /// upload does when the local service is not answering.
    pub picgo_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_save: false,
            auto_upload: false,
            host: ImageHost::PicGo,
            endpoint: "http://127.0.0.1:36677/upload".into(),
            token: String::new(),
            field: "file".into(),
            url_path: "result[0]".into(),
            image_mode: ImageMode::Relative,
            assets_dir: "assets".into(),
            picgo_path: String::new(),
        }
    }
}

impl Settings {
    /// Fill in the endpoint and response path that belong to the chosen host.
    ///
    /// Only the custom host lets the user edit these, so the built-in hosts
    /// always snap back to their fixed values.
    pub fn apply_host_defaults(&mut self) {
        match self.host {
            ImageHost::PicGo => {
                self.endpoint = "http://127.0.0.1:36677/upload".into();
                self.field = "file".into();
                self.url_path = "result[0]".into();
            }
            ImageHost::SmMs => {
                self.endpoint = "https://sm.ms/api/v2/upload".into();
                self.field = "smfile".into();
                self.url_path = "data.url".into();
            }
            ImageHost::Custom => {
                if self.url_path.trim().is_empty() {
                    self.url_path = "data.url".into();
                }
            }
        }
    }
}

/// The folder MeowDown keeps its own small files in:
/// `%APPDATA%\MeowDown` on Windows, the usual places elsewhere.
pub fn config_dir() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("MeowDown")
}

/// `%APPDATA%\MeowDown\settings.json` on Windows, the usual places elsewhere.
pub fn config_path() -> PathBuf {
    config_dir().join("settings.json")
}

/// A marker file whose existence means the bundled sample document has
/// already had its one showing.
///
/// The sample is a greeting, not a landing page: it teaches the shortcuts once
/// and then gets out of the way, because a reader who opens the program to
/// write something does not want a file they never asked for in front of the
/// caret. A file is used rather than a settings field so that it survives a
/// settings file the user never touched — and so that erasing it brings the
/// greeting back.
fn welcome_flag(dir: &Path) -> PathBuf {
    dir.join("welcome-shown")
}

/// True on the first launch, false on every launch after it.
///
/// The marker is written best-effort. A config folder that refuses the write
/// leaves nowhere to remember the greeting, so it comes back next time — which
/// is still better than a program that will not start because it could not
/// write a file it only wanted for a courtesy.
pub fn claim_welcome() -> bool {
    claim_welcome_in(&config_dir())
}

fn claim_welcome_in(dir: &Path) -> bool {
    let flag = welcome_flag(dir);
    if flag.exists() {
        return false;
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&flag, b"1");
    true
}

impl Settings {
    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(config_path()) else {
            return Self::default();
        };
        let mut settings: Self = serde_json::from_str(&text).unwrap_or_default();
        settings.apply_host_defaults();
        settings
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|err| err.to_string())?;
        std::fs::write(&path, text).map_err(|err| err.to_string())
    }
}

// ------------------------------------------------------------------- PicGo
//
// PicGo is a desktop app, not a service: the upload endpoint only answers while
// the program is running and its "server" switch is on. So an upload that finds
// the port closed starts the program, waits for the port, and only then posts —
// which is what "上传失败：Connection refused" used to be about.

/// Where PicGo installs itself when the user takes the default.
fn picgo_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        out.push(PathBuf::from(&local).join("Programs").join("PicGo").join("PicGo.exe"));
        out.push(PathBuf::from(&local).join("PicGo").join("PicGo.exe"));
    }
    for key in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(key) {
            out.push(PathBuf::from(&dir).join("PicGo").join("PicGo.exe"));
        }
    }
    out
}

/// Look through the registry for an install that went somewhere else.
#[cfg(target_os = "windows")]
fn picgo_from_registry() -> Option<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;

    // `App Paths` is the launcher index the shell itself uses.
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let key = RegKey::predef(root)
            .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\PicGo.exe");
        if let Ok(key) = key {
            if let Ok(value) = key.get_value::<String, _>("") {
                let path = PathBuf::from(value.trim().trim_matches('"'));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }

    // The uninstall list carries the folder the program was put in.
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let uninstall = RegKey::predef(root)
            .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall");
        let Ok(uninstall) = uninstall else {
            continue;
        };
        for name in uninstall.enum_keys().flatten() {
            let Ok(entry) = uninstall.open_subkey(&name) else {
                continue;
            };
            let display = entry.get_value::<String, _>("DisplayName").unwrap_or_default();
            if !display.contains("PicGo") {
                continue;
            }
            if let Ok(icon) = entry.get_value::<String, _>("DisplayIcon") {
                // `"C:\…\PicGo.exe",0` — keep the path, drop the icon index.
                let cleaned = icon.trim().trim_matches('"');
                let cleaned = cleaned.split(',').next().unwrap_or(cleaned);
                let path = PathBuf::from(cleaned.trim_matches('"'));
                if path.is_file() {
                    return Some(path);
                }
            }
            if let Ok(location) = entry.get_value::<String, _>("InstallLocation") {
                let path = PathBuf::from(location.trim().trim_matches('"')).join("PicGo.exe");
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Find PicGo: the saved path first, then the usual folders, then the registry.
pub fn detect_picgo(saved: &str) -> Option<PathBuf> {
    let path = PathBuf::from(saved.trim());
    if !saved.trim().is_empty() && path.is_file() {
        return Some(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(found) = picgo_from_registry() {
        return Some(found);
    }
    picgo_candidates().into_iter().find(|path| path.is_file())
}

/// The port its HTTP service listens on, read off the upload address.
pub fn picgo_port(endpoint: &str) -> u16 {
    let rest = endpoint.trim();
    let rest = rest
        .strip_prefix("http://")
        .or_else(|| rest.strip_prefix("https://"))
        .unwrap_or(rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    authority
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(36677)
}

/// How long a launched PicGo gets to bring its server up.
const PICGO_START_BUDGET: std::time::Duration = std::time::Duration::from_secs(25);

fn port_open(port: u16) -> bool {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpStream::connect_timeout(&address, std::time::Duration::from_millis(200)).is_ok()
}

/// Make sure PicGo's server is up, starting the program if it has to.
///
/// Blocks for up to half a minute, so callers keep it on a background thread.
pub fn ensure_picgo(settings: &Settings) -> Result<(), String> {
    let port = picgo_port(&settings.endpoint);
    if port_open(port) {
        return Ok(());
    }
    let Some(path) = detect_picgo(&settings.picgo_path) else {
        return Err(
            "没有找到 PicGo：请在「设置 → 图片与图床」里指定 PicGo.exe 的位置".to_string(),
        );
    };
    std::process::Command::new(&path)
        .spawn()
        .map_err(|err| format!("启动 PicGo 失败：{err}"))?;
    // A cold start is slow on a spinning disk; a warm one takes barely a second.
    // The budget is a deadline rather than a number of tries: when the firewall
    // drops the probe instead of refusing it, every attempt costs the whole
    // connect timeout, and counting tries turned a 20-second wait into ninety.
    let deadline = std::time::Instant::now() + PICGO_START_BUDGET;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if port_open(port) {
            return Ok(());
        }
    }
    Err(format!(
        "已启动 PicGo（{}），但端口 {port} 没有响应：请在 PicGo 里打开「设置 → 服务端」",
        path.display()
    ))
}

/// Copy `source` into `doc_dir/folder` and return the relative path to write
/// into the document. Existing files are never overwritten.
pub fn copy_image_into(doc_dir: &Path, folder: &str, source: &Path, stem: &str) -> Option<String> {
    let folder = folder.trim().trim_matches('/');
    let folder = if folder.is_empty() { "assets" } else { folder };
    let dir = doc_dir.join(folder);
    std::fs::create_dir_all(&dir).ok()?;

    let ext = source
        .extension()
        .map(|ext| ext.to_string_lossy().to_string())
        .unwrap_or_else(|| "png".to_string());
    let stem = if stem.trim().is_empty() { "image" } else { stem.trim() };

    let mut name = format!("{stem}.{ext}");
    let mut target = dir.join(&name);
    let mut counter = 1;
    while target.exists() {
        name = format!("{stem}-{counter}.{ext}");
        target = dir.join(&name);
        counter += 1;
        if counter > 999 {
            return None;
        }
    }

    std::fs::copy(source, &target).ok()?;
    Some(format!("{folder}/{name}"))
}

/// Upload one file and return its public URL.
///
/// This blocks on the network, so callers run it on a background executor.
pub fn upload_file(path: &Path, settings: &Settings) -> Result<String, String> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image.png".to_string());
    let bytes = std::fs::read(path).map_err(|err| format!("读取文件失败：{err}"))?;
    let mime = mime_for(path);

    match settings.host {
        ImageHost::PicGo => {
            // A desktop app that is not running answers nothing at all, which
            // reads as "上传失败" for no visible reason. Start it first.
            ensure_picgo(settings)?;
            let body = serde_json::json!({ "list": [path.to_string_lossy()] });
            let text = send(
                ureq::post(&settings.endpoint).set("Content-Type", "application/json"),
                Some(body.to_string().into_bytes()),
            )?;
            let value: Value = parse_json(&text)?;
            if value.get("success").and_then(Value::as_bool) == Some(false) {
                return Err(business_error(&value));
            }
            let url = value
                .pointer("/result/0")
                .or_else(|| value.pointer("/data/0"))
                .and_then(Value::as_str)
                .or_else(|| value.get("url").and_then(Value::as_str))
                .ok_or_else(|| format!("响应里没有图片地址：{text}"))?;
            Ok(url.to_string())
        }
        ImageHost::SmMs | ImageHost::Custom => {
            let (content_type, body) = multipart(&settings.field, &file_name, mime, &bytes);
            let mut request = ureq::post(&settings.endpoint).set("Content-Type", &content_type);
            if !settings.token.trim().is_empty() {
                let token = settings.token.trim();
                let header = if settings.host == ImageHost::SmMs {
                    token.to_string()
                } else if token.to_ascii_lowercase().starts_with("bearer ")
                    || token.contains(':')
                {
                    token.to_string()
                } else {
                    format!("Bearer {token}")
                };
                request = request.set("Authorization", &header);
            }
            let text = send(request, Some(body))?;
            let value: Value = parse_json(&text)?;
            if value.get("success").and_then(Value::as_bool) == Some(false) {
                return Err(business_error(&value));
            }
            let url = lookup(&value, &settings.url_path)
                .or_else(|| lookup(&value, "data.url"))
                .or_else(|| lookup(&value, "result[0]"))
                .ok_or_else(|| format!("在响应里找不到 {}：{text}", settings.url_path))?;
            Ok(url)
        }
    }
}

fn send(request: ureq::Request, body: Option<Vec<u8>>) -> Result<String, String> {
    let response = match body {
        Some(bytes) => request.send_bytes(&bytes),
        None => request.call(),
    };
    match response {
        Ok(response) => response
            .into_string()
            .map_err(|err| format!("读取响应失败：{err}")),
        Err(ureq::Error::Status(code, response)) => {
            let text = response.into_string().unwrap_or_default();
            Err(format!("HTTP {code}：{}", text.chars().take(200).collect::<String>()))
        }
        Err(err) => Err(format!("请求失败：{err}")),
    }
}

fn parse_json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|_| format!("响应不是 JSON：{}", text.chars().take(200).collect::<String>()))
}

fn business_error(value: &Value) -> String {
    for key in ["message", "msg", "error", "reason"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    format!("图床返回失败：{value}")
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Build a one-file multipart body without pulling in a form library.
fn multipart(field: &str, file_name: &str, mime: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("----MeowDownBoundary{}", std::process::id());
    let field = if field.trim().is_empty() { "file" } else { field.trim() };
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Read `a.b[0].c` style paths out of a JSON value.
fn lookup(value: &Value, path: &str) -> Option<String> {
    let normalized = path.replace('[', ".").replace(']', "");
    let mut current = value;
    for segment in normalized.split('.').filter(|part| !part.is_empty()) {
        current = match segment.parse::<usize>() {
            Ok(index) => current.get(index)?,
            Err(_) => current.get(segment)?,
        };
    }
    match current {
        Value::String(text) => Some(text.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dotted_json_paths() {
        let value: Value = serde_json::from_str(r#"{"result":["https://a/1.png"]}"#).unwrap();
        assert_eq!(lookup(&value, "result[0]").as_deref(), Some("https://a/1.png"));
        let value: Value = serde_json::from_str(r#"{"data":{"url":"https://b/2.png"}}"#).unwrap();
        assert_eq!(lookup(&value, "data.url").as_deref(), Some("https://b/2.png"));
    }

    /// The sample document is a greeting: one launch, then never again.
    #[test]
    fn the_sample_is_shown_once() {
        let dir = std::env::temp_dir().join(format!("meowdown-welcome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(claim_welcome_in(&dir), "首次启动应该显示示例");
        assert!(!claim_welcome_in(&dir), "再次启动不再显示示例");
        assert!(welcome_flag(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_settings() {
        let mut settings = Settings::default();
        settings.host = ImageHost::SmMs;
        settings.apply_host_defaults();
        assert_eq!(settings.field, "smfile");
        let text = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.url_path, "data.url");
        assert_eq!(back.image_mode, ImageMode::Relative);
    }

    #[test]
    fn reads_the_port_off_the_upload_address() {
        assert_eq!(picgo_port("http://127.0.0.1:36677/upload"), 36677);
        assert_eq!(picgo_port("http://127.0.0.1:1234"), 1234);
        // No port at all falls back to the PicGo default.
        assert_eq!(picgo_port("http://127.0.0.1/upload"), 36677);
        assert_eq!(picgo_port(""), 36677);
    }

    #[test]
    fn builds_a_multipart_body() {
        let (content_type, body) = multipart("file", "a.png", "image/png", b"abc");
        let text = String::from_utf8_lossy(&body);
        assert!(content_type.starts_with("multipart/form-data; boundary="));
        assert!(text.contains("name=\"file\"; filename=\"a.png\""));
        assert!(text.contains("abc"));
    }

    #[test]
    fn copies_images_without_overwriting() {
        let root = std::env::temp_dir().join(format!("meowdown-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("shot.png");
        std::fs::write(&source, b"png").unwrap();

        let first = copy_image_into(&root, "assets", &source, "shot").expect("first copy");
        assert_eq!(first, "assets/shot.png");
        let second = copy_image_into(&root, "assets", &source, "shot").expect("second copy");
        assert_eq!(second, "assets/shot-1.png");
        assert!(root.join("assets/shot.png").exists());
        assert!(root.join("assets/shot-1.png").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Run with the mock host from `tools/mock_host.py` listening on 36678.
    #[test]
    #[ignore]
    fn uploads_as_multipart_to_a_custom_host() {
        let root = std::env::temp_dir().join(format!("meowdown-upload2-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("demo.png");
        std::fs::write(&file, b"fake-png-bytes").unwrap();

        let mut settings = Settings::default();
        settings.host = ImageHost::Custom;
        settings.endpoint = "http://127.0.0.1:36678/upload".into();
        settings.field = "file".into();
        settings.url_path = "data.url".into();
        settings.token = "secret-token".into();
        let url = upload_file(&file, &settings).expect("upload");
        assert!(url.starts_with("https://cdn.example.com/uploaded/"), "{url}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Run with the mock host from `tools/mock_host.py` listening on 36678.
    #[test]
    #[ignore]
    fn uploads_through_a_picgo_style_endpoint() {
        let root = std::env::temp_dir().join(format!("meowdown-upload-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("demo.png");
        std::fs::write(&file, b"fake-png-bytes").unwrap();

        let mut settings = Settings::default();
        settings.host = ImageHost::PicGo;
        settings.endpoint = "http://127.0.0.1:36678/upload".into();
        let url = upload_file(&file, &settings).expect("upload");
        assert!(url.starts_with("https://cdn.example.com/uploaded/"), "{url}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
