//! Reading vanilla game data, from CASC or from a folder of extracted files.
//!
//! Mods think in paths relative to the game's `data` directory — `global/excel/misc.txt`,
//! `local/lng/strings/item-runes.json` — and use backslashes on Windows, because
//! that is what the upstream API examples do. Everything in here normalises those
//! to `/` before touching a filesystem or an archive.
//!
//! Two backends, one interface, because they fail in different ways. CASC is the
//! authoritative source and can never be stale, but it depends on CascLib
//! understanding the current game build. An extracted folder is a plain directory,
//! so it always works, but it silently goes out of date when the game patches. The
//! app prefers CASC and offers the folder as the escape hatch — see
//! [`crate::settings::DataSourceKind::Auto`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Normalises a mod-facing path: forward slashes, no leading slash, no `..`.
pub fn normalize_rel(path: &str) -> Option<String> {
    let unified = path.replace('\\', "/");
    let trimmed = unified.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    for part in trimmed.split('/') {
        if part == ".." {
            return None;
        }
    }
    Some(trimmed.to_string())
}

/// Where vanilla data comes from.
pub enum GameData {
    /// The game's own archives, read through CascLib.
    Casc {
        storage: Box<casc_sys::Storage>,
        path: PathBuf,
    },
    /// A folder of extracted data (a Cairn output, or the game folder itself).
    Directory { root: PathBuf },
}

impl GameData {
    pub fn open_casc(game_path: &Path) -> Result<Self> {
        let storage = casc_sys::Storage::open(game_path).map_err(|e| {
            Error::GameData(format!(
                "could not open the game's archives at {}: {e}",
                game_path.display()
            ))
        })?;
        Ok(GameData::Casc {
            storage: Box::new(storage),
            path: game_path.to_path_buf(),
        })
    }

    pub fn open_directory(root: &Path) -> Result<Self> {
        if !root.is_dir() {
            return Err(Error::GameData(format!(
                "extracted-data folder does not exist: {}",
                root.display()
            )));
        }
        Ok(GameData::Directory {
            root: root.to_path_buf(),
        })
    }

    /// A short description for logs and the error report.
    pub fn describe(&self) -> String {
        match self {
            GameData::Casc { path, .. } => format!("CASC: {}", path.display()),
            GameData::Directory { root } => format!("extracted data: {}", root.display()),
        }
    }

    /// Reads a file by its mod-facing path. `Ok(None)` means the game has no such
    /// file — a normal outcome for optional data.
    pub fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let Some(rel) = normalize_rel(rel) else {
            return Err(Error::GameData(format!("invalid game data path: {rel}")));
        };
        match self {
            GameData::Casc { storage, .. } => {
                // D2R's archive names live under a `data` namespace, and D2RMM paths
                // are already relative to `data`, so the prefix is added once here.
                let candidates = [
                    format!("data:data/{rel}"),
                    format!("data:data/{}", rel.to_lowercase()),
                ];
                let mut last_error = None;
                for name in &candidates {
                    match storage.read_file(name) {
                        Ok(Some(bytes)) => return Ok(Some(bytes)),
                        Ok(None) => {}
                        Err(e) => last_error = Some(e),
                    }
                }
                if let Some(e) = last_error {
                    return Err(Error::GameData(format!("reading {rel}: {e}")));
                }
                Ok(None)
            }
            GameData::Directory { root } => {
                // Accept both "pointed at the game folder" (needs `data/`) and
                // "pointed at the data folder itself".
                for candidate in [root.join(&rel), root.join("data").join(&rel)] {
                    if candidate.is_file() {
                        return std::fs::read(&candidate)
                            .map(Some)
                            .map_err(|e| Error::io(&candidate, e));
                    }
                }
                Ok(None)
            }
        }
    }

    /// Whether the game has this file. Cheaper than reading it.
    pub fn exists(&self, rel: &str) -> bool {
        self.read(rel).map(|b| b.is_some()).unwrap_or(false)
    }
}

/// What the UI shows about the data source in use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSummary {
    pub kind: String,
    pub path: PathBuf,
    pub product: Option<String>,
}

impl GameData {
    pub fn summary(&self) -> SourceSummary {
        match self {
            GameData::Casc { storage, path } => SourceSummary {
                kind: "CASC".into(),
                path: path.clone(),
                product: storage.product(),
            },
            GameData::Directory { root } => SourceSummary {
                kind: "Extracted data".into(),
                path: root.clone(),
                product: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_paths() {
        assert_eq!(
            normalize_rel("global\\excel\\misc.txt"),
            Some("global/excel/misc.txt".into())
        );
        assert_eq!(normalize_rel("/global/x"), Some("global/x".into()));
    }

    #[test]
    fn refuses_traversal() {
        assert_eq!(normalize_rel("../../etc/passwd"), None);
        assert_eq!(normalize_rel("global/../secret"), None);
    }

    #[test]
    fn reads_from_a_directory() {
        let dir = std::env::temp_dir().join("d2rlmm-provider-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("data/global/excel")).unwrap();
        std::fs::write(dir.join("data/global/excel/misc.txt"), b"name\tx\n").unwrap();

        let data = GameData::open_directory(&dir).unwrap();
        // Both "game folder" and "data folder" pointing conventions work.
        assert!(data.exists("global/excel/misc.txt"));
        assert_eq!(
            data.read("global/excel/misc.txt").unwrap().unwrap(),
            b"name\tx\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
