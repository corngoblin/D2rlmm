//! What the last successful install actually contained.
//!
//! The list in the window is a *selection*: toggling a switch changes nothing on disk
//! until Install runs. Without a record of what went in, the UI cannot tell "you
//! changed something" from "nothing to do", so the button looks identical either way
//! and unticking a mod appears to have no effect at all.
//!
//! Upstream D2RMM solves this by keeping the installed list (id + resolved config) and
//! comparing it with what an install would produce now — that comparison is what turns
//! its Install button from outlined into a filled "save" state. This is the same idea,
//! stored on disk so it survives a restart.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{error::Error, mods::Mod, paths};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledMod {
    pub id: String,
    /// The resolved config the mod ran with, not the saved overrides: two different
    /// saved maps that resolve to the same values must compare equal.
    #[serde(default)]
    pub config: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstalledSet {
    #[serde(default)]
    pub mods: Vec<InstalledMod>,
}

impl InstalledSet {
    pub fn path_in(state_dir: &Path) -> PathBuf {
        state_dir.join("installed.json")
    }

    pub fn load_from(state_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path_in(state_dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn load() -> Self {
        Self::load_from(&paths::state_dir())
    }

    pub fn save_to(&self, state_dir: &Path) -> crate::Result<()> {
        paths::ensure_dir(state_dir)?;
        let path = Self::path_in(state_dir);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| Error::msg(format!("could not record the installed set: {e}")))?;
        std::fs::write(&path, text).map_err(|source| Error::Io { path, source })
    }

    pub fn save(&self) -> crate::Result<()> {
        self.save_to(&paths::state_dir())
    }

    /// The set an install would produce right now, in install order.
    pub fn from_mods(mods: &[Mod]) -> Self {
        Self {
            mods: mods
                .iter()
                .map(|m| InstalledMod {
                    id: m.id.clone(),
                    config: m.config().values().clone(),
                })
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.mods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }
}
