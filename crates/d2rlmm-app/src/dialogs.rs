//! Every window that is not the main one: mod configuration, settings, logs, and the
//! small confirmations.
//!
//! # The config dialog is generated, not hand-written
//!
//! A mod declares its options in `mod.json`; this module renders them. That is why
//! there is no code here for any particular mod — `checkbox` becomes a switch,
//! `select` a combo, `section` an expander, and `visible` bindings are re-evaluated
//! after every change so dependent rows appear and disappear as the schema says. The
//! alternative (a bespoke panel per mod) does not scale and would silently break
//! every third-party mod that adds an option.
//!
//! # Saves are immediate
//!
//! Settings and mod configs are written the moment they change. A mod manager
//! holding unsaved state is a trap: users install, see nothing change, and blame the
//! mod. The only stateful thing in the config dialog is the "Reset" button.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use d2rlmm_core::logbook::{self, ReportContext};
use d2rlmm_core::mods;
use d2rlmm_core::paths;
use d2rlmm_core::schema::{self, FieldType, ModConfig};
use d2rlmm_core::settings::{DataSourceKind, Settings};
use gtk::gio;
use serde_json::Value;

use crate::window::MainWindow;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn parent(window: &Rc<MainWindow>) -> Option<&gtk::Window> {
    Some(window.window.upcast_ref::<gtk::Window>())
}

fn alert(heading: &str, body: &str) -> adw::AlertDialog {
    adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build()
}

fn dismiss_button(dialog: &adw::AlertDialog) {
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
}

fn pick_folder<F>(window: &Rc<MainWindow>, title: &str, on_pick: F)
where
    F: FnOnce(PathBuf) + 'static,
{
    let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
    let window = window.window.clone();
    dialog.select_folder(
        Some(&window),
        gio::Cancellable::NONE,
        move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    on_pick(path);
                }
            }
        },
    );
}

// ---------------------------------------------------------------------------
// mod configuration
// ---------------------------------------------------------------------------

/// Opens the generated settings dialog for one mod.
pub fn open_config_dialog(window: &Rc<MainWindow>, mod_id: &str) {
    let Some(module) = window.state.borrow().find(mod_id).cloned() else {
        return;
    };
    let config = Rc::new(RefCell::new(module.config()));

    if module.info.config.is_empty() {
        let dialog = alert(
            "No options",
            &format!(
                "{} has nothing to configure — enabling it is the whole decision.",
                module.display_name()
            ),
        );
        dismiss_button(&dialog);
        dialog.present(parent(window));
        return;
    }

    let dialog = adw::Dialog::builder()
        .title(module.display_name())
        .content_width(560)
        .content_height(640)
        .build();

    let header = adw::HeaderBar::new();
    let reset = gtk::Button::with_label("Reset");
    reset.add_css_class("flat");
    header.pack_start(&reset);
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    header.pack_end(&save);

    let editor = ConfigEditor::new(config.clone());

    let page = adw::PreferencesPage::new();
    page.add(&editor.group());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&page));
    dialog.set_child(Some(&toolbar));

    editor.refresh_visibility();

    {
        let window = window.clone();
        let dialog_clone = dialog.clone();
        let config = config.clone();
        let mod_id = module.id.clone();
        save.connect_clicked(move |_| {
            {
                let mut state = window.state.borrow_mut();
                if let Some(module) = state.find_mut(&mod_id) {
                    let _ = module.save_config(&config.borrow());
                    let name = module.display_name().to_string();
                    drop(state);
                    window.toast(&format!("Saved settings for {name}"));
                }
            }
            dialog_clone.close();
        });
    }

    {
        let window = window.clone();
        let dialog_clone = dialog.clone();
        let mod_id = module.id.clone();
        reset.connect_clicked(move |_| {
            {
                let mut state = window.state.borrow_mut();
                if let Some(module) = state.find_mut(&mod_id) {
                    let mut fresh = module.config();
                    fresh.reset();
                    let _ = module.save_config(&fresh);
                }
            }
            dialog_clone.close();
            // Rebuild from the now-default config: cheaper to reason about than
            // resetting two dozen widgets in place.
            open_config_dialog(&window, &mod_id);
            window.toast("Options reset to defaults");
        });
    }

    dialog.present(parent(window));
}

/// Drives the generated option rows.
///
/// Holds the config being edited and the (field, widget) pairs whose visibility is
/// conditional. Rows are wired as they are built, which keeps each control's
/// meaning next to its construction instead of in a second pass that has to find
/// widgets again by type.
#[derive(Clone)]
struct ConfigEditor {
    config: Rc<RefCell<ModConfig>>,
    visibility: Rc<RefCell<Vec<(Value, gtk::Widget)>>>,
}

impl ConfigEditor {
    fn new(config: Rc<RefCell<ModConfig>>) -> Self {
        Self {
            config,
            visibility: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn group(&self) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::new();
        group.set_title(&self.title());
        let fields: Vec<Value> = self.config.borrow().fields().to_vec();
        for field in &fields {
            if let Some(row) = self.build_row(field) {
                group.add(&row);
            }
        }
        group
    }

    fn title(&self) -> String {
        let config = self.config.borrow();
        let mut fields = Vec::new();
        schema::flatten(config.fields(), &mut fields);
        let count = fields
            .iter()
            .filter(|f| !matches!(FieldType::of(f), Some(FieldType::Section)))
            .count();
        format!("{count} option{}", if count == 1 { "" } else { "s" })
    }

    /// Applies the schema's `visible` bindings. Called after every change.
    fn refresh_visibility(&self) {
        let config = self.config.borrow();
        for (field, widget) in self.visibility.borrow().iter() {
            widget.set_visible(config.is_visible(field));
        }
    }

    fn build_row(&self, field: &Value) -> Option<gtk::Widget> {
        let id = schema::field_id(field)?.to_string();
        let name = schema::field_name(field).unwrap_or(&id).to_string();
        let description = schema::field_description(field).map(str::to_string);
        let value = self
            .config
            .borrow()
            .get(&id)
            .cloned()
            .unwrap_or(Value::Null);

        let row: gtk::Widget = match FieldType::of(field)? {
            FieldType::Section => {
                let expander = adw::ExpanderRow::new();
                expander.set_title(&name);
                if let Some(description) = &description {
                    expander.set_subtitle(description);
                }
                expander.set_expanded(self.config.borrow().is_expanded(field));
                for child in schema::children(field) {
                    if let Some(child_row) = self.build_row(child) {
                        expander.add_row(&child_row);
                    }
                }
                {
                    let editor = self.clone();
                    let id = id.clone();
                    expander.connect_expanded_notify(move |row| {
                        editor
                            .config
                            .borrow_mut()
                            .set_expanded(&id, row.is_expanded());
                        editor.refresh_visibility();
                    });
                }
                expander.upcast()
            }

            FieldType::Checkbox => {
                let switch = adw::SwitchRow::new();
                switch.set_title(&name);
                if let Some(description) = &description {
                    switch.set_subtitle(description);
                }
                switch.set_active(schema::truthy(&value));
                {
                    let editor = self.clone();
                    let id = id.clone();
                    switch.connect_active_notify(move |row| {
                        editor
                            .config
                            .borrow_mut()
                            .set(&id, Value::Bool(row.is_active()));
                        editor.refresh_visibility();
                    });
                }
                switch.upcast()
            }

            FieldType::Number => {
                let min = field.get("minValue").and_then(Value::as_f64).unwrap_or(0.0);
                let max = field
                    .get("maxValue")
                    .and_then(Value::as_f64)
                    .unwrap_or(1_000_000.0);
                let spin = adw::SpinRow::with_range(min, max, 1.0);
                spin.set_title(&name);
                if let Some(description) = &description {
                    spin.set_subtitle(description);
                }
                spin.set_value(value.as_f64().unwrap_or(min));
                {
                    let editor = self.clone();
                    let id = id.clone();
                    spin.connect_value_notify(move |row| {
                        let value = row.value();
                        // Integers stay integers: a mod reading
                        // `config.stackSize.length` would notice a float.
                        let json = if value.fract() == 0.0 {
                            serde_json::json!(value as i64)
                        } else {
                            serde_json::json!(value)
                        };
                        editor.config.borrow_mut().set(&id, json);
                        editor.refresh_visibility();
                    });
                }
                spin.upcast()
            }

            FieldType::Text => {
                let entry = adw::EntryRow::new();
                entry.set_title(&name);
                if let Some(description) = &description {
                    entry.set_tooltip_text(Some(description));
                }
                entry.set_text(value.as_str().unwrap_or_default());
                {
                    let editor = self.clone();
                    let id = id.clone();
                    entry.connect_changed(move |row| {
                        editor
                            .config
                            .borrow_mut()
                            .set(&id, Value::String(row.text().to_string()));
                        editor.refresh_visibility();
                    });
                }
                entry.upcast()
            }

            FieldType::Select => {
                let options = schema::options(field);
                let labels: Vec<&str> = options.iter().map(|(label, _, _)| label.as_str()).collect();
                let combo = adw::ComboRow::new();
                combo.set_title(&name);
                if let Some(description) = &description {
                    combo.set_subtitle(description);
                }
                combo.set_model(Some(&gtk::StringList::new(&labels)));
                combo.set_selected(
                    options
                        .iter()
                        .position(|(_, candidate, _)| *candidate == value)
                        .unwrap_or(0) as u32,
                );
                {
                    let editor = self.clone();
                    let id = id.clone();
                    let options = options.clone();
                    combo.connect_selected_notify(move |row| {
                        if let Some((_, value, _)) = options.get(row.selected() as usize) {
                            editor.config.borrow_mut().set(&id, value.clone());
                            editor.refresh_visibility();
                        }
                    });
                }
                combo.upcast()
            }

            FieldType::Color => {
                let action = adw::ActionRow::new();
                action.set_title(&name);
                if let Some(description) = &description {
                    action.set_subtitle(description);
                }
                let button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
                button.set_rgba(&rgba_from_config(&value));
                button.set_valign(gtk::Align::Center);
                action.add_suffix(&button);
                {
                    let editor = self.clone();
                    let id = id.clone();
                    button.connect_rgba_notify(move |button| {
                        editor
                            .config
                            .borrow_mut()
                            .set(&id, config_from_rgba(&button.rgba()));
                        editor.refresh_visibility();
                    });
                }
                action.upcast()
            }
        };

        self.visibility.borrow_mut().push((field.clone(), row.clone()));
        Some(row)
    }
}

/// `[r, g, b, a]` from a mod's config — channels 0–255, alpha 0–1 — into a `GdkRGBA`.
fn rgba_from_config(value: &Value) -> gtk::gdk::RGBA {
    let channels: Vec<f64> = value
        .as_array()
        .map(|items| items.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
        .unwrap_or_else(|| vec![255.0, 255.0, 255.0, 1.0]);
    let get = |index: usize, fallback: f64| channels.get(index).copied().unwrap_or(fallback);
    gtk::gdk::RGBA::new(
        get(0, 255.0) as f32 / 255.0,
        get(1, 255.0) as f32 / 255.0,
        get(2, 255.0) as f32 / 255.0,
        get(3, 1.0) as f32,
    )
}

fn config_from_rgba(rgba: &gtk::gdk::RGBA) -> Value {
    serde_json::json!([
        (rgba.red() * 255.0).round() as i64,
        (rgba.green() * 255.0).round() as i64,
        (rgba.blue() * 255.0).round() as i64,
        (rgba.alpha() as f64 * 1000.0).round() / 1000.0,
    ])
}

// ---------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------

pub fn open_settings_dialog(window: &Rc<MainWindow>) {
    let dialog = adw::Dialog::builder()
        .title("Settings")
        .content_width(600)
        .content_height(700)
        .build();

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let page = adw::PreferencesPage::new();

    // --- game ---------------------------------------------------------------
    let game_group = adw::PreferencesGroup::new();
    game_group.set_title("Game");

    let game_row = adw::ActionRow::new();
    game_row.set_title("Game folder");
    let game_button = gtk::Button::with_label("Browse…");
    game_button.add_css_class("flat");
    game_button.set_valign(gtk::Align::Center);
    game_row.add_suffix(&game_button);
    game_group.add(&game_row);

    let source_row = adw::ComboRow::new();
    source_row.set_title("Vanilla data from");
    let source_labels: Vec<&str> = DataSourceKind::ALL.iter().map(|k| k.label()).collect();
    source_row.set_model(Some(&gtk::StringList::new(&source_labels)));
    source_row.set_selected(
        DataSourceKind::ALL
            .iter()
            .position(|k| *k == window.state.borrow().settings.data_source)
            .unwrap_or(0) as u32,
    );
    game_group.add(&source_row);

    let extracted_row = adw::ActionRow::new();
    extracted_row.set_title("Extracted data folder");
    let extracted_button = gtk::Button::with_label("Browse…");
    extracted_button.add_css_class("flat");
    extracted_button.set_valign(gtk::Align::Center);
    extracted_row.add_suffix(&extracted_button);
    game_group.add(&extracted_row);

    page.add(&game_group);

    // --- install target -----------------------------------------------------
    let target_group = adw::PreferencesGroup::new();
    target_group.set_title("Install target");
    target_group.set_description(Some(
        "By default the merged data is written to \
         mods/<name>/<name>.mpq, which the game loads with -mod <name> -txt. \
         For a game running from extracted data (-direct -txt), write straight into \
         a data folder instead.",
    ));

    let direct_row = adw::SwitchRow::new();
    direct_row.set_title("Write into a data folder");
    direct_row.set_subtitle("For -direct -txt installs, including extracted trees");
    direct_row.set_active(window.state.borrow().settings.direct_mode);
    target_group.add(&direct_row);

    let data_dir_row = adw::ActionRow::new();
    data_dir_row.set_title("Data folder");
    let data_dir_button = gtk::Button::with_label("Browse…");
    data_dir_button.add_css_class("flat");
    data_dir_button.set_valign(gtk::Align::Center);
    data_dir_row.add_suffix(&data_dir_button);
    target_group.add(&data_dir_row);

    let name_row = adw::EntryRow::new();
    name_row.set_title("Mod folder name");
    name_row.set_text(&window.state.borrow().settings.output_mod_name);
    target_group.add(&name_row);

    let launch_row = adw::ActionRow::new();
    launch_row.set_title("Launch options");
    launch_row.set_subtitle("Give these to the game in Steam or Lutris");
    let copy_button = gtk::Button::with_label("Copy");
    copy_button.add_css_class("flat");
    copy_button.set_valign(gtk::Align::Center);
    launch_row.add_suffix(&copy_button);
    target_group.add(&launch_row);

    page.add(&target_group);

    // --- saves --------------------------------------------------------------
    let saves_group = adw::PreferencesGroup::new();
    saves_group.set_title("Saves");
    let saves_row = adw::ActionRow::new();
    saves_row.set_title("Save folder");
    let saves_button = gtk::Button::with_label("Browse…");
    saves_button.add_css_class("flat");
    saves_button.set_valign(gtk::Align::Center);
    saves_row.add_suffix(&saves_button);
    saves_group.add(&saves_row);
    page.add(&saves_group);

    // --- library ------------------------------------------------------------
    let library_group = adw::PreferencesGroup::new();
    library_group.set_title("Mod library");
    let library_row = adw::ActionRow::new();
    library_row.set_title("Folder");
    let library_button = gtk::Button::with_label("Open");
    library_button.add_css_class("flat");
    library_button.set_valign(gtk::Align::Center);
    library_row.add_suffix(&library_button);
    library_group.add(&library_row);
    page.add(&library_group);

    // --- about --------------------------------------------------------------
    let about_group = adw::PreferencesGroup::new();
    about_group.set_title("About");
    let version_row = adw::ActionRow::new();
    version_row.set_title("Version");
    version_row.set_subtitle(&format!(
        "D2rlmm {} · D2RMM API {}.{}",
        d2rlmm_core::VERSION,
        d2rlmm_core::D2RMM_API_VERSION.0,
        d2rlmm_core::D2RMM_API_VERSION.1
    ));
    about_group.add(&version_row);
    page.add(&about_group);

    toolbar.set_content(Some(&page));
    dialog.set_child(Some(&toolbar));

    // --- behaviour ----------------------------------------------------------
    let update_labels = {
        let window = window.clone();
        let game_row = game_row.clone();
        let extracted_row = extracted_row.clone();
        let data_dir_row = data_dir_row.clone();
        let saves_row = saves_row.clone();
        let launch_row = launch_row.clone();
        let library_row = library_row.clone();
        move || {
            let state = window.state.borrow();
            game_row.set_subtitle(
                &state
                    .settings
                    .game_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Not set — press Browse".to_string()),
            );
            extracted_row.set_subtitle(
                &state
                    .settings
                    .pre_extracted_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Not set".to_string()),
            );
            let data_dir = state
                .settings
                .effective_output_data_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Not set".to_string());
            data_dir_row.set_subtitle(&data_dir);
            data_dir_row.set_visible(state.settings.direct_mode);

            saves_row.set_subtitle(
                &state
                    .settings
                    .effective_base_saves()
                    .display()
                    .to_string(),
            );

            let args = if state.settings.direct_mode {
                "-direct -txt".to_string()
            } else {
                state.launch_arguments()
            };
            launch_row.set_subtitle(&args);
            library_row.set_subtitle(&paths::mods_dir().display().to_string());
        }
    };
    update_labels();

    {
        let window = window.clone();
        let update_labels = update_labels.clone();
        game_button.connect_clicked(move |_| {
            let window = window.clone();
            let callback_window = window.clone();
            let update_labels = update_labels.clone();
            pick_folder(&window, "Select the Diablo II: Resurrected folder", move |path| {
                let window = callback_window;
                {
                    let mut state = window.state.borrow_mut();
                    state.settings.game_path = Some(path);
                    let _ = state.save();
                }
                update_labels();
                window.refresh();
            });
        });
    }

    {
        let window = window.clone();
        let update_labels = update_labels.clone();
        extracted_button.connect_clicked(move |_| {
            let window = window.clone();
            let callback_window = window.clone();
            let update_labels = update_labels.clone();
            pick_folder(&window, "Select the extracted data folder", move |path| {
                let window = callback_window;
                {
                    let mut state = window.state.borrow_mut();
                    state.settings.pre_extracted_path = Some(path);
                    let _ = state.save();
                }
                update_labels();
                window.refresh();
            });
        });
    }

    {
        let window = window.clone();
        let update_labels = update_labels.clone();
        data_dir_button.connect_clicked(move |_| {
            let window = window.clone();
            let callback_window = window.clone();
            let update_labels = update_labels.clone();
            pick_folder(&window, "Select the data folder to install into", move |path| {
                let window = callback_window;
                {
                    let mut state = window.state.borrow_mut();
                    state.settings.direct_data_path = Some(path);
                    let _ = state.save();
                }
                update_labels();
            });
        });
    }

    {
        let window = window.clone();
        let update_labels = update_labels.clone();
        saves_button.connect_clicked(move |_| {
            let window = window.clone();
            let callback_window = window.clone();
            let update_labels = update_labels.clone();
            pick_folder(&window, "Select the game's save folder", move |path| {
                let window = callback_window;
                {
                    let mut state = window.state.borrow_mut();
                    state.settings.base_saves_path = Some(path);
                    let _ = state.save();
                }
                update_labels();
            });
        });
    }

    {
        let window = window.clone();
        source_row.connect_selected_notify(move |row| {
            let kind = DataSourceKind::ALL
                .get(row.selected() as usize)
                .copied()
                .unwrap_or_default();
            let mut state = window.state.borrow_mut();
            state.settings.data_source = kind;
            let _ = state.save();
            drop(state);
            window.refresh();
        });
    }

    {
        let window = window.clone();
        let update_labels = update_labels.clone();
        direct_row.connect_active_notify(move |row| {
            {
                let mut state = window.state.borrow_mut();
                state.settings.direct_mode = row.is_active();
                let _ = state.save();
            }
            update_labels();
            window.refresh();
        });
    }

    {
        let window = window.clone();
        name_row.connect_changed(move |row| {
            let text = row.text().to_string();
            // A mod folder name is a single path component; refuse separators rather
            // than creating a nested folder the game will not find.
            if text.contains('/') || text.contains('\\') || text.trim().is_empty() {
                return;
            }
            let mut state = window.state.borrow_mut();
            state.settings.output_mod_name = text;
            let _ = state.save();
        });
    }

    {
        let window = window.clone();
        copy_button.connect_clicked(move |_| {
            let args = {
                let state = window.state.borrow();
                if state.settings.direct_mode {
                    "-direct -txt".to_string()
                } else {
                    state.launch_arguments()
                }
            };
            window.window.clipboard().set_text(&args);
            window.toast("Launch options copied");
        });
    }

    {
        let button = library_button;
        button.connect_clicked(move |_| {
            let dir = paths::mods_dir();
            let _ = paths::ensure_dir(&dir);
            let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
        });
    }

    dialog.present(parent(window));
}

// ---------------------------------------------------------------------------
// logs and reports
// ---------------------------------------------------------------------------

pub fn open_log_dialog(window: &Rc<MainWindow>) {
    let dialog = adw::Dialog::builder()
        .title("Install log")
        .content_width(760)
        .content_height(560)
        .build();

    let header = adw::HeaderBar::new();
    let save = gtk::Button::with_label("Save report…");
    save.add_css_class("suggested-action");
    header.pack_end(&save);
    let copy = gtk::Button::with_label("Copy");
    copy.add_css_class("flat");
    header.pack_start(&copy);

    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.add_css_class("install-log");
    view.buffer().set_text(&window.install_log_text());

    let scroller = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroller));
    dialog.set_child(Some(&toolbar));

    {
        let window = window.clone();
        copy.connect_clicked(move |_| {
            let text = window.install_log_text();
            window.window.clipboard().set_text(&text);
            window.toast("Log copied to the clipboard");
        });
    }

    {
        let window = window.clone();
        let dialog_clone = dialog.clone();
        save.connect_clicked(move |_| {
            dialog_clone.close();
            save_error_report(&window);
        });
    }

    dialog.present(parent(window));
}

/// Writes an error report and tells the user where it went.
pub fn save_error_report(window: &Rc<MainWindow>) {
    let (game_path, data_source, output_mod_name, direct_mode, mods) = window.report_context();
    let log = window.report_log();
    let ctx = ReportContext {
        headline: "user requested an error report",
        log: &log,
        game_path,
        data_source,
        output_mod_name,
        direct_mode,
        mods,
        extra: vec![
            ("mod library".to_string(), paths::mods_dir().display().to_string()),
            ("logs".to_string(), paths::logs_dir().display().to_string()),
        ],
    };

    match logbook::write_error_report(&paths::logs_dir(), &ctx) {
        Ok(path) => {
            let dialog = alert(
                "Error report saved",
                &format!(
                    "Attach this file to a bug report:\n\n{}\n\nIt contains your mod list, \
                     settings and the log — no account details.",
                    path.display()
                ),
            );
            dismiss_button(&dialog);
            dialog.add_response("copy", "Copy path");
            let window = window.clone();
            let present_window = window.clone();
            let path_clone = path.clone();
            dialog.connect_response(None, move |dialog, response| {
                if response == "copy" {
                    window.window.clipboard().set_text(&path_clone.display().to_string());
                    dialog.close();
                    window.toast("Report path copied");
                }
            });
            dialog.present(parent(&present_window));
        }
        Err(e) => {
            window.toast(&format!("Could not write the report: {e}"));
        }
    }
}

/// Shown when an install finished with failing mods.
pub fn show_install_failures(window: &Rc<MainWindow>, failures: &[String]) {
    let dialog = alert(
        "Some mods could not be applied",
        &format!(
            "{}\n\nThe other mods were installed. Fix or disable the failing ones and \
             install again.",
            failures.join("\n")
        ),
    );
    dialog.add_response("close", "Later");
    dialog.set_close_response("close");
    dialog.add_response("report", "Save report…");
    dialog.set_default_response(Some("report"));
    let window_clone = window.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "report" {
            save_error_report(&window_clone);
        }
    });
    dialog.present(parent(window));
}

/// Shown when an install could not run at all (bad game path, unreadable archives).
pub fn show_install_error(window: &Rc<MainWindow>, message: &str) {
    let dialog = alert(
        "The install could not run",
        &format!(
            "{message}\n\nCheck the game folder and the vanilla data source in Settings."
        ),
    );
    dismiss_button(&dialog);
    dialog.add_response("settings", "Open Settings");
    let window_clone = window.clone();
    dialog.connect_response(None, move |dialog, response| {
        if response == "settings" {
            dialog.close();
            open_settings_dialog(&window_clone);
        }
    });
    dialog.present(parent(window));
}

// ---------------------------------------------------------------------------
// library management
// ---------------------------------------------------------------------------

pub fn import_mod_zip(window: &Rc<MainWindow>) {
    let dialog = gtk::FileDialog::builder()
        .title("Import a mod (.zip)")
        .modal(true)
        .build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Mod archives"));
    filter.add_pattern("*.zip");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));

    let window_clone = window.clone();
    dialog.open(parent(window), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else { return };
        let Some(path) = file.path() else { return };
        match mods::import_zip(&path, &paths::mods_dir()) {
            Ok(id) => {
                window_clone.state.borrow_mut().refresh();
                window_clone.refresh();
                window_clone.toast(&format!("Added {id} to the library"));
            }
            Err(e) => {
                let dialog = alert("Could not import that mod", &e.to_string());
                dismiss_button(&dialog);
                dialog.present(parent(&window_clone));
            }
        }
    });
}

pub fn import_mod_dir(window: &Rc<MainWindow>) {
    let window_clone = window.clone();
    let dialog = gtk::FileDialog::builder()
        .title("Install a mod from a folder")
        .modal(true)
        .build();
    let window_for_dialog = window.window.clone();
    dialog.select_folder(
        Some(&window_for_dialog),
        gio::Cancellable::NONE,
        move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            match mods::import_dir(&path, &paths::mods_dir()) {
                Ok(id) => {
                    window_clone.state.borrow_mut().refresh();
                    window_clone.refresh();
                    window_clone.toast(&format!("Added {id} to the library"));
                }
                Err(e) => {
                    let dialog = alert("That folder is not a mod", &e.to_string());
                    dismiss_button(&dialog);
                    dialog.present(parent(&window_clone));
                }
            }
        },
    );
}

pub fn confirm_remove(window: &Rc<MainWindow>, id: &str, label: &str) {
    let dialog = alert(
        &format!("Remove {label}?"),
        "The mod folder is deleted from your library. Settings and files you added \
         elsewhere are untouched.",
    );
    dialog.add_response("cancel", "Cancel");
    dialog.set_close_response("cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));

    let window = window.clone();
    let present_window = window.clone();
    let id = id.to_string();
    dialog.connect_response(None, move |dialog, response| {
        if response != "remove" {
            return;
        }
        dialog.close();
        match mods::remove(&paths::mods_dir(), &id) {
            Ok(()) => {
                let mut state = window.state.borrow_mut();
                state.settings.enabled.retain(|m| m != &id);
                state.settings.order.retain(|m| m != &id);
                let _ = state.save();
                state.refresh();
                drop(state);
                window.refresh();
                window.toast("Mod removed");
            }
            Err(e) => {
                let dialog = alert("Could not remove that mod", &e.to_string());
                dismiss_button(&dialog);
                dialog.present(parent(&window));
            }
        }
    });
    dialog.present(parent(&present_window));
}

pub fn open_about(window: &Rc<MainWindow>) {
    let about = adw::AboutDialog::builder()
        .application_name("D2rlmm")
        .application_icon("io.github.corngoblin.D2rlmm")
        .developer_name("Ferah")
        .version(d2rlmm_core::VERSION)
        .website("https://github.com/corngoblin/D2rlmm")
        .issue_url("https://github.com/corngoblin/D2rlmm/issues")
        .comments(
            "A mod manager for Diablo II: Resurrected on Linux.\n\n\
             Runs D2RMM mods unmodified: enable them, order them, install, and the \
             merged data is written where the game will load it. Launching the game \
             is left to Steam, Lutris or whatever else already does it well.",
        )
        .build();
    about.present(parent(window));
}

/// Kept so settings changes are always persisted even if a future caller forgets.
#[allow(dead_code)]
pub fn save_settings(settings: &Settings) {
    let _ = settings.save_default();
}
