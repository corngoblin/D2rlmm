//! The mod engine behind D2rlmm.
//!
//! D2rlmm is a Linux-native reimplementation of [D2RMM](https://github.com/olegbl/d2rmm),
//! the Diablo II: Resurrected mod manager. The goal is *mod compatibility*: an
//! existing D2RMM mod folder should install here unchanged, because the mods
//! themselves are the interesting artifact and nobody wants to rewrite them.
//!
//! # How a D2RMM install actually works
//!
//! A mod is a folder with a `mod.json` manifest and a `mod.js` program. Installing
//! runs that program inside a JavaScript sandbox with two globals: `config` (the
//! user's saved settings for this mod) and `D2RMM` (the mod API). The program reads
//! vanilla game data through `D2RMM.readTsv` / `readJson`, edits it, and writes it
//! back through `D2RMM.writeTsv` / `writeJson`. Mods run in order, so each one sees
//! the previous mod's output — that is the whole reason install order is meaningful.
//!
//! Nothing touches disk until every mod has run: edits accumulate in [`filestore`],
//! which is flushed in one pass at the end. That is what makes an install atomic
//! enough to be worth the name, and it is why a mod that throws leaves no partial
//! file behind.
//!
//! # Where vanilla data comes from
//!
//! D2R ships its data inside CASC archives, so reading `global/excel/misc.txt`
//! means extracting it from a storage container. [`provider`] does that through
//! CascLib ([`casc_sys`]), or — for people who have already unpacked their install
//! with [Cairn](https://github.com/corngoblin/cairn) — from a plain directory of
//! extracted files. Both paths are first-class because the directory one is the
//! escape hatch when a game build outruns CascLib.
//!
//! # Where the result goes
//!
//! The game looks for mods in `mods/<name>/<name>.mpq/data`. That folder-shaped
//! `.mpq` is not a typo: D2R treats a *directory* with that name as a data root
//! when launched with `-mod <name> -txt`, which is exactly how D2RMM ships its
//! output, so D2rlmm writes the same layout. A `modinfo.json` beside the data
//! folder tells the game which save directory to use.

pub mod error;
pub mod filestore;
pub mod installer;
pub mod install_state;
pub mod jsonc;
pub mod js;
pub mod logbook;
pub mod mods;
pub mod paths;
pub mod provider;
pub mod schema;
pub mod settings;
pub mod tsv;

pub use error::{Error, Result};

/// The D2RMM version whose mod API this engine implements.
///
/// Mods gate features on this number (`if (D2RMM.getVersion() < 1.7) throw …`), so
/// it must report the API level actually implemented, not this program's own
/// version. Bumping it without implementing the corresponding API would let mods
/// take code paths that then fail.
pub const D2RMM_API_VERSION: (u32, u32, u32) = (1, 9, 0);

/// This program's own version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
