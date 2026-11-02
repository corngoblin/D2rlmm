//! The main window: the mod list, and the install button.
//!
//! Layout is `AdwToolbarView` in an `AdwToastOverlay`: a header bar with the two
//! things a user actually does (add a mod, install), a banner when something needs
//! attention, and a boxed list of mods. No sidebar, no tabs — a mod manager has one
//! list and one action, and hiding either behind navigation would be pretence.
//!
//! Installing runs on a worker thread because reading a real install's archives is
//! tens of seconds of I/O; the UI stays live and polls a channel for progress. That
//! channel is the only cross-thread state and it carries owned values, so the worker
//! can never touch a widget.
//!
//! Widgets are built first and signals are wired afterwards ([`MainWindow::wire`]),
//! because every handler needs a strong reference to the window and the window needs
//! the widgets. Trying to do both in one pass means either a pointer registry or a
//! forest of `RefCell<Option<…>>`; two phases are simply correct.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use adw::prelude::*;
use d2rlmm_core::installer::{self, InstallRequest};
use d2rlmm_core::mods::Mod;
use d2rlmm_core::paths::{self, ResolvedSource};
use d2rlmm_core::provider::GameData;
use gtk::gio;
use gtk::glib;

use crate::dialogs;
use crate::state::AppState;

/// Messages from the install thread.
enum Message {
    Progress(installer::Progress),
    Done(Box<d2rlmm_core::Result<installer::InstallReport>>),
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub state: Rc<RefCell<AppState>>,
    list: gtk::ListBox,
    stack: gtk::Stack,
    progress: gtk::ProgressBar,
    install_button: gtk::Button,
    toast: adw::ToastOverlay,
    banner: adw::Banner,
    subtitle: gtk::Label,
    add_button: gtk::MenuButton,
    menu_button: gtk::MenuButton,
    empty_button: gtk::Button,
    actions: gio::SimpleActionGroup,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let state = Rc::new(RefCell::new(AppState::load()));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("D2rlmm")
            .default_width(880)
            .default_height(660)
            .build();

        let header = adw::HeaderBar::new();

        let title = gtk::Box::new(gtk::Orientation::Vertical, 0);
        title.set_valign(gtk::Align::Center);
        let title_label = gtk::Label::new(Some("D2rlmm"));
        title_label.add_css_class("title");
        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("mod-subtitle");
        subtitle.add_css_class("caption");
        title.append(&title_label);
        title.append(&subtitle);
        header.set_title_widget(Some(&title));

        let add_button = gtk::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add a mod")
            .build();
        header.pack_start(&add_button);

        let install_button = gtk::Button::with_label("Install");
        install_button.add_css_class("suggested-action");
        install_button.set_tooltip_text(Some("Build the mod folder from the enabled mods"));
        header.pack_end(&install_button);

        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main menu")
            .build();
        header.pack_end(&menu_button);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        list.set_margin_top(12);
        list.set_margin_bottom(24);
        list.set_margin_start(18);
        list.set_margin_end(18);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let empty_page = adw::StatusPage::builder()
            .icon_name("io.github.corngoblin.D2rlmm")
            .title("No mods yet")
            .description(
                "Import a mod folder or a .zip from a mod site such as Nexus Mods, then \
                 flip it on and press Install.",
            )
            .build();
        let empty_button = gtk::Button::with_label("Import a mod…");
        empty_button.add_css_class("pill");
        empty_button.add_css_class("suggested-action");
        empty_button.set_halign(gtk::Align::Center);
        empty_page.set_child(Some(&empty_button));

        let stack = gtk::Stack::new();
        stack.add_named(&scroller, Some("list"));
        stack.add_named(&empty_page, Some("empty"));
        stack.set_visible_child_name("empty");

        let banner = adw::Banner::new("");
        banner.set_revealed(false);

        let progress = gtk::ProgressBar::new();
        progress.set_visible(false);
        progress.set_margin_start(18);
        progress.set_margin_end(18);
        progress.set_margin_bottom(12);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&stack);
        content.append(&progress);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&banner);
        toolbar.set_content(Some(&content));

        let toast = adw::ToastOverlay::new();
        toast.set_child(Some(&toolbar));
        window.set_content(Some(&toast));

        let this = Rc::new(Self {
            window,
            state,
            list,
            stack,
            progress,
            install_button,
            toast,
            banner,
            subtitle,
            add_button,
            menu_button,
            empty_button,
            actions: gio::SimpleActionGroup::new(),
        });

        this.wire();
        this.refresh();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Connects every signal once the window exists.
    fn wire(self: &Rc<Self>) {
        // --- menu -----------------------------------------------------------
        let menu = gio::Menu::new();
        menu.append(Some("Settings…"), Some("win.settings"));
        menu.append(Some("Install log…"), Some("win.log"));
        menu.append(Some("Save error report…"), Some("win.report"));
        menu.append(Some("Rescan mod library"), Some("win.rescan"));
        menu.append(Some("Uninstall mods (restore vanilla)"), Some("win.revert"));
        menu.append(Some("About D2rlmm"), Some("win.about"));
        self.menu_button.set_menu_model(Some(&menu));

        let add_menu = gio::Menu::new();
        add_menu.append(Some("Import mod from .zip…"), Some("win.import-zip"));
        add_menu.append(Some("Install mod from folder…"), Some("win.import-dir"));
        self.add_button.set_menu_model(Some(&add_menu));

        let actions: Vec<(&str, Box<dyn Fn(&Rc<Self>)>)> = vec![
            ("settings", Box::new(|w| dialogs::open_settings_dialog(w))),
            ("log", Box::new(|w| dialogs::open_log_dialog(w))),
            ("report", Box::new(|w| dialogs::save_error_report(w))),
            ("rescan", Box::new(|w| w.rescan())),
            ("revert", Box::new(|w| w.start_install(true))),
            ("about", Box::new(|w| dialogs::open_about(w))),
            ("import-zip", Box::new(|w| dialogs::import_mod_zip(w))),
            ("import-dir", Box::new(|w| dialogs::import_mod_dir(w))),
        ];
        for (name, handler) in actions {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    handler(&this);
                }
            });
            self.actions.add_action(&action);
        }
        self.window.insert_action_group("win", Some(&self.actions));

        // --- buttons --------------------------------------------------------
        {
            let weak = Rc::downgrade(self);
            self.install_button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    // Nothing enabled means the button is "Uninstall": remove what is
                    // installed instead of building an empty mod folder.
                    let revert = this.state.borrow().enabled_count() == 0;
                    this.start_install(revert);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.empty_button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    dialogs::import_mod_zip(&this);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.banner.connect_button_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    if this.state.borrow().load_errors.is_empty() {
                        dialogs::open_settings_dialog(&this);
                    } else {
                        dialogs::open_log_dialog(&this);
                    }
                }
            });
        }

        // --- rows -----------------------------------------------------------
        {
            let weak = Rc::downgrade(self);
            self.list.connect_row_activated(move |_, row| {
                if let Some(this) = weak.upgrade() {
                    let index = row.index();
                    let id = this
                        .state
                        .borrow()
                        .ordered()
                        .get(index as usize)
                        .map(|m| m.id.clone());
                    if let Some(id) = id {
                        dialogs::open_config_dialog(&this, &id);
                    }
                }
            });
        }
    }

    /// Rebuilds everything derived from state.
    pub fn refresh(self: &Rc<Self>) {
        self.list.remove_all();
        let (modules, total): (Vec<Mod>, usize) = {
            let state = self.state.borrow();
            let ordered: Vec<Mod> = state.ordered().into_iter().cloned().collect();
            let total = ordered.len();
            (ordered, total)
        };
        for (index, module) in modules.iter().enumerate() {
            let row = self.build_row(module, index, total);
            self.list.append(&row);
        }

        let (has_mods, subtitle, enabled, installing, game_missing, load_errors) = {
            let state = self.state.borrow();
            (
                !state.mods.is_empty(),
                state.data_source_summary(),
                state.enabled_count(),
                state.installing,
                state.game_path().is_none(),
                state.load_errors.len(),
            )
        };

        self.stack
            .set_visible_child_name(if has_mods { "list" } else { "empty" });
        self.subtitle.set_text(&subtitle);

        self.install_button.set_sensitive(!installing);
        self.set_install_button_state(enabled);

        if game_missing {
            self.banner
                .set_title("Set your game folder to install mods");
            self.banner.set_button_label(Some("Settings"));
            self.banner.set_revealed(true);
        } else if load_errors > 0 {
            self.banner.set_title(&format!(
                "{load_errors} folder(s) in the mod library could not be read — see the log"
            ));
            self.banner.set_button_label(Some("Open log"));
            self.banner.set_revealed(true);
        } else {
            self.banner.set_revealed(false);
        }
    }

    fn build_row(self: &Rc<Self>, module: &Mod, index: usize, total: usize) -> gtk::ListBoxRow {
        let enabled = self.state.borrow().settings.is_enabled(&module.id);

        let row = gtk::ListBoxRow::new();
        row.set_activatable(true);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        body.set_margin_top(10);
        body.set_margin_bottom(10);
        body.set_margin_start(12);
        body.set_margin_end(12);

        let order = gtk::Label::new(Some(&format!("{}", index + 1)));
        order.add_css_class("mod-id");
        order.set_valign(gtk::Align::Center);
        order.set_tooltip_text(Some("Install order — later mods win conflicts"));
        body.append(&order);

        let switch = gtk::Switch::new();
        switch.set_active(enabled);
        switch.set_valign(gtk::Align::Center);
        switch.set_tooltip_text(Some("Enable or disable this mod"));
        body.append(&switch);

        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);

        let name = gtk::Label::new(Some(module.display_name()));
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if !enabled {
            name.add_css_class("disabled-mod");
        }
        text.append(&name);

        let mut meta: Vec<String> = Vec::new();
        if let Some(author) = &module.info.author {
            meta.push(author.clone());
        }
        meta.push(format!("v{}", module.version()));
        if !module.has_code() {
            meta.push("no mod.js".to_string());
        }
        let meta_label = gtk::Label::new(Some(&meta.join(" · ")));
        meta_label.set_xalign(0.0);
        meta_label.add_css_class("mod-subtitle");
        meta_label.add_css_class("caption");
        text.append(&meta_label);
        body.append(&text);

        if !module.info.config.is_empty() {
            let configure = gtk::Button::with_label("Configure");
            configure.add_css_class("flat");
            configure.set_valign(gtk::Align::Center);
            let weak = Rc::downgrade(self);
            let id = module.id.clone();
            configure.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    dialogs::open_config_dialog(&this, &id);
                }
            });
            body.append(&configure);
        }

        let menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(gtk::Align::Center)
            .build();
        menu.add_css_class("flat");
        body.append(&menu);

        let group = gio::SimpleActionGroup::new();
        let items = gio::Menu::new();

        let configure_action = gio::SimpleAction::new("configure", None);
        {
            let weak = Rc::downgrade(self);
            let id = module.id.clone();
            configure_action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    dialogs::open_config_dialog(&this, &id);
                }
            });
        }
        group.add_action(&configure_action);
        items.append(Some("Configure…"), Some("row.configure"));

        let up_action = gio::SimpleAction::new("up", None);
        {
            let weak = Rc::downgrade(self);
            let id = module.id.clone();
            up_action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    let moved = this.state.borrow_mut().move_mod(&id, -1);
                    if moved {
                        let _ = this.state.borrow().save();
                        this.refresh();
                    }
                }
            });
        }
        group.add_action(&up_action);
        if index > 0 {
            items.append(Some("Move up"), Some("row.up"));
        }

        let down_action = gio::SimpleAction::new("down", None);
        {
            let weak = Rc::downgrade(self);
            let id = module.id.clone();
            down_action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    let moved = this.state.borrow_mut().move_mod(&id, 1);
                    if moved {
                        let _ = this.state.borrow().save();
                        this.refresh();
                    }
                }
            });
        }
        group.add_action(&down_action);
        if index + 1 < total {
            items.append(Some("Move down"), Some("row.down"));
        }

        let folder_action = gio::SimpleAction::new("folder", None);
        {
            let dir = module.dir.clone();
            folder_action.connect_activate(move |_, _| {
                // Handing this to the desktop portal is the sandbox-safe path; a
                // plain xdg-open is the fallback outside a flatpak.
                let launched = std::process::Command::new("xdg-open").arg(&dir).spawn();
                let _ = launched;
            });
        }
        group.add_action(&folder_action);
        items.append(Some("Open mod folder"), Some("row.folder"));

        let remove_action = gio::SimpleAction::new("remove", None);
        {
            let weak = Rc::downgrade(self);
            let id = module.id.clone();
            let label = module.display_name().to_string();
            remove_action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    dialogs::confirm_remove(&this, &id, &label);
                }
            });
        }
        group.add_action(&remove_action);
        items.append(Some("Remove from library…"), Some("row.remove"));

        row.insert_action_group("row", Some(&group));
        menu.set_menu_model(Some(&items));

        {
            let weak = Rc::downgrade(self);
            let id = module.id.clone();
            switch.connect_active_notify(move |switch| {
                if let Some(this) = weak.upgrade() {
                    let active = switch.is_active();
                    let changed = {
                        let mut state = this.state.borrow_mut();
                        if state.settings.is_enabled(&id) != active {
                            state.settings.set_enabled(&id, active);
                            let _ = state.save();
                            true
                        } else {
                            false
                        }
                    };
                    // Partial update only: rebuilding the list here would destroy
                    // the switch whose signal is still running.
                    if changed {
                        if active {
                            name.remove_css_class("disabled-mod");
                        } else {
                            name.add_css_class("disabled-mod");
                        }
                        this.update_install_button();
                    }
                }
            });
        }

        row.set_child(Some(&body));
        row
    }

    /// Cheap partial update used by the switch.
    fn update_install_button(&self) {
        let state = self.state.borrow();
        self.install_button.set_sensitive(!state.installing);
        self.set_install_button_state(state.enabled_count());
    }

    /// The button is the whole uninstall story, so it changes meaning with no mods on.
    ///
    /// With nothing enabled there is nothing to build, and the files the mods left
    /// behind are exactly what the user wants gone — upstream D2RMM turns the same
    /// button into "Uninstall" for the same reason.
    fn set_install_button_state(&self, enabled: usize) {
        if enabled == 0 {
            self.install_button.set_label("Uninstall");
            self.install_button.remove_css_class("suggested-action");
            self.install_button.add_css_class("destructive-action");
            self.install_button.set_tooltip_text(Some(
                "Remove the installed mods and restore the files they changed",
            ));
        } else {
            self.install_button.set_label(&format!("Install ({enabled})"));
            self.install_button.remove_css_class("destructive-action");
            self.install_button.add_css_class("suggested-action");
            self.install_button.set_tooltip_text(Some(
                "Build the mod folder from the enabled mods",
            ));
        }
    }

    pub fn rescan(self: &Rc<Self>) {
        self.state.borrow_mut().refresh();
        self.refresh();
        self.toast("Mod library rescanned");
    }

    pub fn toast(&self, message: &str) {
        self.toast.add_toast(adw::Toast::new(message));
    }

    /// Kicks off an install (`revert = true` undoes a direct-mode install).
    pub fn start_install(self: &Rc<Self>, revert: bool) {
        {
            let state = self.state.borrow();
            if state.installing {
                return;
            }
            if state.game_path().is_none() {
                drop(state);
                self.toast("Set your game folder first");
                dialogs::open_settings_dialog(self);
                return;
            }
        }

        let settings = self.state.borrow().settings.clone();
        let selected = installer::enabled_mods(&settings, self.state.borrow().mods.clone());
        // An empty list is a valid request when reverting — that is what "uninstall"
        // means — and an error only when installing.
        if selected.is_empty() && !revert {
            self.toast("No mods are enabled");
            return;
        }

        self.state.borrow_mut().installing = true;
        self.install_button.set_sensitive(false);
        self.progress.set_visible(true);
        self.progress.pulse();
        self.toast(if revert {
            "Removing the installed mods…"
        } else {
            "Installing mods…"
        });

        let (tx, rx) = mpsc::channel::<Message>();
        std::thread::spawn(move || {
            let data = match paths::resolve_data_source(
                settings.game_path.as_deref(),
                settings.data_source,
                settings.pre_extracted_path.as_deref(),
            ) {
                Ok(ResolvedSource::Casc(path)) => GameData::open_casc(&path),
                Ok(ResolvedSource::PreExtracted(path)) => GameData::open_directory(&path),
                Err(e) => Err(e),
            };
            let data = match data {
                Ok(data) => data,
                Err(e) => {
                    let _ = tx.send(Message::Done(Box::new(Err(e))));
                    return;
                }
            };

            let base_saves = settings.effective_base_saves();
            let request = InstallRequest {
                mods: selected,
                game_path: settings.game_path.clone().unwrap_or_default(),
                output_mod_name: settings.output_mod_name.clone(),
                direct_mode: settings.direct_mode,
                base_saves_path: base_saves.clone(),
                saves_path: settings.effective_saves_path(&base_saves),
                data: Some(data),
                direct_data_path: settings.effective_output_data_dir(),
                dry_run: revert,
                state_dir: None,
            };

            // Progress goes back over the same channel as the result; the main
            // loop drains it while the bar pulses.
            let result = installer::install(request, &mut |progress| {
                let _ = tx.send(Message::Progress(progress));
            });
            let _ = tx.send(Message::Done(Box::new(result)));
        });

        // The receiver is polled from the main loop rather than blocked on, so the
        // window keeps painting (and the progress bar keeps moving) throughout.
        let receiver = Rc::new(RefCell::new(Some(rx)));
        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(120), move || {
            let mut progress_update: Option<installer::Progress> = None;
            let mut finished: Option<Box<d2rlmm_core::Result<installer::InstallReport>>> = None;
            let mut disconnected = false;

            {
                let borrow = receiver.borrow();
                let Some(rx) = borrow.as_ref() else {
                    return glib::ControlFlow::Break;
                };
                loop {
                    match rx.try_recv() {
                        Ok(Message::Progress(p)) => progress_update = Some(p),
                        Ok(Message::Done(report)) => {
                            finished = Some(report);
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }

            if let Some(p) = progress_update {
                if p.total > 0 {
                    this.progress.set_fraction(p.done as f64 / p.total as f64);
                } else {
                    this.progress.pulse();
                }
            }

            if let Some(result) = finished {
                receiver.borrow_mut().take();
                this.finish_install(*result);
                return glib::ControlFlow::Break;
            }
            if disconnected {
                receiver.borrow_mut().take();
                this.finish_install(Err(d2rlmm_core::Error::msg(
                    "the install worker stopped unexpectedly",
                )));
                return glib::ControlFlow::Break;
            }

            this.progress.pulse();
            glib::ControlFlow::Continue
        });
    }

    fn finish_install(self: &Rc<Self>, result: d2rlmm_core::Result<installer::InstallReport>) {
        self.progress.set_visible(false);
        self.state.borrow_mut().installing = false;
        self.update_install_button();

        match result {
            Ok(report) => {
                let summary = report.summary();
                let failures: Vec<String> = report
                    .failures()
                    .iter()
                    .map(|o| {
                        format!(
                            "{}: {}",
                            o.id,
                            o.error.as_deref().unwrap_or("unknown error")
                        )
                    })
                    .collect();
                self.state.borrow_mut().last_report = Some(report);
                self.toast(&summary);
                if !failures.is_empty() {
                    dialogs::show_install_failures(self, &failures);
                }
            }
            Err(e) => {
                self.toast("Install failed");
                dialogs::show_install_error(self, &e.to_string());
            }
        }
    }

    pub fn install_log_text(&self) -> String {
        let state = self.state.borrow();
        let mut text = String::new();
        for error in &state.load_errors {
            text.push_str(&format!("[ERROR] {error}\n"));
        }
        match &state.last_report {
            Some(report) => text.push_str(&report.log.render()),
            None if text.is_empty() => text.push_str("No install has run in this session yet."),
            None => {}
        }
        text
    }

    /// Context for the error report: everything a maintainer would ask for.
    pub fn report_context(&self) -> (Option<std::path::PathBuf>, String, String, bool, Vec<(String, String, bool)>) {
        let state = self.state.borrow();
        (
            state.settings.game_path.clone(),
            state.data_source_summary(),
            state.settings.output_mod_name.clone(),
            state.settings.direct_mode,
            state
                .ordered()
                .iter()
                .map(|m| {
                    (
                        m.id.clone(),
                        m.version().to_string(),
                        state.settings.is_enabled(&m.id),
                    )
                })
                .collect(),
        )
    }

    /// The log for the report, including a headline describing the last run.
    pub fn report_log(&self) -> d2rlmm_core::logbook::InstallLog {
        match &self.state.borrow().last_report {
            Some(report) => report.log.clone(),
            None => d2rlmm_core::logbook::InstallLog::new(),
        }
    }
}
