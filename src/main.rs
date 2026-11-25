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

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tao::event_loop::EventLoopBuilder;

use crate::config::JsonConfig;
use crate::dbus::{Command, Instance};
use crate::events::UserEvent;

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
    init_logger(log_level(&level));

    log::info!("{} {}", util::APP_NAME, util::version_info());
    log::info!("config file {}", config.file().display());
    if let Some(err) = config.parse_error() {
        log::warn!("Config parse error in {}: {err}", config.file().display());
    }

    let command = parse_command(&args);
    let use_dbus = config.get("dbus", true);
    let bus = dbus::session_bus();

    // already running? hand over the command line before initializing GTK
    if use_dbus {
        if let Some(conn) = &bus {
            if dbus::is_running(conn) {
                dbus::forward(conn, &command);
                return;
            }
        }
    }

    // app_id on Wayland, WM_CLASS on X11: matches StartupWMClass of the .desktop file
    glib::set_prgname(Some(util::APP_NAME));
    glib::set_application_name(util::PRODUCT_NAME);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let visible = Arc::new(AtomicBool::new(false));

    if use_dbus {
        if let Some(conn) = &bus {
            if let Instance::Secondary =
                dbus::register(conn, event_loop.create_proxy(), visible.clone(), &command)
            {
                return;
            }
        }
    }

    let mut app = match app::App::new(&event_loop, config, bus, visible, &command) {
        Ok(app) => app,
        Err(err) => {
            log::error!("{err}");
            std::process::exit(1);
        }
    };
    if !app.check_config() {
        return;
    }

    event_loop.run(move |event, _target, control_flow| app.handle(event, control_flow));
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
