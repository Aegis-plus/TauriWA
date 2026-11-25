//! TauriWA - Simple desktop WhatsApp client for Linux

mod app;
mod autostart;
mod config;
mod dbus;
mod events;
mod i18n;
mod icons;
mod keys;
mod notify;
mod tray;
mod util;

use crate::config::JsonConfig;
use crate::dbus::Command;

const USAGE: &str = "\
Usage: tauriwa [OPTIONS] [whatsapp://URL]

Starts TauriWA, or sends a command to the running instance.

Options:
  --show               Show the window (default)
  --hide               Hide the window
  --toggle             Show or hide the window
  --quit               Quit the running instance
  --log-level=LEVEL    silent, error, warn, info, debug or trace
  -h, --help           Print this help
  -V, --version        Print the version

Configuration: ~/.config/tauriwa/config.json";

fn parse_command(args: &[String]) -> Command {
    if let Some(url) = args
        .iter()
        .find(|a| a.starts_with(&format!("{}:", util::URL_SCHEME)))
    {
        return Command::OpenUrl(url.clone());
    }
    if args.iter().any(|a| a == "--hide") {
        Command::Hide
    } else if args.iter().any(|a| a == "--toggle") {
        Command::Toggle
    } else if args.iter().any(|a| a == "--quit") {
        Command::Quit
    } else {
        Command::Show
    }
}

/// consola-style level names (as in ElecWhat), plus the usual names
fn log_level(name: &str) -> log::LevelFilter {
    match name.to_lowercase().as_str() {
        "silent" | "off" => log::LevelFilter::Off,
        "fatal" | "error" | "0" | "1" => log::LevelFilter::Error,
        "warn" | "warning" | "2" => log::LevelFilter::Warn,
        "debug" | "4" => log::LevelFilter::Debug,
        "trace" | "verbose" | "5" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

fn init_logger(level: log::LevelFilter) {
    let mut builder = env_logger::Builder::new();
    builder
        .filter_level(level)
        // keep the dependencies quiet unless asked through RUST_LOG
        .filter_module("ureq", log::LevelFilter::Warn)
        .filter_module("ureq_proto", log::LevelFilter::Warn)
        .filter_module("rustls", log::LevelFilter::Warn)
        .filter_module("ksni", log::LevelFilter::Warn)
        .filter_module("zbus", log::LevelFilter::Warn)
        .parse_default_env()
        .init();
}

fn main() {
    // CEF runs its helper processes (renderer, GPU, network...) from this
    // same binary: hand them over to CEF before anything else.
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let cef_args = cef::args::Args::new();
    let code = cef::execute_process(
        Some(cef_args.as_main_args()),
        None::<&mut cef::App>,
        std::ptr::null_mut(),
    );
    if code >= 0 {
        std::process::exit(code);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("{} {}", util::APP_NAME, env!("CARGO_PKG_VERSION"));
        return;
    }

    util::migrate_from_elecwhat();
    let config = JsonConfig::new(util::user_data_dir().join("config.json"));

    // CLI option takes priority over config option
    let cli_level = args
        .iter()
        .find_map(|a| a.strip_prefix("--log-level=").map(str::to_owned));
    let level = cli_level.unwrap_or_else(|| config.get("log-level", "info".to_owned()));
    let level = log_level(&level);
    init_logger(level);

    log::info!("{} {}", util::APP_NAME, util::version_info());
    log::info!("config file {}", config.file().display());
    if let Some(err) = config.parse_error() {
        log::warn!("Config parse error in {}: {err}", config.file().display());
    }

    let command = parse_command(&args);
    let use_dbus = config.get("dbus", true);
    let bus = dbus::session_bus();

    // already running? hand over the command line before starting Chromium
    if use_dbus {
        if let Some(conn) = &bus {
            if dbus::is_running(conn) {
                dbus::forward(conn, &command);
                return;
            }
        }
    }

    let profile = util::webview_data_dir();
    if let Err(err) = std::fs::create_dir_all(&profile) {
        log::error!("cannot create {}: {err}", profile.display());
    }
    log::info!("browser profile {}", profile.display());
    let spell_langs: Vec<String> = config.get("spellcheck-languages", i18n::preferred_languages());
    log::debug!("spellLangs {spell_langs:?}");
    util::set_spellcheck_preferences(&profile, config.get("spellcheck", true), &spell_langs);
    let profile = cef::CefString::from(profile.to_string_lossy().as_ref());
    let user_agent: Option<String> = config.get("user-agent", None);
    let settings = cef::Settings {
        // Chromium's sandbox needs a setuid helper or unprivileged user
        // namespaces, which some distributions (Ubuntu 24.04) restrict
        no_sandbox: 1,
        root_cache_path: profile.clone(),
        cache_path: profile,
        persist_session_cookies: 1,
        user_agent: cef::CefString::from(user_agent.as_deref().unwrap_or("")),
        log_severity: match level {
            log::LevelFilter::Trace | log::LevelFilter::Debug => cef::LogSeverity::INFO,
            log::LevelFilter::Off => cef::LogSeverity::DISABLE,
            _ => cef::LogSeverity::ERROR,
        },
        ..Default::default()
    };

    *app::STARTUP.lock().unwrap() = Some(app::Startup {
        config,
        bus,
        use_dbus,
        command,
    });

    let mut cef_app = app::TauriwaApp::new();
    if cef::initialize(
        Some(cef_args.as_main_args()),
        Some(&settings),
        Some(&mut cef_app),
        std::ptr::null_mut(),
    ) != 1
    {
        log::error!("cannot initialize Chromium (CEF)");
        std::process::exit(1);
    }
    // app::start() runs once the browser context is ready
    cef::run_message_loop();
    cef::shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_command(&args(&[])), Command::Show);
        assert_eq!(parse_command(&args(&["--toggle"])), Command::Toggle);
        assert_eq!(parse_command(&args(&["--hide"])), Command::Hide);
        assert_eq!(parse_command(&args(&["--quit"])), Command::Quit);
        assert_eq!(
            parse_command(&args(&["--toggle", "whatsapp://send?phone=1"])),
            Command::OpenUrl("whatsapp://send?phone=1".into())
        );
    }
}
