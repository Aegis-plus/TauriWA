use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub const APP_NAME: &str = "tauriwa";
pub const PRODUCT_NAME: &str = "TauriWA";
pub const DESKTOP_NAME: &str = "tauriwa.desktop";
pub const URL_SCHEME: &str = "whatsapp";
pub const DEFAULT_URL: &str = "https://web.whatsapp.com/";

/// "156.0.8078.100", the version of the bundled Chromium
pub fn chromium_version() -> &'static str {
    // b"156.0.31+ge603d97+chromium-156.0.8078.100\0"
    let cef = std::str::from_utf8(cef::sys::CEF_VERSION).unwrap_or("");
    cef.trim_end_matches('\0')
        .rsplit("chromium-")
        .next()
        .unwrap_or("")
}

/// The user agent of the Chrome release matching the bundled Chromium
pub fn chrome_user_agent() -> String {
    format!(
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
        cef::sys::CHROME_VERSION_MAJOR
    )
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn is_debug() -> bool {
    env("DEBUG").as_deref() == Some("1")
}

pub fn is_snap() -> bool {
    env("SNAP").is_some()
}

pub fn flatpak_id() -> Option<String> {
    env("FLATPAK_ID")
}

pub fn appimage() -> Option<String> {
    env("APPIMAGE")
}

/// GNOME Shell doesn't show a tray icon without a (rarely installed) extension,
/// so hiding to tray on close would leave the app with no way to reopen it.
pub fn default_quit_on_close() -> bool {
    env("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase()
        .contains("gnome")
}

/// A binary run from a cargo `target/` directory
pub fn is_dev_build() -> bool {
    cfg!(debug_assertions)
        || std::env::current_exe()
            .map(|exe| exe.components().any(|c| c.as_os_str() == "target"))
            .unwrap_or(false)
}

/// How the app was installed, as far as we can tell from the environment
pub fn packaging() -> &'static str {
    if is_snap() {
        "snap"
    } else if flatpak_id().is_some() {
        "flatpak"
    } else if appimage().is_some() {
        "appimage"
    } else if is_dev_build() {
        "dev"
    } else {
        "packaged"
    }
}

pub fn session_type() -> Option<String> {
    env("XDG_SESSION_TYPE")
}

/// "2.0.0 (webkitgtk 2.48.1, wayland, packaged)"
pub fn version_info() -> String {
    let mut details = vec![format!("chromium {}", chromium_version())];
    if let Some(session) = session_type() {
        details.push(session);
    }
    details.push(packaging().to_owned());
    format!("{} ({})", env!("CARGO_PKG_VERSION"), details.join(", "))
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/.config/tauriwa`: config.json, persistent-state.json, user-scripts/, user-icons/
pub fn user_data_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        dirs::config_dir()
            .unwrap_or_else(|| home_dir().join(".config"))
            .join(APP_NAME)
    })
}

/// `~/.local/share/tauriwa/chromium`: the Chromium profile (cookies, local
/// storage, IndexedDB, caches)
pub fn webview_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home_dir().join(".local/share"))
        .join(APP_NAME)
        .join("chromium")
}

/// Chromium's spellchecker settings, written to the profile's Preferences
/// file before Chromium starts (it reads it at launch). Chromium downloads
/// the dictionaries itself on first use.
pub fn set_spellcheck_preferences(profile: &Path, enabled: bool, languages: &[String]) {
    let file = profile.join("Default").join("Preferences");
    let mut prefs: serde_json::Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let before = prefs.clone();
    fn object<'a>(
        v: &'a mut serde_json::Value,
        key: &str,
    ) -> &'a mut serde_json::Map<String, serde_json::Value> {
        let entry = v
            .as_object_mut()
            .expect("object")
            .entry(key)
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        entry.as_object_mut().expect("object")
    }
    object(&mut prefs, "browser").insert(
        "enable_spellchecking".to_owned(),
        serde_json::Value::Bool(enabled),
    );
    if enabled {
        object(&mut prefs, "spellcheck")
            .insert("dictionaries".to_owned(), serde_json::json!(languages));
    }
    if prefs == before {
        return;
    }
    let written = file
        .parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|_| std::fs::write(&file, prefs.to_string()));
    if let Err(err) = written {
        log::warn!("spellcheck: {}: {err}", file.display());
    }
}

pub fn downloads_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(home_dir)
}

/// `dir/name`, or `dir/name (1).ext`... when it already exists
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    // no directories in a name chosen by a web page
    let name = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| n != "." && n != "..")
        .unwrap_or_else(|| "download".to_owned());
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    (1..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

/// `~/.cache/tauriwa`
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| home_dir().join(".cache"))
        .join(APP_NAME)
}

/// First run: copy the settings of ElecWhat (`~/.config/elecwhat`), the app TauriWA was ported from.
/// The WhatsApp session itself can't be carried over (different browser engine).
pub fn migrate_from_elecwhat() {
    let new_dir = user_data_dir();
    let old_dir = new_dir.with_file_name("elecwhat");
    if new_dir.exists() || !old_dir.join("config.json").exists() {
        return;
    }
    let result = std::fs::create_dir_all(new_dir).and_then(|_| {
        for name in ["config.json", "user-scripts", "user-icons"] {
            let from = old_dir.join(name);
            if from.exists() {
                copy_recursive(&from, &new_dir.join(name))?;
            }
        }
        Ok(())
    });
    match result {
        Ok(()) => eprintln!("tauriwa: imported settings from {}", old_dir.display()),
        Err(err) => eprintln!(
            "tauriwa: cannot import settings from {}: {err}",
            old_dir.display()
        ),
    }
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

pub fn replace_variables(s: &str) -> String {
    s.replace("${userData}", &user_data_dir().to_string_lossy())
}

/// Lexically normalize a path (like node's `path.resolve`, without touching the filesystem)
pub fn resolve_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `.desktop` file the desktop environment knows us by (Unity launcher API, notifications)
pub fn desktop_file_id() -> String {
    if let Some(hint) = env("BAMF_DESKTOP_FILE_HINT") {
        if let Some(name) = Path::new(&hint).file_name() {
            return name.to_string_lossy().into_owned();
        }
    }
    if let Some(id) = flatpak_id() {
        return format!("{id}.desktop");
    }
    if is_snap() {
        return "tauriwa_tauriwa.desktop".to_owned();
    }
    DESKTOP_NAME.to_owned()
}

/// `whatsapp://send/?phone=...` -> `https://web.whatsapp.com/send/?phone=...`
/// ("send" is parsed as the host, not the path)
pub fn convert_whatsapp_url(url: &str) -> Option<String> {
    if !url.starts_with(&format!("{URL_SCHEME}:")) {
        return None;
    }
    match url::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("");
            let mut web = format!("https://web.whatsapp.com/{host}{}", parsed.path());
            if let Some(query) = parsed.query() {
                web.push('?');
                web.push_str(query);
            }
            if let Some(fragment) = parsed.fragment() {
                web.push('#');
                web.push_str(fragment);
            }
            log::info!("Converting whatsapp:// URL: {url} -> {web}");
            Some(web)
        }
        Err(err) => {
            log::error!("Failed to convert whatsapp URL: {url}: {err}");
            None
        }
    }
}

pub fn unread_count_from_favicon(favicon_url: &str) -> i64 {
    const PREFIX: &str = "https://web.whatsapp.com/favicon/1x/f";
    favicon_url
        .strip_prefix(PREFIX)
        .and_then(|rest| {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() || !rest[digits.len()..].starts_with('/') {
                None
            } else {
                digits.parse().ok()
            }
        })
        .unwrap_or(0)
}

/// Binary units, like byte-size's "iec" units: "1.5 MiB"
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// Content of a `file://` (`file://~/x.css` is relative to the home directory) or `https://` URL
pub fn load_url(url: &url::Url) -> Result<String, String> {
    match url.scheme() {
        "file" => {
            let path = if url.host_str() == Some("~") {
                home_dir().join(url.path().trim_start_matches('/'))
            } else {
                url.to_file_path()
                    .map_err(|_| format!("invalid file URL {url}"))?
            };
            log::debug!("load from file {}", path.display());
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
        }
        "https" => {
            log::debug!("load from {url}");
            http_agent()
                .get(url.as_str())
                .call()
                .map_err(|e| e.to_string())?
                .body_mut()
                .read_to_string()
                .map_err(|e| e.to_string())
        }
        other => Err(format!("unsupported protocol {other}:")),
    }
}

/// Open a link in the user's browser
pub fn open_external(url: &str) {
    log::debug!("open external {url}");
    let url = url.to_owned();
    std::thread::spawn(move || {
        let result = std::process::Command::new("xdg-open").arg(&url).status();
        match result {
            Ok(status) if status.success() => {}
            Ok(status) => log::error!("xdg-open {url}: {status}"),
            Err(err) => log::error!("xdg-open {url}: {err}"),
        }
    });
}

/// Make this the default handler of whatsapp: links (`xdg-mime`)
pub fn register_url_scheme() {
    std::thread::spawn(|| {
        let mime = format!("x-scheme-handler/{URL_SCHEME}");
        let current = std::process::Command::new("xdg-mime")
            .args(["query", "default", &mime])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        let desktop = desktop_file_id();
        if current == desktop {
            return;
        }
        log::info!("Registering as default protocol client for {URL_SCHEME}://");
        if let Err(err) = std::process::Command::new("xdg-mime")
            .args(["default", &desktop, &mime])
            .status()
        {
            log::warn!("xdg-mime: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whatsapp_urls() {
        assert_eq!(
            convert_whatsapp_url("whatsapp://send/?phone=123&text=hi").as_deref(),
            Some("https://web.whatsapp.com/send/?phone=123&text=hi")
        );
        assert_eq!(
            convert_whatsapp_url("whatsapp://send?phone=123").as_deref(),
            Some("https://web.whatsapp.com/send?phone=123")
        );
        assert_eq!(convert_whatsapp_url("https://example.com"), None);
    }

    #[test]
    fn unread() {
        assert_eq!(
            unread_count_from_favicon("https://web.whatsapp.com/favicon/1x/f05/v4/"),
            5
        );
        assert_eq!(
            unread_count_from_favicon("https://web.whatsapp.com/favicon/1x/f12/v4/"),
            12
        );
        assert_eq!(
            unread_count_from_favicon("https://web.whatsapp.com/favicon/1x/favicon/v4/"),
            0
        );
        assert_eq!(
            unread_count_from_favicon("https://static.whatsapp.net/rsrc.php/favicon.png"),
            0
        );
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn tilde_file_url() {
        let url = url::Url::parse("file://~/mysheet.css").unwrap();
        assert_eq!(url.host_str(), Some("~"));
        assert_eq!(url.path(), "/mysheet.css");
        // raw css is not a URL
        assert!(url::Url::parse("span { font-size: small !important; }").is_err());
    }

    #[test]
    fn spellcheck_preferences() {
        let dir = std::env::temp_dir().join(format!("tauriwa-prefs-{}", std::process::id()));
        let file = dir.join("Default/Preferences");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{"browser":{"other":1},"spellcheck":"broken","keep":true}"#,
        )
        .unwrap();
        set_spellcheck_preferences(&dir, true, &["fr".to_owned(), "en-US".to_owned()]);
        let prefs: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(prefs["browser"]["enable_spellchecking"], true);
        assert_eq!(prefs["browser"]["other"], 1);
        assert_eq!(
            prefs["spellcheck"]["dictionaries"],
            serde_json::json!(["fr", "en-US"])
        );
        assert_eq!(prefs["keep"], true);
        set_spellcheck_preferences(&dir, false, &[]);
        let prefs: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(prefs["browser"]["enable_spellchecking"], false);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unique_paths() {
        let dir = std::env::temp_dir().join(format!("tauriwa-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.jpg"), dir.join("a.jpg"));
        std::fs::write(dir.join("a.jpg"), "").unwrap();
        assert_eq!(unique_path(&dir, "a.jpg"), dir.join("a (1).jpg"));
        assert_eq!(unique_path(&dir, "../../etc/passwd"), dir.join("passwd"));
        assert_eq!(unique_path(&dir, ".."), dir.join("download"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chromium() {
        assert!(chromium_version().starts_with(&cef::sys::CHROME_VERSION_MAJOR.to_string()));
        assert!(chrome_user_agent().contains("Chrome/"));
    }

    #[test]
    fn resolve() {
        assert_eq!(
            resolve_path(Path::new("/a/b/../c/./d.js")),
            PathBuf::from("/a/c/d.js")
        );
    }
}
