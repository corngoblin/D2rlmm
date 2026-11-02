//! Persistent user settings, and the mod list's order and enabled flags.
//!
//! D2RMM parks this in the renderer's `localStorage`, which is fine for an Electron
//! app and useless for anything else. A JSON file in the XDG config directory is
//! greppable, diffable, easy to back up, and survives the app being reinstalled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{io_at, Error, Result};

/// Where vanilla game data is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DataSourceKind {
    /// Use CASC if a game folder is set, otherwise extracted data.
    #[default]
    Auto,
    /// Always read the game's own CASC archives.
    Casc,
    /// Read a folder of data files (e.g. one produced by Cairn).
    PreExtracted,
}

impl DataSourceKind {
    pub fn label(self) -> &'static str {
        match self {
            DataSourceKind::Auto => "Automatic",
            DataSourceKind::Casc => "Game archives (CASC)",
            DataSourceKind::PreExtracted => "Extracted data folder",
        }
    }

    pub const ALL: [DataSourceKind; 3] = [
        DataSourceKind::Auto,
        DataSourceKind::Casc,
        DataSourceKind::PreExtracted,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The D2R installation (the folder containing `Data`).
    pub game_path: Option<PathBuf>,
    /// Which vanilla data source to read.
    pub data_source: DataSourceKind,
    /// A folder of already-extracted game data, used when `data_source` allows it.
    pub pre_extracted_path: Option<PathBuf>,
    /// The game's save directory; `None` means "use the best guess".
    pub base_saves_path: Option<PathBuf>,
    /// Override for this mod's save directory, as D2RMM allows.
    pub saves_path: Option<PathBuf>,
    /// Name of the mod folder written into the game (`-mod <name>` uses it).
    pub output_mod_name: String,
    /// Write straight into a data folder instead of building a mod folder.
    ///
    /// This is the mode for a game that runs from extracted ("direct") data with
    /// `-direct -txt` — either the game's own `data` folder or a tree produced by
    /// an extractor such as Cairn or D2R File Extractor.
    pub direct_mode: bool,
    /// Which data folder direct mode writes into. `None` means "work it out":
    /// the game's own `data` folder, or the extracted tree if that is the only
    /// thing configured. See [`Settings::effective_output_data_dir`].
    pub direct_data_path: Option<PathBuf>,
    /// Enabled mods, by folder name.
    pub enabled: Vec<String>,
    /// Install order, by folder name. Mods missing from this list sort last.
    pub order: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            game_path: None,
            data_source: DataSourceKind::Auto,
            pre_extracted_path: None,
            base_saves_path: None,
            saves_path: None,
            output_mod_name: "D2rlmm".to_string(),
            direct_mode: false,
            direct_data_path: None,
            enabled: Vec::new(),
            order: Vec::new(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let settings: Settings = serde_json::from_str(&text).map_err(|source| Error::Json {
                    path: path.to_path_buf(),
                    source,
                })?;
                Ok(settings)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn load_default() -> Result<Self> {
        Self::load(&crate::paths::settings_path())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            crate::paths::ensure_dir(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| Error::msg(format!("could not encode settings: {e}")))?;
        io_at(path, std::fs::write(path, text + "\n"))
    }

    pub fn save_default(&self) -> Result<()> {
        self.save(&crate::paths::settings_path())
    }

    /// The save directory in effect: the explicit override, else the derived
    /// `<base saves>/mods/<output name>` (or `<base saves>` in direct mode).
    pub fn effective_saves_path(&self, base_saves: &Path) -> PathBuf {
        if let Some(explicit) = &self.saves_path {
            return explicit.clone();
        }
        if self.direct_mode {
            base_saves.to_path_buf()
        } else {
            base_saves.join("mods").join(&self.output_mod_name)
        }
    }

    /// The base saves folder in effect: explicit setting, else the first existing
    /// guess, else `~/Saved Games/Diablo II Resurrected`.
    pub fn effective_base_saves(&self) -> PathBuf {
        if let Some(explicit) = &self.base_saves_path {
            return explicit.clone();
        }
        crate::paths::save_dir_candidates()
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("/"))
                    .join("Saved Games/Diablo II Resurrected")
            })
    }

    /// The folder direct mode installs into.
    ///
    /// Resolution order, most explicit first:
    ///
    /// 1. the folder the user picked,
    /// 2. the game's own `data` folder,
    /// 3. the extracted-data tree (so a machine with extracted data and no
    ///    recognisable game folder can still be modded).
    pub fn effective_output_data_dir(&self) -> Option<PathBuf> {
        if let Some(explicit) = &self.direct_data_path {
            return Some(crate::paths::data_dir_of(explicit));
        }
        if let Some(game) = self.game_path.as_deref().filter(|p| p.is_dir()) {
            return Some(crate::paths::data_dir_of(game));
        }
        self.pre_extracted_path
            .as_deref()
            .filter(|p| p.is_dir())
            .map(crate::paths::data_dir_of)
    }

    pub fn is_enabled(&self, mod_id: &str) -> bool {
        self.enabled.iter().any(|id| id == mod_id)
    }

    pub fn set_enabled(&mut self, mod_id: &str, enabled: bool) {
        self.enabled.retain(|id| id != mod_id);
        if enabled {
            self.enabled.push(mod_id.to_string());
            if !self.order.iter().any(|id| id == mod_id) {
                self.order.push(mod_id.to_string());
            }
        }
    }

    /// Puts every known mod into a stable order, keeping the user's arrangement and
    /// appending anything new (a freshly installed mod) at the end.
    pub fn sync_order(&mut self, known: &[String]) {
        self.order.retain(|id| known.iter().any(|k| k == id));
        for id in known {
            if !self.order.iter().any(|k| k == id) {
                self.order.push(id.clone());
            }
        }
        self.enabled.retain(|id| known.iter().any(|k| k == id));
    }

    pub fn move_mod(&mut self, from: usize, to: usize) {
        if from >= self.order.len() || to >= self.order.len() || from == to {
            return;
        }
        let id = self.order.remove(from);
        self.order.insert(to, id);
    }
}
