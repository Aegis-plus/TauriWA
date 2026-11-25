//! The main window, the webview, and everything that happens on the main (GTK) thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use serde_json::{json, Value};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::unix::WindowExtUnix;
use tao::window::{Fullscreen, Icon, Window, WindowBuilder};
use webkit2gtk::{
    SettingsExt, UserContentManagerExt, WebContextExt as _, WebViewExt, WebsiteDataManagerExt,
    WebsiteDataManagerExtManual, WebsiteDataTypes,
};
use wry::{
    NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, WebContext, WebView,
    WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix,
};
use zbus::blocking::Connection;

use crate::config::JsonConfig;
use crate::dbus::{self, Command};
use crate::events::{send, Proxy, UserEvent};
use crate::i18n::{self, Translations};
use crate::icons::{self, RgbaIcon};
use crate::notify::{Notification, Notifier};
use crate::{autostart, keys, tray, util};

const BRIDGE_JS: &str = include_str!("../web/bridge.js");
const INJECTED_JS: &str = include_str!("../web/injected.js");

/// Data cleared by "Clear cache…": the http cache and CacheStorage, not cookies/IndexedDB
fn cache_types() -> WebsiteDataTypes {
    WebsiteDataTypes::DISK_CACHE | WebsiteDataTypes::MEMORY_CACHE | WebsiteDataTypes::DOM_CACHE
}

pub struct App {
    window: Window,
    webview: WebView,
    wk: webkit2gtk::WebView,
    // must outlive the webview
    _web_context: WebContext,
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
    fullscreen: bool,
    newest_favicon: Option<String>,
    bounds_dirty: Option<Instant>,
    icons_dir: PathBuf,
    user_agent: String,
    exit: bool,
}

impl App {
    pub fn new(
        event_loop: &EventLoop<UserEvent>,
        config: JsonConfig,
        bus: Option<Connection>,
        visible: Arc<AtomicBool>,
        command: &Command,
    ) -> Result<Self, String> {
        let proxy = event_loop.create_proxy();
        let debug = util::is_debug();
        let persist = JsonConfig::new(util::user_data_dir().join("persistent-state.json"));
        log::info!("state file {}", persist.file().display());

        let quit_on_close = config.get("quit-on-close", util::default_quit_on_close());
        let show_at_startup = debug || quit_on_close || config.get("show-at-startup", true);
        let user_agent: String = config.get("user-agent", util::DEFAULT_USER_AGENT.to_owned());
        let icons_dir: PathBuf = config
            .get::<Option<String>>("icons-directory", None)
            .map(|d| PathBuf::from(util::replace_variables(&d)))
            .unwrap_or_else(|| util::user_data_dir().join("user-icons"));

        let preferred_langs = i18n::preferred_languages();
        log::debug!("preferredLangs {preferred_langs:?}");
        let t = Translations::load(&preferred_langs[0]);

        // --- window ---
        let mut builder = WindowBuilder::new()
            .with_title(util::PRODUCT_NAME)
            .with_visible(show_at_startup)
            .with_window_icon(window_icon());
        let bounds = persist.raw("window-bounds").cloned().unwrap_or(json!({}));
        let dim =
            |key: &str, default: f64| bounds.get(key).and_then(Value::as_f64).unwrap_or(default);
        let mut width = dim("width", 1099.0);
        if debug {
            width += 1000.0;
        }
        builder = builder.with_inner_size(LogicalSize::new(width, dim("height", 800.0)));
        if let (Some(x), Some(y)) = (
            bounds.get("x").and_then(Value::as_f64),
            bounds.get("y").and_then(Value::as_f64),
        ) {
            builder = builder.with_position(LogicalPosition::new(x, y));
        }
        let window = builder
            .build(event_loop)
            .map_err(|e| format!("cannot create window: {e}"))?;
        visible.store(show_at_startup, Ordering::SeqCst);

        // --- webview ---
        let data_dir = util::webview_data_dir();
        log::info!("webview data {}", data_dir.display());
        let mut web_context = WebContext::new(Some(data_dir));

        let start_url = match command {
            Command::OpenUrl(url) => web_url(url),
            _ => None,
        };
        let url: String = config.get("url", util::DEFAULT_URL.to_owned());

        let bridge =
            BRIDGE_JS.replace("__TAURIWA_STATE__", &page_state(&config, debug).to_string());

        let webview = {
            let p_ipc = proxy.clone();
            let p_load = proxy.clone();
            let p_title = proxy.clone();
            let p_download = proxy.clone();
            WebViewBuilder::new_with_web_context(&mut web_context)
                .with_url(start_url.as_deref().unwrap_or(&url))
                .with_user_agent(&user_agent)
                .with_devtools(true)
                .with_autoplay(true)
                .with_clipboard(true)
                .with_initialization_script_for_main_only(bridge, true)
                .with_ipc_handler(move |req| send(&p_ipc, UserEvent::Ipc(req.into_body())))
                .with_on_page_load_handler(move |event, url| {
                    log::debug!(
                        "page load {} {url}",
                        matches!(event, PageLoadEvent::Finished)
                    );
                    if let PageLoadEvent::Finished = event {
                        send(&p_load, UserEvent::PageLoaded);
                    }
                })
                .with_document_title_changed_handler(move |title| {
                    send(&p_title, UserEvent::Title(title))
                })
                .with_new_window_req_handler(|url, _features| {
                    open_link(&url);
                    NewWindowResponse::Deny
                })
                .with_navigation_handler(|url| {
                    // keep web pages in the window (as before), hand other schemes to the system
                    if ["https:", "http:", "about:", "blob:", "data:", "file:"]
                        .iter()
                        .any(|s| url.starts_with(s))
                    {
                        true
                    } else {
                        open_link(&url);
                        false
                    }
                })
                .with_permission_handler(|kind| match kind {
                    // calls, voice messages, notifications (handled by web/bridge.js anyway)
                    PermissionKind::Microphone
                    | PermissionKind::Camera
                    | PermissionKind::DisplayCapture
                    | PermissionKind::Notifications => PermissionResponse::Allow,
                    _ => PermissionResponse::Default,
                })
                .with_download_started_handler(|url, path| {
                    log::info!("download {url} -> {}", path.display());
                    true
                })
                .with_download_completed_handler(move |_url, path, success| {
                    send(&p_download, UserEvent::DownloadFinished { path, success })
                })
                .build_gtk(window.default_vbox().ok_or("no gtk vbox")?)
                .map_err(|e| format!("cannot create webview: {e}"))?
        };
        let wk = webview.webview();

        if let Some(context) = wk.context() {
            let spellcheck = config.get("spellcheck", true);
            context.set_spell_checking_enabled(spellcheck);
            let spell_langs: Vec<String> =
                config.get("spellcheck-languages", preferred_langs.clone());
            log::debug!("spellLangs {spell_langs:?}");
            // enchant wants "en_US"
            let spell_langs: Vec<String> =
                spell_langs.iter().map(|l| l.replace('-', "_")).collect();
            let refs: Vec<&str> = spell_langs.iter().map(String::as_str).collect();
            context.set_spell_checking_languages(&refs);
        }

        if let Some(settings) = WebViewExt::settings(&wk) {
            settings.set_enable_webrtc(true);
            settings.set_enable_media_stream(true);
            settings.set_enable_developer_extras(true);
            settings.set_javascript_can_access_clipboard(true);
        }

        let zoom = persist.get("zoom", 1.0f64).clamp(0.3, 3.0);
        if zoom != 1.0 {
            let _ = webview.zoom(zoom);
        }

        if debug || config.get("open-dev-tools", false) {
            webview.open_devtools();
        }

        // --- failed loads: notify and retry ---
        {
            let notifier_proxy = proxy.clone();
            let retry_ms: u64 = config.get("retry-interval", 15000u64);
            let url = url.clone();
            wk.connect_load_failed(move |wk, _event, failing_uri, error| {
                if error.matches(webkit2gtk::NetworkError::Cancelled)
                    || error.matches(webkit2gtk::PolicyError::FrameLoadInterruptedByPolicyChange)
                {
                    return false;
                }
                log::warn!("did-fail-load {failing_uri}: {error}");
                send(
                    &notifier_proxy,
                    UserEvent::Ipc(json!({"method": "loadFailed", "args": []}).to_string()),
                );
                let wk = wk.clone();
                let url = url.clone();
                glib::timeout_add_local_once(Duration::from_millis(retry_ms), move || {
                    log::info!("retry");
                    wk.load_uri(&url);
                });
                false
            });
        }

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

        let app = Self {
            window,
            webview,
            wk,
            _web_context: web_context,
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
            fullscreen: false,
            newest_favicon: None,
            bounds_dirty: None,
            icons_dir,
            user_agent,
            exit: false,
        };
        app.load_css();
        Ok(app)
    }

    /// Ask what to do about a broken config file. Returns false to quit.
    pub fn check_config(&self) -> bool {
        let Some(error) = self.config.parse_error() else {
            return true;
        };
        let dialog = gtk::MessageDialog::new(
            Some(self.window.gtk_window()),
            gtk::DialogFlags::MODAL,
            gtk::MessageType::Error,
            gtk::ButtonsType::None,
            &format!("{}:\n\n{error}", self.config.file().display()),
        );
        dialog.set_title(&self.t.get("config_title", "Configuration file"));
        dialog.add_button(
            &self
                .t
                .get("config_continue", "Continue with default config"),
            gtk::ResponseType::Accept,
        );
        dialog.add_button(
            &self.t.get("config_quit", "Quit"),
            gtk::ResponseType::Reject,
        );
        let response = dialog.run();
        dialog.close();
        response != gtk::ResponseType::Reject
    }

    pub fn handle(&mut self, event: Event<UserEvent>, control_flow: &mut ControlFlow) {
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if self
                    .bounds_dirty
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(1))
                {
                    self.save_bounds();
                }
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    self.save_bounds();
                    if self.quit_on_close {
                        self.quit();
                    } else {
                        log::debug!("close: hide");
                        self.hide_window();
                    }
                }
                WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                    self.bounds_dirty = Some(Instant::now());
                }
                _ => {}
            },
            Event::UserEvent(event) => self.handle_user_event(event),
            Event::LoopDestroyed => self.save_bounds(),
            _ => {}
        }

        *control_flow = if self.exit {
            ControlFlow::Exit
        } else if let Some(t) = self.bounds_dirty {
            ControlFlow::WaitUntil(t + Duration::from_secs(1))
        } else {
            ControlFlow::Wait
        };
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
                    let _ = self.webview.load_url(&web);
                }
                None => self.show_window(None),
            },
            UserEvent::Ipc(message) => self.handle_ipc(&message),
            UserEvent::PageLoaded => self.page_loaded(),
            UserEvent::Title(title) => {
                self.window.set_title(if title.is_empty() {
                    util::PRODUCT_NAME
                } else {
                    &title
                });
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
            UserEvent::Css(css) => self.add_css(&css),
            UserEvent::Reload => {
                log::debug!("reload");
                let _ = self.webview.reload();
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
            "loadFailed" => self.notifier.simple("Failed to load"),
            "windowToggle" => self.toggle_window(),
            "windowShow" => self.show_window(None),
            "windowHide" => self.hide_window(),
            "reload" => {
                let _ = self.webview.reload();
            }
            "quit" => self.quit(),
            "zoomIn" => self.set_zoom(self.zoom * 1.1),
            "zoomOut" => self.set_zoom(self.zoom / 1.1),
            "zoomReset" => self.set_zoom(1.0),
            "fullscreenToggle" => {
                self.fullscreen = !self.fullscreen;
                self.window
                    .set_fullscreen(self.fullscreen.then_some(Fullscreen::Borderless(None)));
            }
            "devTools" => self.webview.open_devtools(),
            other => log::warn!("ipc: unknown method {other}"),
        }
    }

    fn eval(&self, js: &str) {
        if let Err(err) = self.webview.evaluate_script(js) {
            log::error!("executeJavaScript: {err}");
        }
    }

    fn page_loaded(&self) {
        log::debug!("did-finish-load");
        self.eval(INJECTED_JS);

        // user scripts
        let user_scripts_dir = util::resolve_path(&util::user_data_dir().join("user-scripts"));
        let scripts: Vec<String> = self.config.get("scripts", vec![]);
        for script in scripts {
            let file = util::resolve_path(std::path::Path::new(&util::replace_variables(&script)));
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
                Err(_) => self.add_css(&css),
            }
        }
    }

    fn add_css(&self, css: &str) {
        if css.is_empty() {
            return;
        }
        let Some(manager) = self.wk.user_content_manager() else {
            return;
        };
        let sheet = webkit2gtk::UserStyleSheet::new(
            css,
            webkit2gtk::UserContentInjectedFrames::TopFrame,
            webkit2gtk::UserStyleLevel::User,
            &[],
            &[],
        );
        manager.add_style_sheet(&sheet);
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
        let _ = self.webview.zoom(self.zoom);
        self.persist
            .set("zoom", json!((self.zoom * 100.0).round() / 100.0));
    }

    fn show_window(&self, activation_token: Option<String>) {
        let gtk_window = self.window.gtk_window();
        // lets the compositor raise the window on Wayland
        if let Some(token) = activation_token {
            gtk_window.set_startup_id(&token);
        }
        if self.window.is_minimized() {
            self.window.set_minimized(false);
        }
        self.window.set_visible(true);
        gtk_window.present();
        self.window.set_focus();
        self.visible.store(true, Ordering::SeqCst);
    }

    fn hide_window(&self) {
        if self.config.get("close-chat-on-hide", true) {
            self.eval("typeof ewCloseChat === 'function' && ewCloseChat()");
        }
        self.window.set_visible(false);
        self.visible.store(false, Ordering::SeqCst);
    }

    fn toggle_window(&self) {
        log::debug!("toggleVisibility");
        if self.window.is_visible() {
            self.hide_window();
        } else {
            self.show_window(None);
        }
    }

    fn quit(&mut self) {
        log::debug!("quit");
        self.save_bounds();
        self.exit = true;
    }

    fn save_bounds(&mut self) {
        self.bounds_dirty = None;
        if self.debug || self.window.is_minimized() || !self.window.is_visible() {
            return;
        }
        let scale = self.window.scale_factor();
        let size: LogicalSize<f64> = self.window.inner_size().to_logical(scale);
        let mut bounds =
            json!({ "width": size.width.round() as i64, "height": size.height.round() as i64 });
        // not available on Wayland
        if let Ok(pos) = self.window.outer_position() {
            let pos: LogicalPosition<f64> = pos.to_logical(scale);
            bounds["x"] = json!(pos.x.round() as i64);
            bounds["y"] = json!(pos.y.round() as i64);
        }
        if self.persist.raw("window-bounds") != Some(&bounds) {
            self.persist.set("window-bounds", bounds);
        }
    }

    fn clear_cache(&self) {
        let Some(manager) = self.wk.website_data_manager() else {
            return;
        };
        let parent = self.window.gtk_window().clone();
        let t = self.t.clone();
        let proxy = self.proxy.clone();
        cache_size(&manager.clone(), move |size_before| {
            let message = t
                .get("clear_cache_message", "Clear the cache ({size})?")
                .replace("{size}", &util::format_size(size_before));
            let dialog = gtk::MessageDialog::new(
                Some(&parent),
                gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
                gtk::MessageType::Question,
                gtk::ButtonsType::None,
                &message,
            );
            dialog.set_title(&t.get("clear_cache", "Clear cache"));
            dialog.set_secondary_text(Some(&t.get(
                "clear_cache_detail",
                "Cached images and videos will be downloaded again when needed. You will stay logged in and your chats are not affected.",
            )));
            dialog.add_button(&t.get("cancel", "Cancel"), gtk::ResponseType::Cancel);
            dialog.add_button(
                &t.get("clear_cache_ok", "Clear cache"),
                gtk::ResponseType::Accept,
            );
            dialog.set_default_response(gtk::ResponseType::Accept);
            dialog.connect_response(move |dialog, response| {
                dialog.close();
                if response != gtk::ResponseType::Accept {
                    return;
                }
                log::info!(
                    "clear cache, size before {}",
                    util::format_size(size_before)
                );
                let proxy = proxy.clone();
                manager.clear(
                    cache_types(),
                    glib::TimeSpan(0),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Err(err) = result {
                            log::error!("clear cache: {err}");
                        }
                        send(&proxy, UserEvent::CacheCleared { size_before });
                    },
                );
            });
            dialog.show_all();
        });
    }

    fn cache_cleared(&self, size_before: u64) {
        let Some(manager) = self.wk.website_data_manager() else {
            return;
        };
        let notifier = self.notifier.clone();
        let message = self
            .t
            .get("clear_cache_done", "Cache cleared, {size} freed");
        let wk = self.wk.clone();
        cache_size(&manager, move |size_after| {
            log::info!("clear cache, size after {}", util::format_size(size_after));
            notifier.simple(message.replace(
                "{size}",
                &util::format_size(size_before.saturating_sub(size_after)),
            ));
            wk.reload();
        });
    }

    fn about(&self) {
        let dialog = gtk::AboutDialog::builder()
            .program_name(util::PRODUCT_NAME)
            .version(util::version_info())
            .comments(env!("CARGO_PKG_DESCRIPTION"))
            .authors(vec![
                "aegis-plus".to_owned(),
                "Pierre Carru (original ElecWhat)".to_owned(),
            ])
            .website(env!("CARGO_PKG_HOMEPAGE"))
            .license_type(gtk::License::Gpl30)
            .transient_for(self.window.gtk_window())
            .modal(true)
            .build();
        if let Some(logo) = pixbuf(icons::APP_512_PNG) {
            dialog.set_logo(Some(
                &logo
                    .scale_simple(128, 128, gtk::gdk_pixbuf::InterpType::Bilinear)
                    .unwrap_or(logo),
            ));
        }
        dialog.connect_response(|dialog, _| dialog.close());
        dialog.show_all();
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

fn cache_size(manager: &webkit2gtk::WebsiteDataManager, callback: impl FnOnce(u64) + 'static) {
    let types = cache_types();
    manager.fetch(types, None::<&gtk::gio::Cancellable>, move |result| {
        let size = match result {
            Ok(items) => items.iter().map(|d| d.size(types)).sum(),
            Err(err) => {
                log::warn!("cache size: {err}");
                0
            }
        };
        callback(size);
    });
}

fn pixbuf(png: &[u8]) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
    loader.write(png).ok()?;
    loader.close().ok()?;
    loader.pixbuf()
}

fn window_icon() -> Option<Icon> {
    let image = image::load_from_memory(icons::APP_PNG).ok()?.into_rgba8();
    let (w, h) = image.dimensions();
    Icon::from_rgba(image.into_raw(), w, h).ok()
}

/// Notification servers want an icon path or name
fn write_app_icon() -> String {
    let file = util::cache_dir().join("app.png");
    let ok = file.exists()
        || (std::fs::create_dir_all(util::cache_dir()).is_ok()
            && std::fs::write(&file, icons::APP_PNG).is_ok());
    if ok {
        file.to_string_lossy().into_owned()
    } else {
        util::APP_NAME.to_owned()
    }
}
