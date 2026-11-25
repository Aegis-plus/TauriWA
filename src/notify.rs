//! Desktop notifications through org.freedesktop.Notifications.
//!
//! The page's `Notification` objects (see `web/bridge.js`) are mapped to desktop
//! notifications; a click on one is forwarded back to the page so WhatsApp opens
//! the right chat.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use zbus::blocking::{Connection, Proxy as DbusProxy};
use zbus::zvariant::Value;

use crate::events::{send, Proxy, UserEvent};
use crate::util;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub struct Notification {
    /// id of the page's Notification object, None for our own notifications
    pub js_id: Option<u64>,
    pub title: String,
    pub body: String,
    pub tag: Option<String>,
    pub silent: bool,
}

enum Request {
    Show(Notification),
    Close(u64),
}

#[derive(Default)]
struct State {
    /// desktop notification id -> page Notification id
    by_server_id: HashMap<u32, Option<u64>>,
    by_js_id: HashMap<u64, u32>,
    by_tag: HashMap<String, u32>,
    activation_tokens: HashMap<u32, String>,
}

#[derive(Clone)]
pub struct Notifier {
    tx: Option<mpsc::Sender<Request>>,
}

impl Notifier {
    pub fn new(conn: Option<Connection>, proxy: Proxy, app_icon: String) -> Self {
        let Some(conn) = conn else {
            log::warn!("notifications disabled: no session bus");
            return Self { tx: None };
        };
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = mpsc::channel();

        listen(&conn, "ActivationToken", {
            let state = state.clone();
            move |msg| {
                if let Ok((id, token)) = msg.body().deserialize::<(u32, String)>() {
                    state.lock().unwrap().activation_tokens.insert(id, token);
                }
            }
        });
        listen(&conn, "ActionInvoked", {
            let state = state.clone();
            let proxy = proxy.clone();
            move |msg| {
                let Ok((id, action)) = msg.body().deserialize::<(u32, String)>() else {
                    return;
                };
                let mut state = state.lock().unwrap();
                let Some(js_id) = state.by_server_id.get(&id).copied() else {
                    return; // someone else's notification
                };
                log::debug!("notification {id} action {action}");
                let activation_token = state.activation_tokens.remove(&id);
                send(
                    &proxy,
                    UserEvent::NotificationClicked {
                        js_id,
                        activation_token,
                    },
                );
            }
        });
        listen(&conn, "NotificationClosed", {
            let state = state.clone();
            let proxy = proxy.clone();
            move |msg| {
                let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>() else {
                    return;
                };
                let mut state = state.lock().unwrap();
                state.activation_tokens.remove(&id);
                state.by_tag.retain(|_, v| *v != id);
                if let Some(Some(js_id)) = state.by_server_id.remove(&id) {
                    state.by_js_id.remove(&js_id);
                    send(&proxy, UserEvent::NotificationClosed { js_id });
                }
            }
        });

        std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || worker(conn, rx, state, app_icon))
            .expect("spawn notifications thread");

        Self { tx: Some(tx) }
    }

    pub fn show(&self, notification: Notification) {
        match &self.tx {
            Some(tx) => {
                let _ = tx.send(Request::Show(notification));
            }
            None => log::info!(
                "notification: {} - {}",
                notification.title,
                notification.body
            ),
        }
    }

    /// Simple notification from the app itself, a click shows the window
    pub fn simple(&self, body: impl Into<String>) {
        self.show(Notification {
            js_id: None,
            title: util::APP_NAME.to_owned(),
            body: body.into(),
            tag: None,
            silent: false,
        });
    }

    pub fn close(&self, js_id: u64) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request::Close(js_id));
        }
    }
}

fn listen(
    conn: &Connection,
    signal: &'static str,
    handler: impl Fn(zbus::Message) + Send + 'static,
) {
    let conn = conn.clone();
    std::thread::Builder::new()
        .name(format!("notif-{signal}"))
        .spawn(move || {
            let proxy = match DbusProxy::new(&conn, DEST, PATH, DEST) {
                Ok(p) => p,
                Err(err) => return log::error!("notifications: {err}"),
            };
            match proxy.receive_signal(signal) {
                Ok(signals) => signals.for_each(handler),
                Err(err) => log::error!("notifications: {signal}: {err}"),
            }
        })
        .expect("spawn notifications listener");
}

fn worker(
    conn: Connection,
    rx: mpsc::Receiver<Request>,
    state: Arc<Mutex<State>>,
    app_icon: String,
) {
    let proxy = match DbusProxy::new(&conn, DEST, PATH, DEST) {
        Ok(p) => p,
        Err(err) => return log::error!("notifications: {err}"),
    };
    let desktop_entry = util::desktop_file_id()
        .trim_end_matches(".desktop")
        .to_owned();
    for request in rx {
        match request {
            Request::Show(n) => {
                let replaces_id = n
                    .tag
                    .as_ref()
                    .and_then(|tag| state.lock().unwrap().by_tag.get(tag).copied())
                    .unwrap_or(0);
                let mut hints: HashMap<&str, Value> = HashMap::new();
                hints.insert("desktop-entry", Value::from(desktop_entry.as_str()));
                hints.insert("category", Value::from("im.received"));
                if n.silent {
                    hints.insert("suppress-sound", Value::from(true));
                }
                let actions = vec!["default", "Open"];
                let result: zbus::Result<u32> = proxy.call(
                    "Notify",
                    &(
                        util::APP_NAME,
                        replaces_id,
                        app_icon.as_str(),
                        &n.title,
                        &n.body,
                        actions,
                        hints,
                        -1i32,
                    ),
                );
                match result {
                    Ok(id) => {
                        let mut state = state.lock().unwrap();
                        state.by_server_id.insert(id, n.js_id);
                        if let Some(js_id) = n.js_id {
                            state.by_js_id.insert(js_id, id);
                        }
                        if let Some(tag) = n.tag {
                            state.by_tag.insert(tag, id);
                        }
                    }
                    Err(err) => log::error!("notification: {err}"),
                }
            }
            Request::Close(js_id) => {
                let id = state.lock().unwrap().by_js_id.remove(&js_id);
                if let Some(id) = id {
                    let result: zbus::Result<()> = proxy.call("CloseNotification", &(id,));
                    if let Err(err) = result {
                        log::debug!("close notification: {err}");
                    }
                }
            }
        }
    }
}
