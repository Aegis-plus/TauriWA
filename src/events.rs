use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cef::*;

use crate::icons::RgbaIcon;

/// Everything that is handled on CEF's UI thread, coming from other threads
/// (D-Bus calls, tray menu, notifications, downloads of icons/CSS) or from
/// CEF callbacks that must not re-enter the app state.
#[derive(Debug)]
pub enum UserEvent {
    Show {
        activation_token: Option<String>,
    },
    Hide,
    Toggle,
    Quit,
    /// whatsapp: link opened while running
    OpenUrl(String),
    /// message posted by the page (`web/bridge.js`)
    Ipc(String),
    BrowserCreated,
    PageLoaded,
    LoadFailed,
    /// retry loading the start page after a failure
    Retry,
    Title(String),
    /// the main window was moved or resized
    BoundsChanged,
    /// the window manager asked to close the main window
    CloseRequested,
    NotificationClicked {
        js_id: Option<u64>,
        activation_token: Option<String>,
    },
    NotificationClosed {
        js_id: u64,
    },
    Favicon {
        url: String,
        icon: Option<Arc<RgbaIcon>>,
    },
    Css(String),
    Reload,
    ClearCache,
    CacheCleared {
        size_before: u64,
    },
    SetQuitOnClose(bool),
    SetAutostart(bool),
    About,
    DownloadFinished {
        path: Option<PathBuf>,
        success: bool,
    },
}

/// Handle to send events to the UI thread, from any thread
#[derive(Clone, Debug, Default)]
pub struct Proxy;

static QUEUE: Mutex<VecDeque<UserEvent>> = Mutex::new(VecDeque::new());

/// The function the UI thread runs for each event (set once by the app)
static HANDLER: Mutex<Option<fn(UserEvent)>> = Mutex::new(None);

pub fn set_handler(handler: fn(UserEvent)) {
    *HANDLER.lock().unwrap() = Some(handler);
}

wrap_task! {
    struct DrainTask {}

    impl Task {
        fn execute(&self) {
            drain();
        }
    }
}

wrap_task! {
    struct DelayedSend {
        event: Arc<Mutex<Option<UserEvent>>>,
    }

    impl Task {
        fn execute(&self) {
            if let Some(event) = self.event.lock().unwrap().take() {
                send(&Proxy, event);
            }
        }
    }
}

fn drain() {
    let Some(handler) = *HANDLER.lock().unwrap() else {
        return;
    };
    loop {
        // don't hold the lock while handling: handlers may send more events
        let Some(event) = QUEUE.lock().unwrap().pop_front() else {
            break;
        };
        handler(event);
    }
}

/// Queue `event` for the UI thread. Never handles it synchronously, so it is
/// also safe to call from inside the app's own handlers and CEF callbacks.
pub fn send(_proxy: &Proxy, event: UserEvent) {
    QUEUE.lock().unwrap().push_back(event);
    let mut task = DrainTask::new();
    if post_task(ThreadId::UI, Some(&mut task)) == 0 {
        log::debug!("UI thread not running, event queued");
    }
}

/// Handle the events already queued (from before the app existed)
pub fn kick() {
    let mut task = DrainTask::new();
    post_task(ThreadId::UI, Some(&mut task));
}

/// Like `send`, after `delay_ms`
pub fn send_delayed(event: UserEvent, delay_ms: i64) {
    let mut task = DelayedSend::new(Arc::new(Mutex::new(Some(event))));
    post_delayed_task(ThreadId::UI, Some(&mut task), delay_ms);
}
