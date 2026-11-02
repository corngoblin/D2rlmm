//! D2rlmm — a mod manager for Diablo II: Resurrected on Linux.
//!
//! The window is a list of mods and two buttons. Everything else in here exists to
//! keep those two honest:
//!
//! * the list reflects what is actually on disk (rescan on every change, never a
//!   cached copy that disagrees with the folder the user can open),
//! * an install runs off the main loop so the window keeps painting, and reports
//!   back through the same log the error report is built from.
//!
//! There is deliberately no "launch game" button. D2R on Linux runs under
//! Steam/Proton or Lutris, and a mod manager that tries to launch someone else's
//! launcher is a support burden; the launch arguments are shown instead.

mod dialogs;
mod state;
mod window;

use adw::prelude::*;
use gtk::gdk;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

const APP_ID: &str = "io.github.corngoblin.D2rlmm";
const APP_NAME: &str = "D2rlmm";

// Holds the one window's `Rc<MainWindow>` for the life of the process.
//
// This app is single-window: `MainWindow` is created on `activate`, presented, and
// has to stay alive because all of its signal handlers hold weak references to it.
thread_local! {
    static LIVING_WINDOW: RefCell<Option<Rc<window::MainWindow>>> = const { RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    // Answering `--version` must not need a display: it is the cheapest proof that
    // an installed build is the one that was just packaged.
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("{}", version_string());
        return glib::ExitCode::SUCCESS;
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();
    // The window icon comes from the installed desktop file / icon theme, so there
    // is nothing to set here beyond the app id.

    app.connect_startup(|_| load_css());
    app.connect_activate(|app| {
        let window = window::MainWindow::new(app);
        // GTK owns the GtkWindow once it is presented, but *every* handler in
        // `window.rs` upgrades a `Weak<MainWindow>`. The `Rc` therefore needs an
        // owner that outlives this callback: without one it is dropped on return,
        // every `upgrade()` yields `None`, and each button, switch and menu item
        // becomes a silent no-op while the widgets themselves still draw, animate
        // and open popovers — which reads as a frozen app rather than a dead one.
        LIVING_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
        window.present();
    });

    app.run()
}

/// Loads the stylesheet.
///
/// The app is mostly stock libadwaita — GNOME's own widgets in GNOME's own style —
/// and the CSS only adds the few things the design system has no answer for: a
/// monospace log, dimmed metadata, and the mod-row's drag affordance.
fn load_css() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// The one-line version string, used by `--version` and the packaging smoke test.
pub fn version_string() -> String {
    format!(
        "{APP_NAME} {} (D2RMM API {}.{})",
        d2rlmm_core::VERSION,
        d2rlmm_core::D2RMM_API_VERSION.0,
        d2rlmm_core::D2RMM_API_VERSION.1
    )
}
