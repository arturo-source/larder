// Release builds on Windows are GUI apps: no console window next to the main window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod runtime;
mod s3;
mod ui;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

fn main() -> glib::ExitCode {
    // `<app> --connect <profile>` connects to a saved profile on startup. GApplication would reject
    // unknown options, so we parse ours here and only pass the program name on.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("larder {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    let connect_to = args
        .iter()
        .position(|a| a == "--connect")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let app = adw::Application::builder()
        .application_id(ui::APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|app| {
        gtk::Window::set_default_icon_name(ui::APP_ID);
        load_css();
        ui::set_accels(app);
    });
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let win = ui::Win::new(app);
        win.window.present();
        if let Some(name) = &connect_to {
            win.connect_profile(name);
        }
    });
    app.run_with_args(&args[..1])
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
