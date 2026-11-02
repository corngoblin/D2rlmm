//! App state: the settings file, the mod library as last scanned, and the last run.
//!
//! Held behind `Rc<RefCell<_>>` because GTK is single-threaded and every widget
//! callback wants it. The rule the widgets follow is *mutate then rebuild*, never
//! "patch the row in place": the list is cheap to rebuild (a few dozen rows) and a
//! rebuild cannot drift from the state the way partial updates do.

use std::path::PathBuf;

use d2rlmm_core::installer::InstallReport;
use d2rlmm_core::mods::{self, Mod};
use d2rlmm_core::settings::Settings;

pub struct AppState {
    pub settings: Settings,
    pub mods: Vec<Mod>,
    /// Folders in the library that are not loadable mods, with the reason.
    pub load_errors: Vec<String>,
    /// The most recent install run, kept so the log can be reopened.
    pub last_report: Option<InstallReport>,
    pub installing: bool,
}

impl AppState {
    /// Loads settings and scans the library.
    pub fn load() -> Self {
        let settings = Settings::load_default().unwrap_or_default();
        let mut state = Self {
            settings,
            mods: Vec::new(),
            load_errors: Vec::new(),
            last_report: None,
            installing: false,
        };
        state.refresh();
        state
    }

    /// Rescans the mod library and reconciles the saved order with what exists.
    pub fn refresh(&mut self) {
        let dir = d2rlmm_core::paths::mods_dir();
        let (mods, errors) = mods::discover(&dir);
        self.load_errors = errors.iter().map(|e| e.to_string()).collect();
        self.mods = mods;

        let known: Vec<String> = self.mods.iter().map(|m| m.id.clone()).collect();
        self.settings.sync_order(&known);
    }

    pub fn save(&self) -> d2rlmm_core::Result<()> {
        self.settings.save_default()
    }

    /// Mods in install order, as saved by the user.
    pub fn ordered(&self) -> Vec<&Mod> {
        let mut out: Vec<&Mod> = Vec::new();
        for id in &self.settings.order {
            if let Some(module) = self.mods.iter().find(|m| &m.id == id) {
                out.push(module);
            }
        }
        // Anything the order file has not caught up with yet still shows up.
        for module in &self.mods {
            if !out.iter().any(|m| m.id == module.id) {
                out.push(module);
            }
        }
        out
    }

    pub fn find(&self, id: &str) -> Option<&Mod> {
        self.mods.iter().find(|m| m.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Mod> {
        self.mods.iter_mut().find(|m| m.id == id)
    }

    pub fn enabled_count(&self) -> usize {
        self.mods
            .iter()
            .filter(|m| self.settings.is_enabled(&m.id))
            .count()
    }

    /// Moves a mod one slot up or down in the install order.
    pub fn move_mod(&mut self, id: &str, delta: isize) -> bool {
        let order = &self.settings.order;
        let Some(index) = order.iter().position(|m| m == id) else {
            return false;
        };
        let target = index as isize + delta;
        if target < 0 || target >= order.len() as isize {
            return false;
        }
        self.settings.move_mod(index, target as usize);
        true
    }

    /// The game folder currently in effect, if the user has set one.
    pub fn game_path(&self) -> Option<PathBuf> {
        self.settings.game_path.clone().filter(|p| p.is_dir())
    }

    /// A one-line description of where vanilla data will come from, for the header
    /// and the error report.
    pub fn data_source_summary(&self) -> String {
        match d2rlmm_core::paths::resolve_data_source(
            self.settings.game_path.as_deref(),
            self.settings.data_source,
            self.settings.pre_extracted_path.as_deref(),
        ) {
            Ok(d2rlmm_core::paths::ResolvedSource::Casc(path)) => {
                format!("Game archives — {}", path.display())
            }
            Ok(d2rlmm_core::paths::ResolvedSource::PreExtracted(path)) => {
                format!("Extracted data — {}", path.display())
            }
            Err(e) => e.to_string(),
        }
    }

    /// The arguments the user must give the game, which is the one thing a Linux
    /// user cannot guess.
    pub fn launch_arguments(&self) -> String {
        format!("-mod {} -txt", self.settings.output_mod_name)
    }
}
