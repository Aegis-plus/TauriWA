//! D-Bus service `io.github.aegis_plus.tauriwa`:
//! - single instance: a second `tauriwa` forwards its command line to the first one and exits
//! - lets other programs show/hide/toggle the window:
//!   `gdbus call --session --dest io.github.aegis_plus.tauriwa --object-path /io/github/aegis_plus/tauriwa --method io.github.aegis_plus.tauriwa.ToggleVisibility`
//! - dock unread count badge (Unity launcher API)

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use zbus::blocking::Connection;
use zbus::fdo::RequestNameFlags;
use zbus::fdo::RequestNameReply;
use zbus::interface;
use zbus::zvariant::Value;

use crate::events::{send, Proxy, UserEvent};
use crate::util;

pub const SERVICE_NAME: &str = "io.github.aegis_plus.tauriwa";
pub const OBJECT_PATH: &str = "/io/github/aegis_plus/tauriwa";

struct Service {
    proxy: Proxy,
    visible: Arc<AtomicBool>,
}

#[interface(name = "io.github.aegis_plus.tauriwa")]
impl Service {
    fn show(&self) {
        send(
            &self.proxy,
            UserEvent::Show {
                activation_token: None,
            },
        );
    }

    /// Same as Show, with an xdg-activation token so the window can be raised on Wayland
    fn activate(&self, activation_token: String) {
        let activation_token = Some(activation_token).filter(|t| !t.is_empty());
        send(&self.proxy, UserEvent::Show { activation_token });
    }

    fn hide(&self) {
        send(&self.proxy, UserEvent::Hide);
    }

    fn visible(&self) -> bool {
        self.visible.load(Ordering::SeqCst)
    }

    /// Returns the new visibility
    fn toggle_visibility(&self) -> bool {
        let visible = !self.visible.load(Ordering::SeqCst);
        send(&self.proxy, UserEvent::Toggle);
        visible
    }

    /// Open a whatsapp: or https://web.whatsapp.com/ URL
    fn open_url(&self, url: String) {
        send(&self.proxy, UserEvent::OpenUrl(url));
    }

    fn quit(&self) {
        send(&self.proxy, UserEvent::Quit);
    }
}

pub enum Instance {
    /// We own the name (or D-Bus is disabled/unavailable): run the app
    Primary,
    /// Another tauriwa is running and got our command line
    Secondary,
}

pub fn session_bus() -> Option<Connection> {
    match Connection::session() {
        Ok(conn) => Some(conn),
        Err(err) => {
            log::error!("DBus: not connected to session bus: {err}");
            None
        }
    }
}

/// Export the service and claim the name, or hand `command` over to the running instance
pub fn register(
    conn: &Connection,
    proxy: Proxy,
    visible: Arc<AtomicBool>,
    command: &Command,
) -> Instance {
    if let Err(err) = conn
        .object_server()
        .at(OBJECT_PATH, Service { proxy, visible })
    {
        log::error!("DBus: cannot export {OBJECT_PATH}: {err}");
        return Instance::Primary;
    }
    match conn.request_name_with_flags(SERVICE_NAME, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            log::info!("DBus registered: {SERVICE_NAME}");
            Instance::Primary
        }
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {
            forward(conn, command);
            Instance::Secondary
        }
        Err(err) => {
            log::error!("Error DBus: {err}");
            Instance::Primary
        }
    }
}

/// What the command line asks the app to do
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Show,
    Hide,
    Toggle,
    Quit,
    OpenUrl(String),
}

/// Is another instance owning the service name?
pub fn is_running(conn: &Connection) -> bool {
    match zbus::blocking::fdo::DBusProxy::new(conn) {
        Ok(proxy) => match zbus::names::BusName::try_from(SERVICE_NAME) {
            Ok(name) => proxy.name_has_owner(name).unwrap_or(false),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

pub fn forward(conn: &Connection, command: &Command) {
    log::info!("already running, forwarding {command:?}");
    let token = std::env::var("XDG_ACTIVATION_TOKEN").unwrap_or_default();
    let result = match command {
        Command::Show => conn.call_method(
            Some(SERVICE_NAME),
            OBJECT_PATH,
            Some(SERVICE_NAME),
            "Activate",
            &(token,),
        ),
        Command::Hide => conn.call_method(
            Some(SERVICE_NAME),
            OBJECT_PATH,
            Some(SERVICE_NAME),
            "Hide",
            &(),
        ),
        Command::Toggle => conn.call_method(
            Some(SERVICE_NAME),
            OBJECT_PATH,
            Some(SERVICE_NAME),
            "ToggleVisibility",
            &(),
        ),
        Command::Quit => conn.call_method(
            Some(SERVICE_NAME),
            OBJECT_PATH,
            Some(SERVICE_NAME),
            "Quit",
            &(),
        ),
        Command::OpenUrl(url) => conn.call_method(
            Some(SERVICE_NAME),
            OBJECT_PATH,
            Some(SERVICE_NAME),
            "OpenUrl",
            &(url.as_str(),),
        ),
    };
    if let Err(err) = result {
        log::error!("DBus: cannot reach the running instance: {err}");
    }
}

/// https://wiki.ubuntu.com/Unity/LauncherAPI
/// gnome dash-to-dock: https://github.com/micheleg/dash-to-dock/blob/b96a003bec758fc35efb0f4e5134030a921c418b/launcherAPI.js#L14-L21
/// kde plasma: https://github.com/KDE/plasma-desktop/commit/e284e9dc17051f22d05985e218fa44ddaba78de5
pub fn set_badge_count(conn: &Connection, count: i64) {
    let app_uri = format!("application://{}", util::desktop_file_id());
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("count", Value::from(count));
    props.insert("count-visible", Value::from(count != 0));
    if let Err(err) = conn.emit_signal(
        None::<&str>,
        "/",
        "com.canonical.Unity.LauncherEntry",
        "Update",
        &(app_uri, props),
    ) {
        log::warn!("DBus: badge: {err}");
    }
}
