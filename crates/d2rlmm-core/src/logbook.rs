//! Install logs and the error report people attach to bug reports.
//!
//! Two different artifacts, deliberately:
//!
//! * An **install log** per run, appended live, so the UI can show progress and so
//!   there is a record of what happened after the window is closed.
//! * An **error report**, written on demand (or automatically when an install
//!   fails), which bundles the log with the environment: versions, paths, data
//!   source, and the exact mod list with configs.
//!
//! The report exists because the alternative is a bug report that says "it didn't
//! work". Almost every real failure here is explainable from "which game folder,
//! which data source, which mods, in which order" — so that is what the report
//! leads with, before the log lines.
//!
//! Nothing secret goes in: configs are included (they are the user's mod settings,
//! which matter for reproducing a bug) but no credentials or personal file paths
//! beyond the ones the user already chose in the settings.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{io_at, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn parse(raw: &str) -> Level {
        match raw {
            "debug" | "trace" => Level::Debug,
            "warn" | "warning" => Level::Warn,
            "error" => Level::Error,
            _ => Level::Info,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARNING",
            Level::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: Level,
    pub mod_id: Option<String>,
    pub message: String,
}

impl LogLine {
    pub fn new(level: &str, message: impl Into<String>) -> Self {
        Self {
            level: Level::parse(level),
            mod_id: None,
            message: message.into(),
        }
    }

    pub fn for_mod(level: &str, mod_id: &str, message: impl Into<String>) -> Self {
        Self {
            level: Level::parse(level),
            mod_id: Some(mod_id.to_string()),
            message: message.into(),
        }
    }

    /// Renders a line for a text log: `[WARNING] [ModId] message`.
    pub fn render(&self) -> String {
        match &self.mod_id {
            Some(id) => format!("[{}] [{}] {}", self.level.label(), id, self.message),
            None => format!("[{}] {}", self.level.label(), self.message),
        }
    }
}

/// A log of one install run, with the ability to spill to disk.
#[derive(Debug, Default, Clone)]
pub struct InstallLog {
    lines: Vec<LogLine>,
}

impl InstallLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, line: LogLine) {
        // Keep the in-memory tail bounded: a mod that logs thousands of lines should
        // not grow the UI's buffer without limit. The tail is what matters for
        // diagnosing a failure that happened at the end.
        const MAX_LINES: usize = 20_000;
        if self.lines.len() >= MAX_LINES {
            self.lines.remove(0);
        }
        self.lines.push(line);
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.push(LogLine::new("info", message));
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.push(LogLine::new("warn", message));
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.push(LogLine::new("error", message));
    }

    pub fn lines(&self) -> &[LogLine] {
        &self.lines
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = LogLine>) {
        for line in lines {
            self.push(line);
        }
    }

    pub fn error_count(&self) -> usize {
        self.lines.iter().filter(|l| l.level == Level::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.lines.iter().filter(|l| l.level == Level::Warn).count()
    }

    pub fn render(&self) -> String {
        self.lines
            .iter()
            .map(LogLine::render)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Writes the log to `dir`, returning the file path.
    pub fn save(&self, dir: &Path) -> Result<PathBuf> {
        crate::paths::ensure_dir(dir)?;
        let path = dir.join(format!("install-{}.log", timestamp_compact()));
        let mut file = std::fs::File::create(&path).map_err(|e| crate::error::Error::io(&path, e))?;
        for line in &self.lines {
            writeln!(file, "{}", line.render()).map_err(|e| crate::error::Error::io(&path, e))?;
        }
        Ok(path)
    }
}

/// Everything the error report needs to be useful.
pub struct ReportContext<'a> {
    pub headline: &'a str,
    pub log: &'a InstallLog,
    pub game_path: Option<PathBuf>,
    pub data_source: String,
    pub output_mod_name: String,
    pub direct_mode: bool,
    pub mods: Vec<(String, String, bool)>, // id, version, enabled
    pub extra: Vec<(String, String)>,
}

/// Writes a self-contained error report and returns its path.
pub fn write_error_report(dir: &Path, ctx: &ReportContext<'_>) -> Result<PathBuf> {
    crate::paths::ensure_dir(dir)?;
    let path = dir.join(format!("d2rlmm-report-{}.txt", timestamp_compact()));
    let mut out = String::new();

    out.push_str("D2rlmm error report\n");
    out.push_str("===================\n\n");
    out.push_str(&format!("what:      {}\n", ctx.headline));
    out.push_str(&format!("when:      {}\n", timestamp_human()));
    out.push_str(&format!("version:   {}\n", crate::VERSION));
    out.push_str(&format!("api level: D2RMM {}.{}\n", crate::D2RMM_API_VERSION.0, crate::D2RMM_API_VERSION.1));
    out.push_str(&format!("system:    {} {}\n", std::env::consts::OS, std::env::consts::ARCH));
    if let Ok(hostname) = std::env::var("HOSTNAME") {
        out.push_str(&format!("host:      {hostname}\n"));
    }
    out.push('\n');

    out.push_str("Environment\n-----------\n");
    out.push_str(&format!(
        "game folder:  {}\n",
        ctx.game_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(not set)".into())
    ));
    out.push_str(&format!("data source:  {}\n", ctx.data_source));
    out.push_str(&format!("output mod:   {}\n", ctx.output_mod_name));
    out.push_str(&format!("direct mode:  {}\n", if ctx.direct_mode { "yes" } else { "no" }));
    out.push_str(&format!("mod library:  {}\n", crate::paths::mods_dir().display()));
    for (key, value) in &ctx.extra {
        out.push_str(&format!("{key}: {value}\n"));
    }
    out.push('\n');

    out.push_str("Mods (in install order)\n-----------------------\n");
    if ctx.mods.is_empty() {
        out.push_str("(none)\n");
    }
    for (id, version, enabled) in &ctx.mods {
        out.push_str(&format!(
            "{} {id} {version}\n",
            if *enabled { "[x]" } else { "[ ]" }
        ));
    }
    out.push('\n');

    out.push_str("Log\n---\n");
    if ctx.log.lines().is_empty() {
        out.push_str("(no log lines)\n");
    } else {
        out.push_str(&ctx.log.render());
        out.push('\n');
    }

    io_at(&path, std::fs::write(&path, out))?;
    Ok(path)
}

fn timestamp_compact() -> String {
    let (y, mo, d, h, mi, s) = utc_parts(SystemTime::now());
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

fn timestamp_human() -> String {
    let (y, mo, d, h, mi, s) = utc_parts(SystemTime::now());
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// Splits a timestamp into UTC fields without pulling in a date library.
///
/// The civil-from-days algorithm is Howard Hinnant's, which is exact for the whole
/// proleptic Gregorian calendar and short enough to not warrant a dependency for
/// the four places this program prints a time.
fn utc_parts(time: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, minute, second) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_with_level_and_mod() {
        let line = LogLine::for_mod("warn", "MyMod", "something odd");
        assert_eq!(line.render(), "[WARNING] [MyMod] something odd");
    }

    #[test]
    fn counts_errors_and_warnings() {
        let mut log = InstallLog::new();
        log.info("a");
        log.warn("b");
        log.error("c");
        assert_eq!(log.error_count(), 1);
        assert_eq!(log.warning_count(), 1);
    }

    #[test]
    fn timestamp_is_a_plausible_date() {
        let (y, m, d, _, _, _) = utc_parts(UNIX_EPOCH);
        assert_eq!((y, m, d), (1970, 1, 1));
        let (y, m, d, _, _, _) = utc_parts(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        assert_eq!((y, m, d), (2023, 11, 14));
    }

    #[test]
    fn report_contains_the_essentials() {
        let dir = std::env::temp_dir().join("d2rlmm-report-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = InstallLog::new();
        log.info("starting");
        log.error("something broke");
        let ctx = ReportContext {
            headline: "install failed",
            log: &log,
            game_path: Some(PathBuf::from("/games/d2r")),
            data_source: "CASC: /games/d2r".into(),
            output_mod_name: "D2rlmm".into(),
            direct_mode: false,
            mods: vec![("MyMod".into(), "1.0".into(), true)],
            extra: vec![("gtk".into(), "4.22".into())],
        };
        let path = write_error_report(&dir, &ctx).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("install failed"));
        assert!(text.contains("/games/d2r"));
        assert!(text.contains("[x] MyMod 1.0"));
        assert!(text.contains("something broke"));
        assert!(text.contains("gtk: 4.22"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_a_log_file() {
        let dir = std::env::temp_dir().join("d2rlmm-log-save-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = InstallLog::new();
        log.info("hello");
        let path = log.save(&dir).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("hello"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
