//! Default keyboard shortcuts, handled in the page by `web/injected.js`.
//!
//! A binding is `"<modifiers> <key>"` (modifiers: C=Ctrl, S=Shift, A=Alt, M=Meta),
//! or just `"<key>"`, `key` being a KeyboardEvent.key value.
//! An effect is `{ "whatsappAction": "..." }` (WhatsApp Web's own keyboard commands),
//! `{ "action": "OPEN_NTH_CHAT", "chatIndex": n }`, `{ "action": "SLEEP", "duration": ms }`,
//! `{ "action": "IPC", "method": "...", "args": [...] }` (see `tauriwa` in `web/bridge.js`),
//! or an array of effects run in order.

use serde_json::{json, Map, Value};

pub fn default_keys() -> Map<String, Value> {
    let mut keys = json!({
        "A ArrowDown": { "whatsappAction": "GO_TO_NEXT_CHAT" },
        "A ArrowUp": { "whatsappAction": "GO_TO_PREV_CHAT" },
        "C Tab": { "whatsappAction": "GO_TO_NEXT_CHAT" },
        "CS Tab": { "whatsappAction": "GO_TO_PREV_CHAT" },
        "C u": { "whatsappAction": "TOGGLE_UNREAD" },
        "C ArrowUp": { "whatsappAction": "EDIT_LAST_MESSAGE" },
        "A l": [
            { "action": "IPC", "method": "windowToggle" },
            { "whatsappAction": "LOCK_SCREEN" }
        ],

        // app-level shortcuts (reload, quit, zoom, fullscreen, developer tools)
        "C r": { "action": "IPC", "method": "reload" },
        "C q": { "action": "IPC", "method": "quit" },
        "C =": { "action": "IPC", "method": "zoomIn" },
        "CS +": { "action": "IPC", "method": "zoomIn" },
        "C -": { "action": "IPC", "method": "zoomOut" },
        "F11": { "action": "IPC", "method": "fullscreenToggle" },
        "CS I": { "action": "IPC", "method": "devTools" }
    });
    let map = keys.as_object_mut().expect("object");
    for (index, key) in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]
        .iter()
        .enumerate()
    {
        let effect = json!({ "action": "OPEN_NTH_CHAT", "chatIndex": index });
        map.insert(format!("A {key}"), effect.clone());
        map.insert(format!("C {key}"), effect);
    }
    map.clone()
}

/// Defaults overridden by the "keys" config entry
pub fn merged_keys(user: Option<&Value>) -> Map<String, Value> {
    let mut keys = default_keys();
    if let Some(Value::Object(user)) = user {
        for (k, v) in user {
            keys.insert(k.clone(), v.clone());
        }
    }
    keys
}
