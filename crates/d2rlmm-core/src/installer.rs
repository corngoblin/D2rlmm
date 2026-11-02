//! The install pipeline: run mods in order, then write the result where the game
//! will find it.
//!
//! Shape of a run, and why:
//!
//! 1. **Every enabled mod runs, in order, against one shared merged tree.** Later
//!    mods see earlier mods' output; that is the entire point of install order.
//! 2. **A mod that fails does not stop the run.** It is recorded as failed and the
//!    rest continue, because the alternative — one broken mod in a twelve-mod list
//!    leaving the user with no install at all — is worse, and upstream behaves the
//!    same way. The report says exactly which mods failed.
//! 3. **Nothing is written until all mods are done.** So a failure leaves the
//!    previous install untouched, and the user can fix the mod and retry.
//! 4. **The flush mirrors D2R's own layout**: `mods/<name>/<name>.mpq/data/…` plus a
//!    `modinfo.json` beside the data folder. Get this wrong and the game silently
//!    loads nothing, so it is spelled out in one place ([`flush`]) rather than
//!    scattered.
//!
//! Direct mode exists for people who would rather patch their game in place. Its
//! price is that uninstalling has to reconstruct vanilla data, which is why the file
//! store tracks whether each file came from the game or from a mod.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::error::{io_at, Error, Result};
use crate::filestore::{FileStore, Warning, WarningKind};
use crate::js::{run_mod, ModSummary, NextStringId, SandboxHost};
use crate::logbook::{InstallLog, LogLine};
use crate::mods::Mod;
use crate::provider::GameData;
use crate::{paths, settings::Settings};

/// What to install and where.
pub struct InstallRequest {
    /// Enabled mods, already in install order.
    pub mods: Vec<Mod>,
    pub game_path: PathBuf,
    pub output_mod_name: String,
    /// Write straight into a data folder instead of building a mod folder.
    ///
    /// This is the mode for a game running from extracted data (`-direct -txt`):
    /// the game's own `data` folder, or a tree produced by Cairn / D2R File
    /// Extractor and pointed at by the user.
    pub direct_mode: bool,
    /// Which data folder direct mode writes into. `None` means "derive it from the
    /// game folder", which is what the UI usually wants.
    pub direct_data_path: Option<PathBuf>,
    /// The game's own save folder (the base for the relative `savepath`).
    pub base_saves_path: PathBuf,
    /// This mod's save folder.
    pub saves_path: PathBuf,
    /// The vanilla data source. Owned by the install run, and moved into the
    /// sandbox, hence the `Option`.
    pub data: Option<GameData>,
    /// Revert instead of install (direct mode only; a mod-folder install is simply
    /// deleted by writing a fresh one).
    pub dry_run: bool,
}

/// Progress for the UI. Deliberately coarse: a per-file callback would flood the
/// main loop for a 150k-file install.
#[derive(Debug, Clone)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct ModOutcome {
    pub id: String,
    pub name: String,
    pub version: String,
    pub ok: bool,
    pub error: Option<String>,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub outcomes: Vec<ModOutcome>,
    pub files_written: usize,
    pub output_dir: PathBuf,
    pub log: InstallLog,
    pub duration: Duration,
    pub dry_run: bool,
    pub direct_mode: bool,
}

impl InstallReport {
    pub fn failures(&self) -> Vec<&ModOutcome> {
        self.outcomes.iter().filter(|o| !o.ok).collect()
    }

    pub fn installed_count(&self) -> usize {
        self.outcomes.iter().filter(|o| o.ok).count()
    }

    pub fn summary(&self) -> String {
        let ok = self.installed_count();
        let failed = self.outcomes.len() - ok;
        if self.dry_run {
            if self.direct_mode {
                format!("Reverted {ok} mod(s) to vanilla data")
            } else {
                "Removed the generated mod folder".to_string()
            }
        } else if failed == 0 {
            format!(
                "Installed {ok} mod(s), {} file(s) written",
                self.files_written
            )
        } else {
            format!(
                "Installed {ok} of {} mod(s) — {failed} failed, see the log",
                self.outcomes.len()
            )
        }
    }
}

/// Runs an install (or a revert) and reports what happened.
pub fn install(mut request: InstallRequest, progress: &mut dyn FnMut(Progress)) -> Result<InstallReport> {
    let started = Instant::now();
    let total = request.mods.len();
    let mut log = InstallLog::new();

    log.info(format!(
        "{} {} mod(s) into {} ({} mode)",
        if request.dry_run { "Reverting" } else { "Installing" },
        total,
        request.game_path.display(),
        if request.direct_mode { "direct" } else { "mod folder" }
    ));
    let data = request
        .data
        .take()
        .ok_or_else(|| Error::msg("no game data source for this install"))?;
    log.info(format!("using {}", data.describe()));

    // Mods in install order, but *not* installed yet — `getModList()` reports
    // progress as the run proceeds.
    let mods_list: Vec<ModSummary> = request
        .mods
        .iter()
        .map(|m| ModSummary {
            id: m.id.clone(),
            name: m.display_name().to_string(),
            version: m.version().to_string(),
            installed: false,
            config: m.config().values().clone(),
        })
        .collect();

    let host = Rc::new(RefCell::new(SandboxHost {
        store: FileStore::new(),
        data,
        mod_id: String::new(),
        mod_dir: PathBuf::new(),
        saves_dir: request.saves_path.clone(),
        mods_list,
        installed: Vec::new(),
        next_string_id: NextStringId::default(),
        log: Vec::new(),
        pending_warnings: Vec::new(),
    }));

    let mut outcomes = Vec::new();

    for (index, module) in request.mods.iter().enumerate() {
        progress(Progress {
            done: index,
            total,
            label: format!("Installing {}", module.display_name()),
        });

        let mod_started = Instant::now();
        let config = module.config();

        // A mod with no runnable code is reported, not fatal.
        let code = match module.code() {
            Ok(code) => code,
            Err(e) => {
                log.push(LogLine::for_mod("error", &module.id, e.to_string()));
                outcomes.push(ModOutcome {
                    id: module.id.clone(),
                    name: module.display_name().to_string(),
                    version: module.version().to_string(),
                    ok: false,
                    error: Some(e.to_string()),
                    duration: mod_started.elapsed(),
                });
                continue;
            }
        };

        {
            let mut state = host.borrow_mut();
            state.mod_id = module.id.clone();
            state.mod_dir = module.dir.clone();
        }

        let config_json = serde_json::to_string(config.values())
            .map_err(|e| Error::msg(format!("could not encode config: {e}")))?;

        // Mods run during a revert too. It looks wasteful, and it is what makes a
        // revert correct: the set of files a mod touched is only knowable by
        // asking it again. Only the flush differs (see `flush`), which is also how
        // upstream does it.
        let result = run_mod(host.clone(), &code, &config_json);

        let mod_log: Vec<LogLine> = {
            let mut state = host.borrow_mut();
            state.log.drain(..).collect()
        };
        log.extend(mod_log);

        match result {
            Ok(()) => {
                {
                    let mut state = host.borrow_mut();
                    state.installed.push(module.id.clone());
                    if let Some(summary) = state.mods_list.iter_mut().find(|m| m.id == module.id) {
                        summary.installed = true;
                    }
                }
                log.push(LogLine::for_mod(
                    "info",
                    &module.id,
                    format!("applied v{}", module.version()),
                ));
                outcomes.push(ModOutcome {
                    id: module.id.clone(),
                    name: module.display_name().to_string(),
                    version: module.version().to_string(),
                    ok: true,
                    error: None,
                    duration: mod_started.elapsed(),
                });
            }
            Err(e) => {
                let message = e.to_string();
                log.push(LogLine::for_mod("error", &module.id, message.clone()));
                outcomes.push(ModOutcome {
                    id: module.id.clone(),
                    name: module.display_name().to_string(),
                    version: module.version().to_string(),
                    ok: false,
                    error: Some(message),
                    duration: mod_started.elapsed(),
                });
            }
        }
    }

    // Warnings the file store produced become log lines: they are the explanation
    // for most "mod A stopped working after I enabled mod B" reports.
    progress(Progress {
        done: total,
        total,
        label: if request.dry_run {
            "Reverting".to_string()
        } else {
            "Writing files".to_string()
        },
    });

    let mut state = Rc::try_unwrap(host)
        .map_err(|_| Error::msg("internal: the sandbox outlived the install"))?
        .into_inner();

    for warning in state.take_warnings() {
        log.push(describe_warning(&warning));
    }

    // A run where every mod failed must not be allowed to delete a working install:
    // the user's next move is to fix the mod and try again, not to rebuild from
    // nothing. (An intentionally empty list still clears the output — that is the
    // documented meaning of installing zero mods.)
    let succeeded = outcomes.iter().filter(|o| o.ok).count();
    if !request.dry_run
        && !request.mods.is_empty()
        && succeeded == 0
        && state.store.modified_count() == 0
    {
        log.warn("no mod produced any files — leaving the previous install untouched");
        let untouched = if request.direct_mode {
            direct_output_dir(&request)
        } else {
            paths::output_mod_dir(&request.game_path, &request.output_mod_name)
        };
        return Ok(InstallReport {
            outcomes,
            files_written: 0,
            output_dir: untouched,
            log,
            duration: started.elapsed(),
            dry_run: request.dry_run,
            direct_mode: request.direct_mode,
        });
    }

    let (files_written, output_dir) = flush(&mut state, &request, &mut log)?;

    log.info(format!(
        "done in {:.1}s — {files_written} file(s) written to {}",
        started.elapsed().as_secs_f64(),
        output_dir.display()
    ));

    Ok(InstallReport {
        outcomes,
        files_written,
        output_dir,
        log,
        duration: started.elapsed(),
        dry_run: request.dry_run,
        direct_mode: request.direct_mode,
    })
}

fn describe_warning(warning: &Warning) -> LogLine {
    match warning.kind {
        WarningKind::Conflict => LogLine::for_mod(
            "warn",
            &warning.mod_id,
            format!(
                "wrote {} without reading it first, and {} already modified it — \
                 that mod's changes to this file may be lost",
                warning.path,
                warning.others.join(", ")
            ),
        ),
        WarningKind::OverwritesGameFile => LogLine::for_mod(
            "warn",
            &warning.mod_id,
            format!(
                "wrote {} without reading it first — any game changes to that file \
                 will be replaced rather than merged",
                warning.path
            ),
        ),
    }
}

/// Writes the merged tree to disk. Returns (files written, output folder).
fn flush(state: &mut SandboxHost, request: &InstallRequest, log: &mut InstallLog) -> Result<(usize, PathBuf)> {
    if request.direct_mode {
        let data_dir = direct_output_dir(request);
        if request.dry_run {
            return revert_direct(state, &data_dir, log);
        }
        paths::ensure_dir(&data_dir)?;
        let written = write_tree(&state.store, &data_dir)?;
        return Ok((written, data_dir));
    }

    let mod_root = paths::output_mod_dir(&request.game_path, &request.output_mod_name);
    let mpq_dir = paths::output_mpq_dir(&request.game_path, &request.output_mod_name);

    if request.dry_run {
        // A mod-folder install is a build product, so removing it *is* reverting.
        if mod_root.exists() {
            std::fs::remove_dir_all(&mod_root).map_err(|e| Error::io(&mod_root, e))?;
            log.info(format!("removed {}", mod_root.display()));
        }
        return Ok((0, mod_root));
    }

    // Start from a clean folder: a mod that stopped writing a file must not leave
    // the previous run's copy behind, or the game keeps loading it.
    if mod_root.exists() {
        std::fs::remove_dir_all(&mod_root).map_err(|e| Error::io(&mod_root, e))?;
    }
    let data_dir = mpq_dir.join("data");
    paths::ensure_dir(&data_dir)?;

    // D2R uses this file to detect game updates; without it, a mod can pin an install
    // to an old data version and the launcher complains.
    if !state.store.exists("global/dataversionbuild.txt") {
        if let Some(bytes) = state.data.read("global/dataversionbuild.txt")? {
            let target = data_dir.join("global/dataversionbuild.txt");
            if let Some(parent) = target.parent() {
                paths::ensure_dir(parent)?;
            }
            io_at(&target, std::fs::write(&target, bytes))?;
        } else {
            log.warn("the game has no global/dataversionbuild.txt to copy");
        }
    }

    let written = write_tree(&state.store, &data_dir)?;

    let savepath = relative_save_path(&request.base_saves_path, &request.saves_path);
    let modinfo = serde_json::json!({
        "name": request.output_mod_name,
        "savepath": savepath,
    });
    let modinfo_path = mpq_dir.join("modinfo.json");
    io_at(
        &modinfo_path,
        std::fs::write(
            &modinfo_path,
            serde_json::to_string_pretty(&modinfo)
                .map_err(|e| Error::msg(format!("encoding modinfo.json: {e}")))?
                + "\n",
        ),
    )?;
    log.info(format!(
        "wrote {} (savepath {savepath})",
        modinfo_path.display()
    ));

    // A build record beside (not inside) the data folder: useful for support, and
    // ignored by the game.
    let manifest = build_manifest(state, request);
    let manifest_path = mod_root.join("d2rlmm-build.json");
    io_at(
        &manifest_path,
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest)
                .map_err(|e| Error::msg(format!("encoding the build manifest: {e}")))?
                + "\n",
        ),
    )?;

    Ok((written, data_dir))
}

fn write_tree(store: &FileStore, data_dir: &Path) -> Result<usize> {
    let mut written = 0;
    for path in store.modified_paths() {
        let target = data_dir.join(&path);
        if let Some(parent) = target.parent() {
            paths::ensure_dir(parent)?;
        }
        match store.data(&path) {
            Some(bytes) => {
                io_at(&target, std::fs::write(&target, bytes))?;
                written += 1;
            }
            None => {
                // A mod deleted this file; make sure it is gone.
                if target.exists() {
                    std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
                }
            }
        }
    }
    Ok(written)
}

/// Where a direct-mode install writes.
///
/// Mirrors [`Settings::effective_output_data_dir`] for callers that have no settings
/// in hand (the CLI, tests). Keeping both in step matters: a revert has to write
/// back into the same folder the install wrote into, or "revert" quietly does
/// nothing.
fn direct_output_dir(request: &InstallRequest) -> PathBuf {
    request
        .direct_data_path
        .clone()
        .unwrap_or_else(|| paths::data_dir_of(&request.game_path))
}

/// Direct-mode revert: restore files that came from the game, delete ones a mod made.
///
/// This reconstructs vanilla from the data source, which is why a revert needs the
/// archives (or the extracted tree) still available. Files a mod created that were
/// never vanilla are simply deleted.
fn revert_direct(
    state: &mut SandboxHost,
    data_dir: &Path,
    log: &mut InstallLog,
) -> Result<(usize, PathBuf)> {
    let data_dir = data_dir.to_path_buf();
    let mut restored = 0;
    let mut removed = 0;
    for path in state.store.modified_paths() {
        let target = data_dir.join(&path);
        if state.store.is_extracted(&path) {
            match state.data.read(&path)? {
                Some(bytes) => {
                    io_at(&target, std::fs::write(&target, bytes))?;
                    restored += 1;
                }
                None => log.warn(format!("cannot restore {path}: not in the game data")),
            }
        } else if target.exists() {
            std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
            removed += 1;
        }
    }
    log.info(format!(
        "reverted {restored} file(s) to vanilla, removed {removed} mod-created file(s)"
    ));
    Ok((restored + removed, data_dir))
}

/// How D2R should spell this mod's save folder in `modinfo.json`.
///
/// Relative when the save folder is under the game's saves (the normal case, and
/// what keeps a mod folder portable between machines with different home paths);
/// absolute when the user pointed somewhere else entirely.
fn relative_save_path(base_saves: &Path, saves: &Path) -> String {
    let mods_dir = base_saves.join("mods");
    if let Ok(rel) = saves.strip_prefix(&mods_dir) {
        if rel.as_os_str().is_empty() {
            return ".".to_string();
        }
        return rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
    }
    if let Ok(rel) = saves.strip_prefix(base_saves) {
        // Not under mods/ — D2R resolves `..`-free relative paths from mods/, so an
        // absolute path is the only correct answer here.
        let _ = rel;
    }
    saves.to_string_lossy().replace('\\', "/")
}

fn build_manifest(state: &SandboxHost, request: &InstallRequest) -> Value {
    let mods: Vec<Value> = request
        .mods
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "name": m.display_name(),
                "version": m.version(),
            })
        })
        .collect();
    let mut out = Map::new();
    out.insert("generator".into(), Value::from("D2rlmm"));
    out.insert("generatorVersion".into(), Value::from(crate::VERSION));
    out.insert("apiVersion".into(), Value::from(format!("{}.{}", crate::D2RMM_API_VERSION.0, crate::D2RMM_API_VERSION.1)));
    out.insert("outputMod".into(), Value::from(request.output_mod_name.clone()));
    out.insert("files".into(), Value::from(state.store.modified_count()));
    out.insert("mods".into(), Value::Array(mods));
    Value::Object(out)
}

/// Builds an [`InstallRequest`] from settings plus a mod list.
pub fn request_from_settings(
    settings: &Settings,
    mods: Vec<Mod>,
    data: GameData,
) -> Result<InstallRequest> {
    let direct_data_path = settings.effective_output_data_dir();
    let game_path = settings.game_path.clone();
    if game_path.is_none() && !(settings.direct_mode && direct_data_path.is_some()) {
        return Err(Error::msg(
            "no game folder is configured — set one in Settings",
        ));
    }
    let base_saves = settings.effective_base_saves();
    let saves = settings.effective_saves_path(&base_saves);
    Ok(InstallRequest {
        mods,
        game_path: game_path.unwrap_or_default(),
        output_mod_name: settings.output_mod_name.clone(),
        direct_mode: settings.direct_mode,
        direct_data_path,
        base_saves_path: base_saves,
        saves_path: saves,
        data: Some(data),
        dry_run: false,
    })
}

/// The enabled mods, in the user's install order.
pub fn enabled_mods(settings: &Settings, mut mods: Vec<Mod>) -> Vec<Mod> {
    let mut ordered: Vec<Mod> = Vec::new();
    for id in &settings.order {
        if let Some(pos) = mods.iter().position(|m| &m.id == id) {
            let m = mods.remove(pos);
            if settings.is_enabled(&m.id) {
                ordered.push(m);
            }
        }
    }
    // Anything enabled but missing from the order (a fresh import) goes last.
    ordered.extend(mods.into_iter().filter(|m| settings.is_enabled(&m.id)));
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fake install: a data folder with the files a mod needs.
    fn game_with_data(root: &Path) {
        let data = root.join("data/global/excel");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("misc.txt"),
            "name\tmaxstack\r\nGold\t100\r\nKey\t50\r\n",
        )
        .unwrap();
        std::fs::write(root.join("data/global/dataversionbuild.txt"), "91541\n").unwrap();
        std::fs::create_dir_all(root.join("data/local/lng")).unwrap();
        std::fs::write(root.join("data/local/lng/next_string_id.txt"), "// 21000\n").unwrap();
        std::fs::write(root.join("Diablo II Resurrected.exe"), "").unwrap();
    }

    fn write_mod(mods_dir: &Path, id: &str, manifest: Value, code: &str) -> Mod {
        let dir = mods_dir.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mod.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        std::fs::write(dir.join("mod.js"), code).unwrap();
        Mod::load(&dir).unwrap()
    }

    #[test]
    fn installs_into_the_mod_folder_layout() {
        let root = temp("d2rlmm-install-layout");
        let game = root.join("game");
        game_with_data(&game);
        let mods_dir = root.join("mods");

        let module = write_mod(
            &mods_dir,
            "StackMod",
            json!({"name": "Stack Mod", "version": "1.0", "config": [
                {"id": "size", "type": "number", "name": "Size", "defaultValue": 500}
            ]}),
            r#"
            const misc = D2RMM.readTsv('global\\excel\\misc.txt', { removeCarriageReturns: true });
            misc.rows.forEach(row => { if (row.name === 'Gold') row.maxstack = String(config.size); });
            D2RMM.writeTsv('global\\excel\\misc.txt', misc, { addCarriageReturns: true });
            "#,
        );

        let request = InstallRequest {
            mods: vec![module],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: false,
            direct_data_path: None,
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: false,
        };

        let mut ticks = 0;
        let report = install(request, &mut |_| ticks += 1).unwrap();
        assert_eq!(report.installed_count(), 1);
        assert_eq!(report.failures().len(), 0);
        assert!(ticks >= 1);

        // The layout the game reads.
        let written = game.join("mods/D2rlmm/D2rlmm.mpq/data/global/excel/misc.txt");
        let text = std::fs::read_to_string(&written).unwrap();
        assert!(text.contains("Gold\t500"), "table not modified: {text:?}");
        assert!(text.contains("\r\n"), "CRLF not restored");
        assert!(game.join("mods/D2rlmm/D2rlmm.mpq/data/global/dataversionbuild.txt").is_file());

        let modinfo = game.join("mods/D2rlmm/D2rlmm.mpq/modinfo.json");
        let info: Value = serde_json::from_str(&std::fs::read_to_string(&modinfo).unwrap()).unwrap();
        assert_eq!(info["name"], "D2rlmm");
        assert_eq!(info["savepath"], "D2rlmm");
        assert!(game.join("mods/D2rlmm/d2rlmm-build.json").is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mods_layer_in_order() {
        let root = temp("d2rlmm-install-order");
        let game = root.join("game");
        game_with_data(&game);
        let mods_dir = root.join("mods");

        let first = write_mod(
            &mods_dir,
            "First",
            json!({"name": "First", "version": "1"}),
            r#"
            const misc = D2RMM.readTsv('global\\excel\\misc.txt', { removeCarriageReturns: true });
            misc.rows[0].maxstack = '999';
            D2RMM.writeTsv('global\\excel\\misc.txt', misc, { addCarriageReturns: true });
            "#,
        );
        let second = write_mod(
            &mods_dir,
            "Second",
            json!({"name": "Second", "version": "1"}),
            r#"
            const misc = D2RMM.readTsv('global\\excel\\misc.txt', { removeCarriageReturns: true });
            misc.rows[0].name = misc.rows[0].name.toUpperCase();
            D2RMM.writeTsv('global\\excel\\misc.txt', misc, { addCarriageReturns: true });
            "#,
        );

        let request = InstallRequest {
            mods: vec![first, second],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: false,
            direct_data_path: None,
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: false,
        };
        install(request, &mut |_| {}).unwrap();

        let text = std::fs::read_to_string(
            game.join("mods/D2rlmm/D2rlmm.mpq/data/global/excel/misc.txt"),
        )
        .unwrap();
        // Both edits present: the second mod saw the first one's output.
        assert!(text.contains("GOLD\t999"), "edits did not compose: {text:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failing_mod_does_not_stop_the_others() {
        let root = temp("d2rlmm-install-fail");
        let game = root.join("game");
        game_with_data(&game);
        let mods_dir = root.join("mods");

        let broken = write_mod(
            &mods_dir,
            "Broken",
            json!({"name": "Broken", "version": "1"}),
            "throw new Error('deliberately broken');",
        );
        let good = write_mod(
            &mods_dir,
            "Good",
            json!({"name": "Good", "version": "1"}),
            r#"
            const misc = D2RMM.readTsv('global\\excel\\misc.txt', { removeCarriageReturns: true });
            misc.rows[1].maxstack = '77';
            D2RMM.writeTsv('global\\excel\\misc.txt', misc, { addCarriageReturns: true });
            "#,
        );

        let request = InstallRequest {
            mods: vec![broken, good],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: false,
            direct_data_path: None,
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: false,
        };
        let report = install(request, &mut |_| {}).unwrap();
        assert_eq!(report.failures().len(), 1);
        assert_eq!(report.installed_count(), 1);
        let text = std::fs::read_to_string(
            game.join("mods/D2rlmm/D2rlmm.mpq/data/global/excel/misc.txt"),
        )
        .unwrap();
        assert!(text.contains("Key\t77"), "good mod did not run: {text:?}");
        assert!(
            report.log.render().contains("deliberately broken"),
            "failure not logged"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reinstall_removes_stale_files() {
        let root = temp("d2rlmm-install-stale");
        let game = root.join("game");
        game_with_data(&game);
        let mods_dir = root.join("mods");

        let module = write_mod(
            &mods_dir,
            "TwoFiles",
            json!({"name": "Two Files", "version": "1"}),
            r#"
            D2RMM.writeTxt('global/extra.txt', 'hi');
            D2RMM.writeTxt('global/other.txt', 'there');
            "#,
        );
        let request = InstallRequest {
            mods: vec![module.clone()],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: false,
            direct_data_path: None,
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: false,
        };
        install(request, &mut |_| {}).unwrap();
        assert!(game.join("mods/D2rlmm/D2rlmm.mpq/data/global/extra.txt").is_file());

        // Now install a version of the mod that no longer writes one of the files.
        std::fs::write(
            mods_dir.join("TwoFiles/mod.js"),
            "D2RMM.writeTxt('global/other.txt', 'there');",
        )
        .unwrap();
        let module = Mod::load(&mods_dir.join("TwoFiles")).unwrap();
        let request2 = InstallRequest {
            mods: vec![module],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: false,
            direct_data_path: None,
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: false,
        };
        install(request2, &mut |_| {}).unwrap();
        assert!(
            !game.join("mods/D2rlmm/D2rlmm.mpq/data/global/extra.txt").exists(),
            "stale file from the previous install survived"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_file_brings_assets_along() {
        let root = temp("d2rlmm-install-assets");
        let game = root.join("game");
        game_with_data(&game);
        let mods_dir = root.join("mods");

        let module = write_mod(
            &mods_dir,
            "AssetMod",
            json!({"name": "Asset Mod", "version": "1"}),
            "D2RMM.copyFile('hd', 'hd', true);",
        );
        let asset_dir = mods_dir.join("AssetMod/hd/ui");
        std::fs::create_dir_all(&asset_dir).unwrap();
        std::fs::write(asset_dir.join("panel.json"), "{\"a\":1}").unwrap();

        let request = InstallRequest {
            mods: vec![module],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: false,
            direct_data_path: None,
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: false,
        };
        install(request, &mut |_| {}).unwrap();
        assert!(
            game.join("mods/D2rlmm/D2rlmm.mpq/data/hd/ui/panel.json").is_file(),
            "copied asset missing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_removes_the_mod_folder() {
        let root = temp("d2rlmm-install-dryrun");
        let game = root.join("game");
        game_with_data(&game);
        let mods_dir = root.join("mods");
        let module = write_mod(
            &mods_dir,
            "Anything",
            json!({"name": "Anything", "version": "1"}),
            "D2RMM.writeTxt('global/x.txt', 'x');",
        );

        for dry_run in [false, true] {
            let request = InstallRequest {
                mods: vec![module.clone()],
                game_path: game.clone(),
                output_mod_name: "D2rlmm".into(),
                direct_mode: false,
                direct_data_path: None,
                base_saves_path: root.join("saves"),
                saves_path: root.join("saves/mods/D2rlmm"),
                data: Some(GameData::open_directory(&game).unwrap()),
                dry_run,
            };
            install(request, &mut |_| {}).unwrap();
        }
        assert!(!game.join("mods/D2rlmm").exists(), "mod folder survived the revert");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enabled_mods_follow_the_saved_order() {
        let root = temp("d2rlmm-enabled-order");
        let mods_dir = root.join("mods");
        let a = write_mod(&mods_dir, "A", json!({"name": "A"}), "");
        let b = write_mod(&mods_dir, "B", json!({"name": "B"}), "");
        let c = write_mod(&mods_dir, "C", json!({"name": "C"}), "");

        let mut settings = Settings::default();
        settings.order = vec!["C".into(), "A".into(), "B".into()];
        settings.enabled = vec!["A".into(), "C".into()];

        let ordered = enabled_mods(&settings, vec![a, b, c]);
        let ids: Vec<&str> = ordered.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["C", "A"]);
        let _ = std::fs::remove_dir_all(&root);
    }


    #[test]
    fn direct_mode_writes_into_an_extracted_data_folder() {
        // The workflow this covers: the game runs from extracted data with
        // `-direct -txt`, so there is no mod folder to build — the *data tree* is
        // the target, wherever the user keeps it.
        let root = temp("d2rlmm-direct-extracted");
        let game = root.join("game");
        game_with_data(&game);
        let extracted = root.join("extracted");
        std::fs::create_dir_all(&extracted).unwrap();
        let mods_dir = root.join("mods");

        let module = write_mod(
            &mods_dir,
            "DirectMod",
            json!({"name": "Direct Mod", "version": "1"}),
            r#"
            const misc = D2RMM.readTsv('global\\excel\\misc.txt', { removeCarriageReturns: true });
            misc.rows[0].maxstack = '1234';
            D2RMM.writeTsv('global\\excel\\misc.txt', misc, { addCarriageReturns: true });
            D2RMM.writeTxt('global/modfile.txt', 'created by the mod');
            "#,
        );

        let request = InstallRequest {
            mods: vec![module.clone()],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: true,
            direct_data_path: Some(extracted.clone()),
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: false,
        };
        let report = install(request, &mut |_| {}).unwrap();
        assert_eq!(report.installed_count(), 1);

        let target = extracted.join("global/excel/misc.txt");
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.contains("Gold\t1234"), "not written into the data tree: {text:?}");
        assert!(extracted.join("global/modfile.txt").is_file());
        // No mod folder is built in direct mode.
        assert!(!game.join("mods").exists());

        // Reverting restores vanilla for files that came from the game and deletes
        // the ones the mod invented.
        let revert = InstallRequest {
            mods: vec![module],
            game_path: game.clone(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: true,
            direct_data_path: Some(extracted.clone()),
            base_saves_path: root.join("saves"),
            saves_path: root.join("saves/mods/D2rlmm"),
            data: Some(GameData::open_directory(&game).unwrap()),
            dry_run: true,
        };
        install(revert, &mut |_| {}).unwrap();
        let restored = std::fs::read_to_string(&target).unwrap();
        assert!(restored.contains("Gold\t100"), "vanilla not restored: {restored:?}");
        assert!(!extracted.join("global/modfile.txt").exists(), "mod-created file survived the revert");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_path_is_relative_when_it_can_be() {
        let base = PathBuf::from("/home/x/Saved Games/Diablo II Resurrected");
        assert_eq!(
            relative_save_path(&base, &base.join("mods/D2rlmm")),
            "D2rlmm"
        );
        assert_eq!(
            relative_save_path(&base, &PathBuf::from("/elsewhere/saves")),
            "/elsewhere/saves"
        );
    }
}
