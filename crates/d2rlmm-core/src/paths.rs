//! Where things live on a Linux box, and how to find a D2R install on one.
//!
//! Two separate problems: our own files (XDG directories, boring and portable) and
//! the game's files (scattered across Steam libraries, Lutris prefixes and Proton
//! compatdata, which is not portable at all). The second half exists because a mod
//! manager that asks "where is your game?" before it has tried to work it out is
//! annoying — the answers here are guesses, and the UI lets the user overrule them.

use std::path::{Path, PathBuf};

use crate::error::{io_at, Result};
use crate::settings::DataSourceKind;

/// `<config>/d2rlmm` — settings and the mod library.
pub fn config_dir() -> PathBuf {
    base_config_dir().join("d2rlmm")
}

fn base_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")).join(".config")
}

/// `<state>/d2rlmm` — logs, which are machine state rather than config.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("d2rlmm");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/state/d2rlmm")
}

pub fn logs_dir() -> PathBuf {
    state_dir().join("logs")
}

pub fn mods_dir() -> PathBuf {
    config_dir().join("mods")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    io_at(path, std::fs::create_dir_all(path))
}

/// The `mods/<name>/<name>.mpq` folder for a given output mod name.
///
/// The doubled name is D2R's layout, not ours: see the crate docs.
pub fn output_mod_dir(game_path: &Path, output_mod_name: &str) -> PathBuf {
    game_path.join("mods").join(output_mod_name)
}

pub fn output_mpq_dir(game_path: &Path, output_mod_name: &str) -> PathBuf {
    output_mod_dir(game_path, output_mod_name).join(format!("{output_mod_name}.mpq"))
}

/// The data root mods write into (`…/mods/<name>/<name>.mpq/data`), or the game's
/// own `data` folder in direct mode.
pub fn output_data_dir(game_path: &Path, output_mod_name: &str, direct: bool) -> PathBuf {
    if direct {
        game_path.join("data")
    } else {
        output_mpq_dir(game_path, output_mod_name).join("data")
    }
}

/// The `data` folder inside a game or extracted-data root.
///
/// Both conventions are accepted because both are common: an extractor can be told
/// to write into the game folder (leaving `data/` under it) or to unpack straight
/// into a folder that *is* the data tree.
pub fn data_dir_of(root: &Path) -> PathBuf {
    let nested = root.join("data");
    if nested.is_dir() {
        nested
    } else {
        root.to_path_buf()
    }
}

/// The game's own save directory on this machine, best guess first.
///
/// D2R under Proton keeps saves inside the prefix, so "the save folder" is only
/// meaningful relative to a specific prefix. Returned candidates are validated by
/// existence, and the user can always point at one by hand.
pub fn save_dir_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return out;
    };

    out.push(home.join("Saved Games/Diablo II Resurrected"));
    out.push(home.join("Documents/Diablo II Resurrected"));

    // Steam / Proton: compatdata/<appid>/pfx/drive_c/users/<user>/Saved Games/…
    // 2536520 is D2R's Steam app id.
    let steam_roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    for root in steam_roots {
        let compat = root.join("steamapps/compatdata/2536520/pfx/drive_c/users");
        push_user_saved_games(&compat, &mut out);
    }

    // Lutris and plain Wine prefixes.
    if let Ok(entries) = std::fs::read_dir(home.join("Games")) {
        for entry in entries.flatten() {
            push_user_saved_games(&entry.path().join("drive_c/users"), &mut out);
        }
    }
    if let Ok(entries) = std::fs::read_dir(home.join(".wine/drive_c/users")) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("Saved Games/Diablo II Resurrected");
            if candidate.is_dir() {
                out.push(candidate);
            }
        }
    }

    out.dedup();
    out
}

fn push_user_saved_games(users_dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(users_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join("Saved Games/Diablo II Resurrected");
        if candidate.is_dir() {
            out.push(candidate);
        }
    }
}

/// Folders that look like a D2R installation, most likely first.
pub fn game_dir_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(explicit) = std::env::var_os("D2R_PATH") {
        out.push(PathBuf::from(explicit));
    }
    let Some(home) = dirs::home_dir() else {
        return out;
    };

    for root in [
        home.join("Games"),
        home.join(".local/share/Steam/steamapps/common"),
        home.join(".steam/steam/steamapps/common"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common"),
        home.join("SteamLibrary/steamapps/common"),
    ] {
        out.push(root.join("Diablo II Resurrected"));
        out.push(root.join("Diablo II Resurrected/drive_c/Program Files (x86)/Diablo II Resurrected"));
    }

    // Lutris installs land in ~/Games/<slug>/ with the game inside the prefix.
    if let Ok(entries) = std::fs::read_dir(home.join("Games")) {
        for entry in entries.flatten() {
            let base = entry.path();
            out.push(base.join("drive_c/Program Files (x86)/Diablo II Resurrected"));
            out.push(base.join("drive_c/Games/Diablo II Resurrected"));
        }
    }

    out.retain(|p| looks_like_game_dir(p));
    out.dedup();
    out
}

/// Whether a folder could be a D2R install: it has the data folder D2R reads, one
/// way or another.
pub fn looks_like_game_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let has_data = path.join("Data").is_dir() || path.join("data").is_dir();
    if !has_data {
        return false;
    }
    let exe = [
        "Diablo II Resurrected.exe",
        "Diablo II Resurrected Launcher.exe",
        "D2R.exe",
    ]
    .iter()
    .any(|name| path.join(name).is_file());
    // A Steam static install has no exe next to Data if the user pointed at the
    // data root; accept a `.build.info` / `.build.config` instead.
    exe || path.join("Data/.build.config").is_file() || path.join(".build.info").is_file()
}

/// Picks the vanilla data source for an install run.
pub fn resolve_data_source(
    game_path: Option<&Path>,
    kind: DataSourceKind,
    pre_extracted: Option<&Path>,
) -> Result<ResolvedSource> {
    match kind {
        DataSourceKind::PreExtracted => {
            let root = pre_extracted
                .filter(|p| p.is_dir())
                .ok_or_else(|| crate::Error::msg("no extracted-data folder is configured"))?;
            Ok(ResolvedSource::PreExtracted(root.to_path_buf()))
        }
        DataSourceKind::Casc => {
            let game = game_path
                .filter(|p| p.is_dir())
                .ok_or_else(|| crate::Error::msg("no game folder is configured"))?;
            Ok(ResolvedSource::Casc(game.to_path_buf()))
        }
        DataSourceKind::Auto => {
            // Prefer a real installation: CASC is read from the game itself, so it
            // can never be stale, while an extracted copy can (a game patch makes it
            // wrong without telling anyone).
            if let Some(game) = game_path.filter(|p| p.is_dir()) {
                return Ok(ResolvedSource::Casc(game.to_path_buf()));
            }
            if let Some(root) = pre_extracted.filter(|p| p.is_dir()) {
                return Ok(ResolvedSource::PreExtracted(root.to_path_buf()));
            }
            Err(crate::Error::msg(
                "no game folder or extracted-data folder is configured",
            ))
        }
    }
}

/// The outcome of [`resolve_data_source`].
#[derive(Debug, Clone)]
pub enum ResolvedSource {
    Casc(PathBuf),
    PreExtracted(PathBuf),
}
