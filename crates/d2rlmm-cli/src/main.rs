//! A small command-line front end.
//!
//! It exists for two reasons. The honest one: installing mods headlessly is useful —
//! a Steam Deck user over SSH, or a script that rebuilds a mod set after a patch.
//! The practical one: it is how the install engine gets tested end to end without a
//! display, against real mod folders and a real game data tree.
//!
//! Argument parsing is hand-rolled. One binary, one subcommand shape, six flags: a
//! whole parser crate would be more dependency than program.

use std::path::PathBuf;
use std::process::ExitCode;

use d2rlmm_core::installer::{self, InstallRequest};
use d2rlmm_core::mods::{self, Mod};
use d2rlmm_core::provider::GameData;
use d2rlmm_core::settings::{DataSourceKind, Settings};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") | Some("--version") | Some("-V") => {
            println!(
                "D2rlmm {} (D2RMM API {}.{}.{})",
                d2rlmm_core::VERSION,
                d2rlmm_core::D2RMM_API_VERSION.0,
                d2rlmm_core::D2RMM_API_VERSION.1,
                d2rlmm_core::D2RMM_API_VERSION.2
            );
            ExitCode::SUCCESS
        }
        Some("list") => list(&args[1..]),
        Some("install") => install(&args[1..], false),
        Some("uninstall") => install(&args[1..], true),
        Some("help") | Some("--help") | Some("-h") | None => {
            usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!(
        "\
D2rlmm — Diablo II: Resurrected Linux Mod Manager (headless)

  d2rlmm-cli list    [--mods DIR]
  d2rlmm-cli install --game DIR [options]
  d2rlmm-cli uninstall --game DIR [options]
  d2rlmm-cli version

install options:
  --game DIR            the D2R installation (the folder containing Data)
  --mods DIR            the mod library (default: ~/.config/d2rlmm/mods)
  --extracted DIR       read vanilla data from an extracted folder (Cairn output);
                        use instead of --game on a machine with no readable archives
  --output-name NAME    name of the mod folder to write (default: D2rlmm)
  --direct              write into a data folder instead of building a mod folder
  --data-out DIR        the data folder --direct writes into (default: <game>/data,
                        or the --extracted tree). For a -direct -txt game.
  --only ID[,ID...]     install only these mod ids
  --dry-run             revert instead of install (same as `uninstall`)
  --config DIR          where mod settings live (default: XDG config)

uninstall removes the install: it deletes the generated mod folder, or in --direct
mode restores every file the mods changed (including files from mods that are no longer
enabled) and deletes the ones they created. Pass --direct/--data-out exactly as you did
for the install, or it will revert the wrong folder.

Exit status is non-zero if any mod failed, so this is scriptable."
    );
}

fn list(args: &[String]) -> ExitCode {
    let mods_dir = flag(args, "--mods")
        .map(PathBuf::from)
        .unwrap_or_else(d2rlmm_core::paths::mods_dir);

    let (mods, errors) = mods::discover(&mods_dir);
    if mods.is_empty() {
        println!("no mods in {}", mods_dir.display());
    }
    for module in &mods {
        let code = if module.has_code() {
            "mod.js"
        } else if module.dir.join("mod.ts").is_file() {
            "mod.ts (unsupported)"
        } else {
            "NO CODE"
        };
        println!(
            "{:<28} {:<10} {:<18} {}",
            module.id,
            module.version(),
            code,
            module.display_name()
        );
    }
    for error in &errors {
        println!("!! {error}");
    }
    ExitCode::SUCCESS
}

/// Installs, or with `uninstall` set removes what is installed.
///
/// `uninstall` is the same pipeline as a revert: mods are not run for their output,
/// only to learn which files they own, and the result is restored to vanilla.
fn install(args: &[String], uninstall: bool) -> ExitCode {
    let game = flag(args, "--game").map(PathBuf::from);
    let extracted = flag(args, "--extracted").map(PathBuf::from);
    if game.is_none() && extracted.is_none() {
        eprintln!("install needs --game DIR (or --extracted DIR)");
        return ExitCode::FAILURE;
    }
    let mods_dir = flag(args, "--mods")
        .map(PathBuf::from)
        .unwrap_or_else(d2rlmm_core::paths::mods_dir);
    let direct = args.iter().any(|a| a == "--direct");
    let dry_run = uninstall || args.iter().any(|a| a == "--dry-run");
    let output_name = flag(args, "--output-name").unwrap_or_else(|| "D2rlmm".to_string());
    let data_out = flag(args, "--data-out").map(PathBuf::from);

    let (all_mods, errors) = mods::discover(&mods_dir);
    for error in &errors {
        eprintln!("skipping: {error}");
    }

    // Selection: --only, else everything in the library in discovery order.
    let selected: Vec<Mod> = match flag(args, "--only") {
        Some(list) => list
            .split(',')
            .filter_map(|id| {
                all_mods
                    .iter()
                    .find(|m| m.id == id.trim())
                    .cloned()
            })
            .collect(),
        None => all_mods,
    };
    // An empty selection is a real request when uninstalling: "restore vanilla" with
    // no mods in the picture. It is an error only for an install.
    if selected.is_empty() && !dry_run {
        eprintln!("nothing to install");
        return ExitCode::FAILURE;
    }

    let data = match &extracted {
        Some(dir) => match GameData::open_directory(dir) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
        None => match GameData::open_casc(game.as_ref().unwrap()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{e}");
                eprintln!("(if the game's archives cannot be read, pass --extracted DIR)");
                return ExitCode::FAILURE;
            }
        },
    };

    // The folder the game itself lives in; with extracted data only, the extracted
    // tree doubles as the root so relative paths and save defaults stay sensible.
    let game = game.or_else(|| extracted.clone()).unwrap_or_default();

    // Saves: keep them next to the game folder when there is no real profile to use,
    // so a headless run never writes into a user's actual save directory.
    let base_saves = d2rlmm_core::paths::save_dir_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| game.join("saves"));
    let settings = Settings {
        output_mod_name: output_name.clone(),
        direct_mode: direct,
        ..Settings::default()
    };

    let request = InstallRequest {
        mods: selected,
        game_path: game.clone(),
        output_mod_name: output_name.clone(),
        direct_mode: direct,
        direct_data_path: if direct {
            data_out.or_else(|| settings.effective_output_data_dir())
        } else {
            None
        },
        base_saves_path: base_saves.clone(),
        saves_path: settings.effective_saves_path(&base_saves),
        data: Some(data),
        dry_run,
        state_dir: None,
    };

    let report = match installer::install(request, &mut |progress| {
        if progress.done < progress.total {
            eprintln!(
                "  [{}/{}] {}",
                progress.done + 1,
                progress.total,
                progress.label
            );
        }
    }) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("install failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("{}", report.log.render());
    println!();
    println!("{}", report.summary());
    println!("output: {}", report.output_dir.display());

    let failures = report.failures();
    if !failures.is_empty() {
        println!("\nfailed mods:");
        for failure in failures {
            println!(
                "  {} — {}",
                failure.id,
                failure.error.as_deref().unwrap_or("unknown error")
            );
        }
        // The report is what makes a bug report actionable; leave one behind.
        let ctx = d2rlmm_core::logbook::ReportContext {
            headline: "headless install had failing mods",
            log: &report.log,
            game_path: Some(game),
            data_source: report.output_dir.display().to_string(),
            output_mod_name: output_name,
            direct_mode: direct,
            mods: report
                .outcomes
                .iter()
                .map(|o| (o.id.clone(), o.version.clone(), o.ok))
                .collect(),
            extra: Vec::new(),
        };
        match d2rlmm_core::logbook::write_error_report(
            &d2rlmm_core::paths::logs_dir(),
            &ctx,
        ) {
            Ok(path) => println!("error report: {}", path.display()),
            Err(e) => eprintln!("could not write the error report: {e}"),
        }
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == name {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix(&format!("{name}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

#[allow(dead_code)]
fn unused(_: DataSourceKind) {}
