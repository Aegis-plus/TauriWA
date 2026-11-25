//! The main window, the browser (Chromium through CEF), and everything that
//! happens on CEF's UI thread.
//!
//! CEF callbacks never touch the app state directly: they only queue a
//! `UserEvent` (see events.rs), which `State::handle_user_event` handles. That
//! keeps the state (`State`) in one place and avoids re-entrancy, as many CEF calls
//! (closing, hiding, showing a window...) call back into the handlers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cef::*;
use serde_json::{json, Value};
use zbus::blocking::Connection;

use crate::config::JsonConfig;
use crate::dbus::{self, Command};
use crate::events::{self, send, Proxy, UserEvent};
use crate::i18n::{self, Translations};
use crate::icons::{self, RgbaIcon};
use crate::notify::{Notification, Notifier};
use crate::{autostart, keys, tray, util};

const BRIDGE_JS: &str = include_str!("../web/bridge.js");
const INJECTED_JS: &str = include_str!("../web/injected.js");

/// Prefix of the console messages the page uses to talk to us (web/bridge.js)
static IPC_PREFIX: OnceLock<String> = OnceLock::new();
/// Set when quitting: the main window may then really close
static QUITTING: AtomicBool = AtomicBool::new(false);

/// What `main` hands over to the app, created once CEF is ready
pub struct Startup {
    pub config: JsonConfig,
    pub bus: Option<Connection>,
    pub use_dbus: bool,
    pub command: Command,
}

pub static STARTUP: Mutex<Option<Startup>> = Mutex::new(None);

thread_local! {
    static APP: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn handle_event(event: UserEvent) {
    APP.with(|app| {
        if let Some(app) = app.borrow_mut().as_mut() {
            app.handle_user_event(event);
        }
    });
}

/// Called on the UI thread once CEF is initialized
pub fn start() {
    let Some(startup) = STARTUP.lock().unwrap().take() else {
        return;
    };
    let visible = Arc::new(AtomicBool::new(false));

    // single instance: claim the D-Bus name now that events can be handled
    if startup.use_dbus {
        if let Some(conn) = &startup.bus {
            if let dbus::Instance::Secondary =
                dbus::register(conn, Proxy, visible.clone(), &startup.command)
            {
                quit_message_loop();
                return;
            }
        }
    }

    let app = State::new(startup, visible);
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    events::set_handler(handle_event);
    // events queued before the app existed
    events::kick();
}

/// The main window was destroyed: the browser is gone, leave the message loop
fn main_window_destroyed() {
    APP.with(|app| {
        if let Ok(mut app) = app.try_borrow_mut() {
            if let Some(app) = app.as_mut() {
                app.close_about();
            }
        }
    });
    quit_message_loop();
}

#[derive(Clone, Copy, Debug)]
enum Question {
    ConfigError,
    ClearCache { size_before: u64 },
}

pub struct State {
    window: Option<Window>,
    browser_view: BrowserView,
    about_window: Option<Window>,
    config: JsonConfig,
    persist: JsonConfig,
    bus: Option<Connection>,
    notifier: Notifier,
    tray: Option<ksni::blocking::Handle<tray::Tray>>,
    t: Translations,
    proxy: Proxy,
    visible: Arc<AtomicBool>,
    quit_on_close: bool,
    debug: bool,
    zoom: f64,
    newest_favicon: Option<String>,
    icons_dir: PathBuf,
    user_agent: String,
    url: String,
    start_url: String,
    /// CSS from the "css" config entry, applied on every page load
    css: Vec<String>,
    page_loaded: bool,
    config_error_asked: bool,
    questions: HashMap<u64, Question>,
    next_question: u64,
    bounds_save_pending: bool,
}

impl State {
    fn new(startup: Startup, visible: Arc<AtomicBool>) -> Self {
        let Startup {
            config,
            bus,
            command,
            ..
        } = startup;
        let proxy = Proxy;
        let debug = util::is_debug();
        let persist = JsonConfig::new(util::user_data_dir().join("persistent-state.json"));
        log::info!("state file {}", persist.file().display());

        let quit_on_close = config.get("quit-on-close", util::default_quit_on_close());
        let show_at_startup = debug || quit_on_close || config.get("show-at-startup", true);
        let user_agent: String = config.get("user-agent", util::chrome_user_agent());
        let icons_dir: PathBuf = config
            .get::<Option<String>>("icons-directory", None)
            .map(|d| PathBuf::from(util::replace_variables(&d)))
            .unwrap_or_else(|| util::user_data_dir().join("user-icons"));

        let preferred_langs = i18n::preferred_languages();
        log::debug!("preferredLangs {preferred_langs:?}");
        let t = Translations::load(&preferred_langs[0]);

        let url: String = config.get("url", util::DEFAULT_URL.to_owned());
        let start_url = match &command {
            Command::OpenUrl(link) => web_url(link),
            _ => None,
        }
        .unwrap_or_else(|| url.clone());

        // --- browser and window ---
        // The browser starts on about:blank: the bridge script is registered
        // once it exists (BrowserCreated), then the real page is loaded.
        let mut client = MainClient::new();
        let mut view_delegate = MainBrowserViewDelegate::new();
        let blank = CefString::from("about:blank");
        let browser_view = browser_view_create(
            Some(&mut client),
            Some(&blank),
            Some(&BrowserSettings::default()),
            None,
            None,
            Some(&mut view_delegate),
        )
        .expect("cannot create the browser");

        let bounds = persist.raw("window-bounds").cloned().unwrap_or(json!({}));
        let dim =
            |key: &str, default: i64| bounds.get(key).and_then(Value::as_i64).unwrap_or(default);
        let mut width = dim("width", 1099);
        if debug {
            width += 1000;
        }
        let size = Size {
            width: width as i32,
            height: dim("height", 800) as i32,
        };
        let position = match (
            bounds.get("x").and_then(Value::as_i64),
            bounds.get("y").and_then(Value::as_i64),
        ) {
            (Some(x), Some(y)) => Some((x as i32, y as i32)),
            _ => None,
        };
        let mut window_delegate = MainWindowDelegate::new(
            RefCell::new(Some(browser_view.clone())),
            size,
            position,
            show_at_startup,
        );
        let window = window_create_top_level(Some(&mut window_delegate));
        visible.store(show_at_startup, Ordering::SeqCst);

        let app_icon_path = write_app_icon();
        let notifier = Notifier::new(bus.clone(), proxy.clone(), app_icon_path);

        // --- tray ---
        let tray_icon = icons::user_icon("app", &icons_dir)
            .or_else(|| RgbaIcon::decode(icons::APP_PNG).map(Arc::new));
        let tray = tray::spawn(
            proxy.clone(),
            t.clone(),
            tray_icon,
            quit_on_close,
            autostart::is_enabled(),
            autostart::supported(),
        );

        if config.get("register-url-scheme", true) && !util::is_dev_build() {
            util::register_url_scheme();
        }
        if config.get("refresh-autostart", true) {
            autostart::refresh();
        }

        let zoom = persist.get("zoom", 1.0f64).clamp(0.3, 3.0);

        let app = Self {
            window,
            browser_view,
            about_window: None,
            config,
            persist,
            bus,
            notifier,
            tray,
            t,
            proxy,
            visible,
            quit_on_close,
            debug,
            zoom,
            newest_favicon: None,
            icons_dir,
            user_agent,
            url,
            start_url,
            css: Vec::new(),
            page_loaded: false,
            config_error_asked: false,
            questions: HashMap::new(),
            next_question: 1,
            bounds_save_pending: false,
        };
        app.load_css();
        app
    }

    fn browser(&self) -> Option<Browser> {
        self.browser_view.browser()
    }

    fn host(&self) -> Option<BrowserHost> {
        self.browser()?.host()
    }

    fn handle_user_event(&mut self, event: UserEvent) {
        match event {
            UserEvent::Show { activation_token } => self.show_window(activation_token),
            UserEvent::Hide => self.hide_window(),
            UserEvent::Toggle => self.toggle_window(),
            UserEvent::Quit => self.quit(),
            UserEvent::OpenUrl(url) => match web_url(&url) {
                Some(web) => {
                    log::info!("Loading WhatsApp URL: {web}");
                    self.show_window(None);
                    // is there a way to pass it to the webapp without reloading?
                    self.load(&web);
                }
                None => self.show_window(None),
            },
            UserEvent::Ipc(message) => self.handle_ipc(&message),
            UserEvent::BrowserCreated => self.browser_created(),
            UserEvent::PageLoaded => self.page_loaded(),
            UserEvent::LoadFailed => {
                self.notifier.simple("Failed to load");
                let retry_ms: i64 = self.config.get("retry-interval", 15000i64);
                events::send_delayed(UserEvent::Retry, retry_ms);
            }
            UserEvent::Retry => {
                log::info!("retry");
                let url = self.url.clone();
                self.load(&url);
            }
            UserEvent::Title(title) => {
                if let Some(window) = &self.window {
                    let title = if title.is_empty() || title.starts_with("about:") {
                        util::PRODUCT_NAME.to_owned()
                    } else {
                        title
                    };
                    window.set_title(Some(&CefString::from(title.as_str())));
                }
            }
            UserEvent::BoundsChanged => {
                // save once things settle
                if !self.bounds_save_pending {
                    self.bounds_save_pending = true;
                    events::send_delayed(UserEvent::BoundsChanged, 1000);
                } else {
                    self.bounds_save_pending = false;
                    self.save_bounds();
                }
            }
            UserEvent::CloseRequested => {
                self.save_bounds();
                if self.quit_on_close {
                    self.quit();
                } else {
                    log::debug!("close: hide");
                    self.hide_window();
                }
            }
            UserEvent::NotificationClicked {
                js_id,
                activation_token,
            } => {
                self.show_window(activation_token);
                if let Some(id) = js_id {
                    self.eval(&format!(
                        "window.__tauriwaNotificationEvent?.({id}, 'click')"
                    ));
                }
            }
            UserEvent::NotificationClosed { js_id } => {
                self.eval(&format!(
                    "window.__tauriwaNotificationEvent?.({js_id}, 'close')"
                ));
            }
            UserEvent::Favicon { url, icon } => {
                // an old icon can take time to download and arrive late
                if self.newest_favicon.as_deref() != Some(url.as_str()) {
                    return;
                }
                if let Some(icon) = icon {
                    if let Some(tray) = &self.tray {
                        tray.update(|t| t.icon = Some(icon));
                    }
                    let unread = util::unread_count_from_favicon(&url);
                    if let Some(bus) = &self.bus {
                        dbus::set_badge_count(bus, unread);
                    }
                }
            }
            UserEvent::Css(css) => {
                if self.page_loaded {
                    self.eval(&css_js(&css));
                }
                self.css.push(css);
            }
            UserEvent::Reload => {
                if self.page_loaded {
                    log::debug!("reload");
                    if let Some(browser) = self.browser() {
                        browser.reload();
                    }
                }
            }
            UserEvent::ClearCache => {
                self.show_window(None);
                self.clear_cache();
            }
            UserEvent::CacheCleared { size_before } => self.cache_cleared(size_before),
            UserEvent::SetQuitOnClose(value) => {
                self.quit_on_close = value;
                self.config.set("quit-on-close", Value::Bool(value));
            }
            UserEvent::SetAutostart(value) => {
                autostart::set(value);
                let enabled = autostart::is_enabled();
                if let Some(tray) = &self.tray {
                    tray.update(|t| t.autostart = enabled);
                }
            }
            UserEvent::About => self.about(),
            UserEvent::DownloadFinished { path, success } => {
                let name = path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                if success {
                    log::info!("downloaded {name}");
                    self.notifier.simple(format!("Downloaded {name}"));
                } else {
                    log::warn!("download failed {name}");
                    self.notifier.simple(format!("Download failed {name}"));
                }
            }
        }
    }

    fn handle_ipc(&mut self, message: &str) {
        let Ok(message) = serde_json::from_str::<Value>(message) else {
            log::warn!("ipc: invalid message {message}");
            return;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let args = message
            .get("args")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let arg_str = |i: usize| args.get(i).and_then(Value::as_str).map(str::to_owned);
        log::debug!("ipc {method}");
        match method {
            "notify" => {
                let Some(n) = args.first() else { return };
                let s = |key: &str| n.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
                self.notifier.show(Notification {
                    js_id: n.get("id").and_then(Value::as_u64),
                    title: s("title"),
                    body: s("body"),
                    tag: n
                        .get("tag")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                        .map(str::to_owned),
                    silent: n.get("silent").and_then(Value::as_bool).unwrap_or(false),
                });
            }
            "notifyClose" => {
                if let Some(id) = args.first().and_then(Value::as_u64) {
                    self.notifier.close(id);
                }
            }
            "open" => {
                if let Some(url) = arg_str(0) {
                    open_link(&url);
                }
            }
            "favicon" => {
                if let Some(url) = arg_str(0) {
                    self.favicon_changed(url);
                }
            }
            "confirmResult" => {
                let id = args.first().and_then(Value::as_u64).unwrap_or(0);
                let ok = args.get(1).and_then(Value::as_bool).unwrap_or(false);
                if let Some(question) = self.questions.remove(&id) {
                    self.answered(question, ok);
                }
            }
            "windowToggle" => self.toggle_window(),
            "windowShow" => self.show_window(None),
            "windowHide" => self.hide_window(),
            "reload" => {
                if let Some(browser) = self.browser() {
                    browser.reload();
                }
            }
            "quit" => self.quit(),
            "zoomIn" => self.set_zoom(self.zoom * 1.1),
            "zoomOut" => self.set_zoom(self.zoom / 1.1),
            "zoomReset" => self.set_zoom(1.0),
            "fullscreenToggle" => {
                if let Some(window) = &self.window {
                    window.set_fullscreen((window.is_fullscreen() == 0).into());
                }
            }
            "devTools" => self.dev_tools(),
            other => log::warn!("ipc: unknown method {other}"),
        }
    }

    fn eval(&self, js: &str) {
        let Some(frame) = self.browser().and_then(|b| b.main_frame()) else {
            log::debug!("eval: no frame");
            return;
        };
        frame.execute_java_script(Some(&CefString::from(js)), None, 0);
    }

    fn load(&self, url: &str) {
        if let Some(frame) = self.browser().and_then(|b| b.main_frame()) {
            frame.load_url(Some(&CefString::from(url)));
        }
    }

    /// Runs a DevTools protocol method on the page, see
    /// https://chromedevtools.github.io/devtools-protocol/
    fn dev_tools_method(&self, method: &str, params: &[(&str, Value)]) {
        let Some(host) = self.host() else { return };
        let Some(mut dict) = dictionary_value_create() else {
            return;
        };
        for (key, value) in params {
            let key = CefString::from(*key);
            match value {
                Value::Bool(b) => {
                    dict.set_bool(Some(&key), (*b).into());
                }
                Value::String(s) => {
                    dict.set_string(Some(&key), Some(&CefString::from(s.as_str())));
                }
                other => log::warn!("devtools {method}: unsupported param {other}"),
            }
        }
        if host.execute_dev_tools_method(0, Some(&CefString::from(method)), Some(&mut dict)) == 0 {
            log::warn!("devtools {method} failed");
        }
    }

    fn browser_created(&mut self) {
        log::debug!("browser created");
        let prefix = IPC_PREFIX.get_or_init(random_prefix);
        let bridge = BRIDGE_JS
            .replace(
                "__TAURIWA_STATE__",
                &page_state(&self.config, self.debug).to_string(),
            )
            .replace(
                "__TAURIWA_IPC_PREFIX__",
                &Value::String(prefix.clone()).to_string(),
            );
        self.dev_tools_method("Page.enable", &[]);
        self.dev_tools_method(
            "Page.addScriptToEvaluateOnNewDocument",
            &[("source", Value::String(bridge))],
        );
        if self.zoom != 1.0 {
            self.apply_zoom();
        }
        if self.debug || self.config.get("open-dev-tools", false) {
            self.dev_tools();
        }
        let url = self.start_url.clone();
        self.load(&url);
    }

    fn page_loaded(&mut self) {
        let Some(url) = self
            .browser()
            .and_then(|b| b.main_frame())
            .map(|f| CefString::from(&f.url()).to_string())
        else {
            return;
        };
        if url == "about:blank" {
            return;
        }
        log::debug!("did-finish-load {url}");
        self.page_loaded = true;
        self.eval(INJECTED_JS);
        for css in &self.css {
            self.eval(&css_js(css));
        }

        // user scripts
        let user_scripts_dir = util::resolve_path(&util::user_data_dir().join("user-scripts"));
        let scripts: Vec<String> = self.config.get("scripts", vec![]);
        for script in scripts {
            let file = util::resolve_path(Path::new(&util::replace_variables(&script)));
            log::info!("load user script {}", file.display());
            if !file.starts_with(&user_scripts_dir) {
                log::error!(
                    "script must be in {}: {}",
                    user_scripts_dir.display(),
                    file.display()
                );
                continue;
            }
            if file.extension().and_then(|e| e.to_str()) != Some("js") {
                log::error!("script must end with .js {}", file.display());
                continue;
            }
            match std::fs::read_to_string(&file) {
                Ok(data) if !data.is_empty() => self.eval(&data),
                Ok(_) => {}
                Err(err) => log::error!("{}: {err}", file.display()),
            }
        }

        // a broken config file: ask once whether to go on with the defaults
        if let (Some(error), false) = (self.config.parse_error(), self.config_error_asked) {
            self.config_error_asked = true;
            let message = format!(
                "{}\n\n{}:\n{error}\n\n{} / {}",
                self.t.get("config_title", "Configuration file"),
                self.config.file().display(),
                self.t
                    .get("config_continue", "Continue with default config"),
                self.t.get("config_quit", "Quit"),
            );
            self.ask(Question::ConfigError, &message);
        }
    }

    /// Asks with the page's confirm box, the answer comes back through IPC
    fn ask(&mut self, question: Question, message: &str) {
        let id = self.next_question;
        self.next_question += 1;
        self.questions.insert(id, question);
        self.eval(&format!(
            "window.__tauriwaConfirm?.({id}, {})",
            Value::String(message.to_owned())
        ));
    }

    fn answered(&mut self, question: Question, ok: bool) {
        match question {
            Question::ConfigError => {
                if !ok {
                    self.quit();
                }
            }
            Question::ClearCache { size_before } => {
                if !ok {
                    return;
                }
                log::info!(
                    "clear cache, size before {}",
                    util::format_size(size_before)
                );
                self.dev_tools_method("Network.clearBrowserCache", &[]);
                self.dev_tools_method(
                    "Storage.clearDataForOrigin",
                    &[
                        ("origin", Value::String(origin(&self.url))),
                        (
                            "storageTypes",
                            Value::String("cache_storage,shader_cache".to_owned()),
                        ),
                    ],
                );
                // the data is removed asynchronously
                events::send_delayed(UserEvent::CacheCleared { size_before }, 1500);
            }
        }
    }

    /// The "css" config entries: inline CSS, file:// or https:// URLs
    fn load_css(&self) {
        let entries: Vec<String> = self.config.get("css", vec![]);
        for css in entries {
            match url::Url::parse(&css) {
                Ok(url) => {
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || match util::load_url(&url) {
                        Ok(data) => send(&proxy, UserEvent::Css(data)),
                        Err(err) => log::error!("error inserting {url}: {err}"),
                    });
                }
                Err(_) => send(&self.proxy, UserEvent::Css(css)),
            }
        }
    }

    fn favicon_changed(&mut self, url: String) {
        self.newest_favicon = Some(url.clone());
        let proxy = self.proxy.clone();
        let icons_dir = self.icons_dir.clone();
        let user_agent = self.user_agent.clone();
        std::thread::spawn(move || {
            let icon = icons::get_icon(&url, &icons_dir, &user_agent);
            send(&proxy, UserEvent::Favicon { url, icon });
        });
    }

    fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(0.3, 3.0);
        if (self.zoom - 1.0).abs() < 0.01 {
            self.zoom = 1.0;
        }
        self.apply_zoom();
        self.persist
            .set("zoom", json!((self.zoom * 100.0).round() / 100.0));
    }

    fn apply_zoom(&self) {
        // Chromium zoom levels: factor = 1.2 ^ level
        if let Some(host) = self.host() {
            host.set_zoom_level(self.zoom.ln() / 1.2f64.ln());
        }
    }

    fn dev_tools(&self) {
        if let Some(host) = self.host() {
            host.show_dev_tools(
                Some(&WindowInfo::default()),
                None,
                Some(&BrowserSettings::default()),
                None,
            );
        }
    }

    fn show_window(&self, activation_token: Option<String>) {
        if activation_token.is_some() {
            log::debug!("activation token ignored");
        }
        let Some(window) = &self.window else { return };
        if window.is_minimized() != 0 {
            window.restore();
        }
        window.show();
        window.activate();
        self.browser_view.request_focus();
        self.visible.store(true, Ordering::SeqCst);
    }

    fn hide_window(&self) {
        if self.config.get("close-chat-on-hide", true) {
            self.eval("typeof ewCloseChat === 'function' && ewCloseChat()");
        }
        if let Some(window) = &self.window {
            window.hide();
        }
        self.visible.store(false, Ordering::SeqCst);
    }

    fn toggle_window(&self) {
        log::debug!("toggleVisibility");
        let visible = self.window.as_ref().is_some_and(|w| w.is_visible() != 0);
        if visible {
            self.hide_window();
        } else {
            self.show_window(None);
        }
    }

    fn quit(&mut self) {
        log::debug!("quit");
        self.save_bounds();
        QUITTING.store(true, Ordering::SeqCst);
        self.close_about();
        match &self.window {
            // can_close lets it close now, on_window_destroyed ends the loop
            Some(window) => window.close(),
            None => quit_message_loop(),
        }
    }

    fn close_about(&mut self) {
        if let Some(window) = self.about_window.take() {
            window.close();
        }
    }

    fn save_bounds(&mut self) {
        let Some(window) = &self.window else { return };
        if self.debug
            || window.is_minimized() != 0
            || window.is_visible() == 0
            || window.is_fullscreen() != 0
        {
            return;
        }
        let rect = window.bounds();
        if rect.width <= 0 || rect.height <= 0 {
            return;
        }
        let bounds = json!({
            "width": rect.width,
            "height": rect.height,
            "x": rect.x,
            "y": rect.y,
        });
        if self.persist.raw("window-bounds") != Some(&bounds) {
            self.persist.set("window-bounds", bounds);
        }
    }

    fn clear_cache(&mut self) {
        let size_before = cache_size(&util::webview_data_dir());
        let message = format!(
            "{}\n\n{}",
            self.t
                .get("clear_cache_message", "Clear the cache ({size})?")
                .replace("{size}", &util::format_size(size_before)),
            self.t.get(
                "clear_cache_detail",
                "Cached images and videos will be downloaded again when needed. You will stay logged in and your chats are not affected.",
            )
        );
        self.ask(Question::ClearCache { size_before }, &message);
    }

    fn cache_cleared(&self, size_before: u64) {
        let size_after = cache_size(&util::webview_data_dir());
        log::info!("clear cache, size after {}", util::format_size(size_after));
        let message = self
            .t
            .get("clear_cache_done", "Cache cleared, {size} freed");
        self.notifier.simple(message.replace(
            "{size}",
            &util::format_size(size_before.saturating_sub(size_after)),
        ));
        if let Some(browser) = self.browser() {
            browser.reload();
        }
    }

    fn about(&mut self) {
        if let Some(window) = &self.about_window {
            if window.is_valid() != 0 {
                window.show();
                window.activate();
                return;
            }
        }
        let html = about_html();
        let url = format!(
            "data:text/html;charset=utf-8,{}",
            CefString::from(&uriencode(Some(&CefString::from(html.as_str())), 0))
        );
        let mut client = AboutClient::new();
        let mut view_delegate = PopupBrowserViewDelegate::new();
        let view = browser_view_create(
            Some(&mut client),
            Some(&CefString::from(url.as_str())),
            Some(&BrowserSettings::default()),
            None,
            None,
            Some(&mut view_delegate),
        );
        let mut delegate = PopupWindowDelegate::new(
            RefCell::new(view),
            Size {
                width: 420,
                height: 380,
            },
        );
        self.about_window = window_create_top_level(Some(&mut delegate));
    }
}

/// State read by the page through `tauriwa.stateGet(name)`
fn page_state(config: &JsonConfig, debug: bool) -> Value {
    json!({
        "debug": debug,
        "notifPrefix": config.get("notification-prefix", String::new()),
        "escToggle": config.get("esc-toggle-window", false),
        "keys": keys::merged_keys(config.raw("keys")),
    })
}

/// Adds CSS to the page without being subject to its Content-Security-Policy
fn css_js(css: &str) -> String {
    format!(
        "(() => {{ const s = new CSSStyleSheet(); s.replaceSync({}); \
         document.adoptedStyleSheets = [...document.adoptedStyleSheets, s]; }})()",
        Value::String(css.to_owned())
    )
}

/// "https://web.whatsapp.com/" -> "https://web.whatsapp.com"
fn origin(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| url.trim_end_matches('/').to_owned())
}

/// whatsapp: links, or links to WhatsApp Web itself
fn web_url(url: &str) -> Option<String> {
    if url.starts_with(&format!("{}:", util::URL_SCHEME)) {
        util::convert_whatsapp_url(url)
    } else if url.starts_with(util::DEFAULT_URL) {
        Some(url.to_owned())
    } else {
        None
    }
}

/// Links clicked in the page
fn open_link(url: &str) {
    if ["https:", "http:", "mailto:", "tel:"]
        .iter()
        .any(|s| url.starts_with(s))
    {
        util::open_external(url);
    } else {
        log::warn!("not opening {url}");
    }
}

/// Pages may navigate the main window to these, others go to the system
fn stays_in_app(url: &str) -> bool {
    [
        "https:",
        "http:",
        "about:",
        "blob:",
        "data:",
        "file:",
        "devtools:",
        "chrome:",
    ]
    .iter()
    .any(|s| url.starts_with(s))
}

fn random_prefix() -> String {
    let mut bytes = [0u8; 8];
    let random = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes));
    if random.is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        bytes = ((nanos as u64) ^ ((std::process::id() as u64) << 32)).to_le_bytes();
    }
    format!("\u{1}tauriwa:{:016x}:", u64::from_le_bytes(bytes))
}

/// Size of Chromium's caches in the profile (http, code, GPU, CacheStorage)
fn cache_size(profile: &Path) -> u64 {
    const CACHE_DIRS: [&str; 8] = [
        "Cache",
        "Code Cache",
        "GPUCache",
        "CacheStorage",
        "ShaderCache",
        "GrShaderCache",
        "DawnGraphiteCache",
        "DawnWebGPUCache",
    ];
    fn dir_size(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => dir_size(&e.path()),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    fn walk(dir: &Path, depth: u32) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| {
                let name = e.file_name();
                if CACHE_DIRS.iter().any(|c| name == **c) {
                    dir_size(&e.path())
                } else if depth < 4 {
                    walk(&e.path(), depth + 1)
                } else {
                    0
                }
            })
            .sum()
    }
    walk(profile, 0)
}

fn about_html() -> String {
    let icon = CefString::from(&base64_encode(Some(icons::APP_512_PNG))).to_string();
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>About {name}</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font: 14px system-ui, sans-serif; text-align: center; margin: 24px; }}
  img {{ width: 112px; height: 112px; }}
  h1 {{ font-size: 22px; margin: 8px 0 4px; }}
  p {{ margin: 6px 0; opacity: .85; }}
  a {{ color: #0ea5a4; }}
</style></head><body>
<img src="data:image/png;base64,{icon}" alt="">
<h1>{name}</h1>
<p>{version}</p>
<p>{description}</p>
<p>aegis-plus · Pierre Carru (original ElecWhat)</p>
<p><a href="{homepage}" target="_blank">{homepage}</a></p>
<p>GPL-3.0-or-later</p>
</body></html>"#,
        name = util::PRODUCT_NAME,
        version = esc(&util::version_info()),
        description = esc(env!("CARGO_PKG_DESCRIPTION")),
        homepage = env!("CARGO_PKG_HOMEPAGE"),
    )
}

/// Notification servers want an icon path or name
fn write_app_icon() -> String {
    let file = util::cache_dir().join("app.png");
    // rewrite it when the bundled icon changed (app update)
    let up_to_date = std::fs::read(&file).is_ok_and(|bytes| bytes == icons::APP_PNG);
    let ok = up_to_date
        || (std::fs::create_dir_all(util::cache_dir()).is_ok()
            && std::fs::write(&file, icons::APP_PNG).is_ok());
    if ok {
        file.to_string_lossy().into_owned()
    } else {
        util::APP_NAME.to_owned()
    }
}

fn app_image() -> Option<Image> {
    let image = image_create()?;
    image.add_png(1.0, Some(icons::APP_PNG));
    image.add_png(2.0, Some(icons::APP_512_PNG));
    Some(image)
}

// ---------------------------------------------------------------------------
// CEF handlers: they only queue events (or answer CEF synchronously)

wrap_window_delegate! {
    struct MainWindowDelegate {
        browser_view: RefCell<Option<BrowserView>>,
        size: Size,
        position: Option<(i32, i32)>,
        show: bool,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            self.size.clone()
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let browser_view = self.browser_view.borrow();
            let (Some(window), Some(browser_view)) = (window, browser_view.as_ref()) else {
                return;
            };
            let mut view = View::from(browser_view);
            window.add_child_view(Some(&mut view));
            window.set_title(Some(&CefString::from(util::PRODUCT_NAME)));
            if let Some(mut image) = app_image() {
                window.set_window_icon(Some(&mut image));
                window.set_window_app_icon(Some(&mut image));
            }
            if self.position.is_none() {
                window.center_window(Some(&self.size));
            }
            if self.show {
                window.show();
            }
        }

        fn initial_bounds(&self, _window: Option<&mut Window>) -> Rect {
            match self.position {
                Some((x, y)) => Rect {
                    x,
                    y,
                    width: self.size.width,
                    height: self.size.height,
                },
                None => Rect::default(),
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            *self.browser_view.borrow_mut() = None;
            main_window_destroyed();
        }

        fn on_window_bounds_changed(&self, _window: Option<&mut Window>, _new_bounds: Option<&Rect>) {
            send(&Proxy, UserEvent::BoundsChanged);
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            if !QUITTING.load(Ordering::SeqCst) {
                // hide or quit, depending on the "quit on close" setting
                send(&Proxy, UserEvent::CloseRequested);
                return 0;
            }
            let browser_view = self.browser_view.borrow();
            match browser_view.as_ref().and_then(|v| v.browser()).and_then(|b| b.host()) {
                Some(host) => host.try_close_browser(),
                None => 1,
            }
        }
    }
}

wrap_browser_view_delegate! {
    struct MainBrowserViewDelegate {}

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn on_popup_browser_view_created(
            &self,
            _browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            _is_devtools: i32,
        ) -> i32 {
            // only DevTools gets here: links opening new windows go to the system browser
            let mut delegate = PopupWindowDelegate::new(
                RefCell::new(popup_browser_view.cloned()),
                Size { width: 1000, height: 700 },
            );
            window_create_top_level(Some(&mut delegate));
            1
        }
    }
}

// DevTools and About windows
wrap_window_delegate! {
    struct PopupWindowDelegate {
        browser_view: RefCell<Option<BrowserView>>,
        size: Size,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            self.size.clone()
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let browser_view = self.browser_view.borrow();
            let (Some(window), Some(browser_view)) = (window, browser_view.as_ref()) else {
                return;
            };
            let mut view = View::from(browser_view);
            window.add_child_view(Some(&mut view));
            if let Some(mut image) = app_image() {
                window.set_window_icon(Some(&mut image));
            }
            window.center_window(Some(&self.size));
            window.show();
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            *self.browser_view.borrow_mut() = None;
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            let browser_view = self.browser_view.borrow();
            match browser_view.as_ref().and_then(|v| v.browser()).and_then(|b| b.host()) {
                Some(host) => host.try_close_browser(),
                None => 1,
            }
        }
    }
}

wrap_browser_view_delegate! {
    struct PopupBrowserViewDelegate {}

    impl ViewDelegate {}

    impl BrowserViewDelegate {}
}

wrap_client! {
    struct MainClient {}

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(MainDisplayHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(MainLifeSpanHandler::new())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(MainLoadHandler::new())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(MainRequestHandler::new())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(MainDownloadHandler::new())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(MainPermissionHandler::new())
        }
    }
}

wrap_display_handler! {
    struct MainDisplayHandler {}

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(CefString::to_string).unwrap_or_default();
            send(&Proxy, UserEvent::Title(title));
        }

        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let (Some(message), Some(prefix)) = (message, IPC_PREFIX.get()) else {
                return 0;
            };
            let message = message.to_string();
            match message.strip_prefix(prefix.as_str()) {
                Some(ipc) => {
                    send(&Proxy, UserEvent::Ipc(ipc.to_owned()));
                    1
                }
                None => 0,
            }
        }
    }
}

wrap_life_span_handler! {
    struct MainLifeSpanHandler {}

    impl LifeSpanHandler {
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            send(&Proxy, UserEvent::BrowserCreated);
        }

        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(url) = target_url {
                open_link(&url.to_string());
            }
            1
        }
    }
}

wrap_load_handler! {
    struct MainLoadHandler {}

    impl LoadHandler {
        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _http_status_code: ::std::os::raw::c_int,
        ) {
            if frame.is_some_and(|f| f.is_main() != 0) {
                send(&Proxy, UserEvent::PageLoaded);
            }
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            if !frame.is_some_and(|f| f.is_main() != 0) {
                return;
            }
            // cancelled navigations, downloads
            if sys::cef_errorcode_t::from(error_code) == sys::cef_errorcode_t::ERR_ABORTED {
                return;
            }
            log::warn!(
                "did-fail-load {}: {}",
                failed_url.map(CefString::to_string).unwrap_or_default(),
                error_text.map(CefString::to_string).unwrap_or_default()
            );
            send(&Proxy, UserEvent::LoadFailed);
        }
    }
}

wrap_request_handler! {
    struct MainRequestHandler {}

    impl RequestHandler {
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: ::std::os::raw::c_int,
            _is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let url = request
                .map(|r| CefString::from(&r.url()).to_string())
                .unwrap_or_default();
            if stays_in_app(&url) {
                0
            } else {
                open_link(&url);
                1
            }
        }

        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            if let Some(url) = target_url {
                open_link(&url.to_string());
            }
            1
        }
    }
}

wrap_download_handler! {
    struct MainDownloadHandler {}

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            _download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 0 };
            let name = suggested_name
                .map(CefString::to_string)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "download".to_owned());
            let path = util::unique_path(&util::downloads_dir(), &name);
            log::info!("download -> {}", path.display());
            callback.cont(Some(&CefString::from(path.to_string_lossy().as_ref())), 0);
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else { return };
            let path = || {
                let p = CefString::from(&item.full_path()).to_string();
                (!p.is_empty()).then(|| PathBuf::from(p))
            };
            if item.is_complete() != 0 {
                send(&Proxy, UserEvent::DownloadFinished { path: path(), success: true });
            } else if item.is_canceled() != 0 || item.is_interrupted() != 0 {
                send(&Proxy, UserEvent::DownloadFinished { path: path(), success: false });
            }
        }
    }
}

wrap_permission_handler! {
    struct MainPermissionHandler {}

    impl PermissionHandler {
        /// microphone and camera (voice messages, calls) for WhatsApp only
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            let origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            match callback {
                Some(callback) if is_whatsapp(&origin) => {
                    callback.cont(requested_permissions);
                    1
                }
                _ => 0,
            }
        }

        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            _prompt_id: u64,
            requesting_origin: Option<&CefString>,
            _requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            let origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            match callback {
                Some(callback) if is_whatsapp(&origin) => {
                    callback.cont(PermissionRequestResult::ACCEPT);
                    1
                }
                _ => 0,
            }
        }
    }
}

fn is_whatsapp(origin: &str) -> bool {
    origin.trim_end_matches('/') == "https://web.whatsapp.com"
}

// About window: links open in the system browser
wrap_client! {
    struct AboutClient {}

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(AboutLifeSpanHandler::new())
        }
    }
}

wrap_life_span_handler! {
    struct AboutLifeSpanHandler {}

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(url) = target_url {
                open_link(&url.to_string());
            }
            1
        }
    }
}

// ---------------------------------------------------------------------------
// The CEF application: command line switches and startup

wrap_app! {
    pub struct TauriwaApp {}

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            // browser process only (subprocesses inherit what matters)
            if process_type.is_some_and(|t| !t.to_string().is_empty()) {
                return;
            }
            let Some(command_line) = command_line else { return };
            let switch = |name: &str, value: Option<&str>| {
                let name = CefString::from(name);
                if command_line.has_switch(Some(&name)) != 0 {
                    return; // the user's own switch wins
                }
                match value {
                    Some(value) => command_line
                        .append_switch_with_value(Some(&name), Some(&CefString::from(value))),
                    None => command_line.append_switch(Some(&name)),
                }
            };
            // native Wayland when available, X11 otherwise
            switch("ozone-platform-hint", Some("auto"));
            // WM_CLASS / app_id, matches StartupWMClass of the .desktop file
            switch("class", Some(util::APP_NAME));
            // don't ask for the GNOME keyring / KWallet: nothing is stored there
            switch("password-store", Some("basic"));
            // WhatsApp plays notification sounds and voice messages by itself
            switch("autoplay-policy", Some("no-user-gesture-required"));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(TauriwaBrowserProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct TauriwaBrowserProcessHandler {}

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            start();
        }
    }
}
