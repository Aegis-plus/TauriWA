//! Tray icons: WhatsApp Web's favicon (it shows the unread count), or the
//! user's own icons from `~/.config/tauriwa/user-icons/<name>.png`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

// rendered from packaging/tauriwa.svg by build.rs
pub const APP_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.png"));
pub const APP_512_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app-512.png"));

/// Pixels in the StatusNotifierItem format: ARGB32, network byte order
#[derive(Debug)]
pub struct RgbaIcon {
    pub width: i32,
    pub height: i32,
    pub argb: Vec<u8>,
}

impl RgbaIcon {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let image = match image::load_from_memory(bytes) {
            Ok(image) => image.into_rgba8(),
            Err(err) => {
                log::error!("cannot decode icon: {err}");
                return None;
            }
        };
        let (width, height) = image.dimensions();
        let mut argb = image.into_raw();
        for px in argb.chunks_exact_mut(4) {
            px.rotate_right(1); // RGBA -> ARGB
        }
        Some(Self {
            width: width as i32,
            height: height as i32,
            argb,
        })
    }
}

type Cache = Mutex<HashMap<String, Option<Arc<RgbaIcon>>>>;

fn user_icon_cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn icon_cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// "https://web.whatsapp.com/favicon/1x/f05/v4/" -> "f05"
pub fn icon_basename(url: &str) -> String {
    let mut s = url.to_owned();
    // drop a "v4/" style version component
    if let Some(start) = find_version_component(&s) {
        let end = s[start + 1..]
            .find('/')
            .map(|i| start + 1 + i + 1)
            .unwrap_or(s.len());
        s.replace_range(start..end, "");
    }
    let trimmed = s.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_owned()
}

fn find_version_component(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    (0..bytes.len()).find(|&i| {
        bytes[i] == b'v' && {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            digits > 0 && bytes.get(i + 1 + digits) == Some(&b'/')
        }
    })
}

pub fn user_icon(url: &str, icons_dir: &Path) -> Option<Arc<RgbaIcon>> {
    let basename = icon_basename(url);
    if let Some(cached) = user_icon_cache().lock().unwrap().get(&basename) {
        return cached.clone();
    }
    let file = icons_dir.join(format!("{basename}.png"));
    log::debug!("favicon {basename} {}", file.display());
    let icon = std::fs::read(&file)
        .ok()
        .and_then(|bytes| RgbaIcon::decode(&bytes))
        .map(Arc::new);
    user_icon_cache()
        .lock()
        .unwrap()
        .insert(basename, icon.clone());
    icon
}

/// Blocking: may download the icon
pub fn get_icon(url: &str, icons_dir: &Path, user_agent: &str) -> Option<Arc<RgbaIcon>> {
    if let Some(icon) = user_icon(url, icons_dir) {
        return Some(icon);
    }
    // larger images
    let url = url.replace("/1x/", "/2x/");
    if let Some(cached) = icon_cache().lock().unwrap().get(&url) {
        return cached.clone();
    }
    if !(url.starts_with("https://static.whatsapp.net/")
        || url.starts_with("https://web.whatsapp.com/"))
    {
        log::debug!("ignoring favicon {url}");
        return None;
    }
    log::debug!("fetch favicon {url}");
    let bytes = match crate::util::http_agent()
        .get(&url)
        .header("User-Agent", user_agent)
        .call()
    {
        Ok(mut response) => match response.body_mut().read_to_vec() {
            Ok(bytes) => bytes,
            Err(err) => {
                log::error!("fetch {url}: {err}");
                return None;
            }
        },
        Err(err) => {
            log::error!("fetch {url}: {err}");
            return None;
        }
    };
    let icon = RgbaIcon::decode(&bytes).map(Arc::new);
    if icon.is_some() {
        icon_cache().lock().unwrap().insert(url, icon.clone());
    }
    icon
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basenames() {
        assert_eq!(
            icon_basename("https://web.whatsapp.com/favicon/1x/f05/v4/"),
            "f05"
        );
        assert_eq!(
            icon_basename("https://web.whatsapp.com/favicon/1x/favicon/v4/"),
            "favicon"
        );
        assert_eq!(icon_basename("app"), "app");
    }

    #[test]
    fn decode_app_icon() {
        for png in [APP_PNG, APP_512_PNG] {
            let icon = RgbaIcon::decode(png).unwrap();
            assert_eq!(icon.argb.len(), (icon.width * icon.height * 4) as usize);
        }
    }
}
