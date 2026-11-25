//! Autostart on login via an XDG autostart entry:
//! https://specifications.freedesktop.org/autostart-spec/latest/
//!
//! The entry is a copy of the installed .desktop file when one can be found.
//! Otherwise (AppImage, dev, ...) the entry is generated.
//!
//! Not supported in a snap: the home plug can't write to the real ~/.config.
//! Not supported in a flatpak either: no access to ~/.config/autostart (would
//! need the org.freedesktop.portal.Background portal).

use std::path::{Path, PathBuf};

use crate::util::{self, DESKTOP_NAME};

/// Same entry as packaging/tauriwa.desktop, without the Exec field codes
pub const DESKTOP_ENTRY: &str = include_str!("../packaging/tauriwa.desktop");

pub fn supported() -> bool {
    !util::is_snap() && util::flatpak_id().is_none()
}

fn autostart_file() -> PathBuf {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| util::home_dir().join(".config"));
    config_home.join("autostart").join(DESKTOP_NAME)
}

/// .desktop file we were launched from, or the installed one
fn find_desktop_file() -> Option<PathBuf> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let mut candidates: Vec<PathBuf> = [
        env("GIO_LAUNCHED_DESKTOP_FILE"),
        env("BAMF_DESKTOP_FILE_HINT"),
    ]
    .into_iter()
    .flatten()
    .map(PathBuf::from)
    .collect();
    let data_home = env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| util::home_dir().join(".local/share"));
    let data_dirs =
        env("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    candidates.push(data_home.join("applications").join(DESKTOP_NAME));
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        candidates.push(Path::new(dir).join("applications").join(DESKTOP_NAME));
    }
    candidates.retain(|c| c.is_absolute());
    log::debug!("autostart: desktop file candidates {candidates:?}");
    let found = candidates.into_iter().find(|c| c.exists());
    match &found {
        Some(f) => log::info!("autostart: found desktop file {}", f.display()),
        None => log::info!("autostart: no installed desktop file found"),
    }
    found
}

/// Desktop entry Exec quoting:
/// https://specifications.freedesktop.org/desktop-entry-spec/latest/exec-variables.html
fn quote_exec_arg(arg: &str) -> String {
    let escaped = arg.replace('%', "%%");
    const SPECIAL: &[char] = &[
        '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
    ];
    if !escaped
        .chars()
        .any(|c| c.is_whitespace() || SPECIAL.contains(&c))
    {
        return escaped;
    }
    let mut out = String::from("\"");
    for c in escaped.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn exec_command() -> String {
    let exe = util::appimage()
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let exe = exe.unwrap_or_else(|| PathBuf::from(util::APP_NAME));
    quote_exec_arg(&exe.to_string_lossy())
}

/// Drop the Exec field codes (%U...): there is nothing to open at login.
/// Make sure the entry isn't disabled.
fn autostart_entry(desktop_file: &str, exec: Option<&str>) -> String {
    let mut in_main_group = false;
    let mut lines: Vec<String> = desktop_file
        .lines()
        .filter(|line| {
            !line.starts_with("X-GNOME-Autostart-enabled=") && !line.starts_with("Hidden=")
        })
        .map(|line| {
            if line.starts_with('[') {
                in_main_group = line.trim() == "[Desktop Entry]";
            } else if in_main_group && line.starts_with("Exec=") {
                if let Some(exec) = exec {
                    return format!("Exec={exec}");
                }
                return strip_field_codes(line);
            }
            line.to_owned()
        })
        .collect();
    let main = lines
        .iter()
        .position(|l| l == "[Desktop Entry]")
        .map(|i| i + 1)
        .unwrap_or(0);
    lines.insert(main, "X-GNOME-Autostart-enabled=true".to_owned());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn strip_field_codes(line: &str) -> String {
    let mut out = String::new();
    let words = line.split(' ').peekable();
    for word in words {
        let is_code =
            word.len() == 2 && word.starts_with('%') && "fFuUdDnNickvm".contains(&word[1..]);
        if !is_code {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out
}

fn entry_content() -> String {
    // Use an installed entry only when it can point to this very copy:
    // - an AppImage's path changes on update, an installed entry may point to another copy
    // - in dev, the installed entry would launch the system tauriwa, not this build
    let source = if util::appimage().is_some() {
        log::info!("autostart: AppImage, not looking for an installed desktop file");
        None
    } else if util::is_dev_build() {
        log::info!("autostart: dev, not looking for an installed desktop file");
        None
    } else {
        find_desktop_file()
    };
    if let Some(source) = source {
        match std::fs::read_to_string(&source) {
            Ok(text) => {
                log::info!("autostart: copying {}", source.display());
                return autostart_entry(&text, None);
            }
            Err(err) => log::warn!("autostart: can't read {}: {err}", source.display()),
        }
    }
    log::info!("autostart: generating entry");
    autostart_entry(DESKTOP_ENTRY, Some(&exec_command()))
}

pub fn is_enabled() -> bool {
    supported() && autostart_file().exists()
}

pub fn set(enabled: bool) {
    if !supported() {
        log::warn!("autostart: not supported");
        return;
    }
    let file = autostart_file();
    let result = if enabled {
        let content = entry_content();
        log::debug!("autostart: entry\n{content}");
        file.parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| std::fs::write(&file, content))
            .map(|_| log::info!("autostart enabled {}", file.display()))
    } else {
        match std::fs::remove_file(&file) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => {
                log::info!("autostart disabled {}", file.display());
                Ok(())
            }
        }
    };
    if let Err(err) = result {
        log::error!("autostart: {err}");
    }
}

/// The AppImage path changes when the file is moved or updated: keep an existing
/// entry pointing to the current one. Other entries are left alone so user edits
/// (e.g. extra arguments) survive.
pub fn refresh() {
    let enabled = is_enabled();
    log::debug!(
        "autostart: enabled {enabled} {}",
        autostart_file().display()
    );
    if enabled && util::appimage().is_some() {
        log::info!("autostart: refreshing entry");
        set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote_exec_arg("/usr/bin/tauriwa"), "/usr/bin/tauriwa");
        assert_eq!(
            quote_exec_arg("/home/a b/x.AppImage"),
            "\"/home/a b/x.AppImage\""
        );
        assert_eq!(quote_exec_arg("/a/100%"), "/a/100%%");
        assert_eq!(quote_exec_arg("/a/$x"), "\"/a/\\$x\"");
    }

    #[test]
    fn entry() {
        let e = autostart_entry(
            "[Desktop Entry]\nName=TauriWA\nExec=tauriwa %U\nHidden=true\n[Other]\nExec=x %U\n",
            None,
        );
        assert_eq!(e, "[Desktop Entry]\nX-GNOME-Autostart-enabled=true\nName=TauriWA\nExec=tauriwa\n[Other]\nExec=x %U\n");
        let g = autostart_entry(DESKTOP_ENTRY, Some("/opt/x"));
        assert!(g.contains("\nExec=/opt/x\n"), "{g}");
    }
}
