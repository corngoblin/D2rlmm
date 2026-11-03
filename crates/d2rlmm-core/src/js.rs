//! The JavaScript sandbox mods run in.
//!
//! D2RMM mods are programs. Reimplementing each mod in Rust is not an option — the
//! point of a mod *manager* is that other people's mods run unmodified — so the
//! engine embeds QuickJS (via `rquickjs`), the same engine upstream uses. Same
//! semantics, same ES2020 baseline, same behaviour for the awkward bits mods rely on
//! (string↔number coercion, sparse arrays, `parseInt`'s quirks).
//!
//! # Shape of the boundary
//!
//! One native function, [`invoke`], called from a small JavaScript prelude that
//! builds the `D2RMM` and `config` globals on top of it. Values cross as JSON in
//! both directions, and errors cross as `{ok:false, error}` envelopes that the
//! prelude turns into real `throw new Error(...)`, so a mod's `try/catch` behaves as
//! it would upstream.
//!
//! That single entry point is deliberate. The alternative — one binding per API
//! method — means dozens of `IntoJs`/`FromJs` conversions to get subtly wrong, and
//! an API surface that is hard to extend. Here the boundary is inspectable: log the
//! call's name and arguments and you can see exactly what a mod asked for.
//!
//! # What runs where
//!
//! The host state ([`SandboxHost`]) is shared by reference-counted cell rather than
//! moved into the engine, because the same install-scoped state (the merged file
//! tree, the string-ID counter, the warnings) has to survive across mods. Each mod
//! gets a *fresh* context but the same host, which is how mods layer edits in order
//! while still being isolated from each other's globals.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rquickjs::{CaughtError, CatchResultExt, Context, Ctx, Function, Runtime};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::filestore::FileStore;
use crate::logbook::LogLine;
use crate::provider::GameData;
use crate::tsv::Tsv;
use crate::{jsonc, D2RMM_API_VERSION};

/// A mod as reported to `D2RMM.getModList()`.
#[derive(Debug, Clone)]
pub struct ModSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub installed: bool,
    pub config: Map<String, Value>,
}

/// `local/lng/next_string_id.txt` bookkeeping, shared by all mods in one install.
///
/// Mods that add new strings have to allocate ids nobody else has used, and they do
/// it by incrementing this file. Upstream caches the raw file text and rewrites the
/// first number in it, quirks included, because two mods incrementing it in sequence
/// must not produce the same id.
#[derive(Debug, Default)]
pub struct NextStringId {
    raw: Option<String>,
    current: i64,
}

impl NextStringId {
    pub fn reset(&mut self) {
        self.raw = None;
        self.current = 0;
    }
}

/// Everything a mod's JS can reach, for the duration of one install.
pub struct SandboxHost {
    pub store: FileStore,
    pub data: GameData,
    pub mod_id: String,
    pub mod_dir: PathBuf,
    pub saves_dir: PathBuf,
    pub mods_list: Vec<ModSummary>,
    pub installed: Vec<String>,
    pub next_string_id: NextStringId,
    pub log: Vec<LogLine>,
    /// Warnings raised by the file store (mod conflicts, blind overwrites). They are
    /// reported at the end of the install rather than mid-run so they read as a
    /// summary instead of interrupting a mod's own output.
    pub pending_warnings: Vec<crate::filestore::Warning>,
}

impl SandboxHost {
    pub fn take_warnings(&mut self) -> Vec<crate::filestore::Warning> {
        std::mem::take(&mut self.pending_warnings)
    }

    fn log_line(&mut self, level: &str, message: impl Into<String>) {
        self.log
            .push(LogLine::for_mod(level, &self.mod_id, message.into()));
    }

    /// Pulls a vanilla file into the merged tree if it is not there yet.
    fn ensure_file(&mut self, path: &str) -> Result<()> {
        if self.store.exists(path) {
            return Ok(());
        }
        match self.data.read(path)? {
            Some(bytes) => {
                let mod_id = self.mod_id.clone();
                self.store.insert_extracted(path, bytes, &mod_id);
                Ok(())
            }
            None => Err(Error::msg(format!(
                "the game has no file \"{path}\"{}",
                match &self.data {
                    GameData::Casc { .. } => " (looked in the game archives)".to_string(),
                    GameData::Directory { root } => {
                        format!(" (looked in {})", root.display())
                    }
                }
            ))),
        }
    }

    fn text_of(&self, path: &str) -> Result<String> {
        let bytes = self
            .store
            .data(path)
            .ok_or_else(|| Error::msg(format!("no data loaded for \"{path}\"")))?;
        // D2R text files are NUL-terminated in places; stop at the first NUL the
        // same way upstream's readCString does, or trailing garbage ends up in the
        // parsed structure.
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let text = String::from_utf8_lossy(&bytes[..end]);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        Ok(text.to_string())
    }

    fn write_bytes(&mut self, path: &str, bytes: Vec<u8>) {
        let is_game_file = self.data.exists(path);
        let mod_id = self.mod_id.clone();
        if let Some(warning) = self.store.write(path, bytes, &mod_id, is_game_file) {
            self.pending_warnings.push(warning);
        }
    }
}

/// The JavaScript prelude: turns [`invoke`] into the `D2RMM` and `config` globals.
const PRELUDE: &str = r#"
(function () {
  'use strict';
  var parse = JSON.parse, stringify = JSON.stringify;
  function call(name, args) {
    var env = parse(__d2r_invoke(name, stringify(args === undefined ? [] : args)));
    if (!env.ok) { throw new Error(env.error); }
    return env.value;
  }
  function fmt(v) {
    if (typeof v === 'string') { return v; }
    if (v instanceof Error) { return v.message; }
    try { return stringify(v); } catch (e) { return String(v); }
  }
  function out(level) {
    return function () {
      var parts = [];
      for (var i = 0; i < arguments.length; i++) { parts.push(fmt(arguments[i])); }
      __d2r_log(level, parts.join(' '));
    };
  }
  globalThis.console = {
    log: out('info'), info: out('info'), debug: out('debug'),
    warn: out('warn'), error: out('error'), trace: out('debug')
  };
  globalThis.D2RMM = {
    getVersion: function () { return call('getVersion'); },
    getFullVersion: function () { return call('getFullVersion'); },
    getModList: function () { return call('getModList'); },
    readTsv: function (p, o) { return call('readTsv', [p, !!(o && o.removeCarriageReturns)]); },
    writeTsv: function (p, d, o) {
      // The whole argument list is serialised once by `call`; stringifying `d` here
      // as well is how you end up handing the engine a JSON string of a JSON string.
      return call('writeTsv', [p, d === undefined ? null : d, !!(o && o.addCarriageReturns)]);
    },
    readJson: function (p) { return call('readJson', [p]); },
    writeJson: function (p, d) {
      return call('writeJson', [p, d === undefined ? null : d]);
    },
    readTxt: function (p) { return call('readTxt', [p]); },
    writeTxt: function (p, d) { return call('writeTxt', [p, String(d)]); },
    copyFile: function (s, d, o) { return call('copyFile', [s, d, !!o]); },
    getNextStringID: function () { return call('getNextStringID'); },
    readSaveFile: function (p) { return call('readSaveFile', [p]); },
    writeSaveFile: function (p, d) { return call('writeSaveFile', [p, d === undefined ? null : d]); },
    getConfigJSON: function () { return stringify(config); },
    error: function (m) { return call('error', [String(m)]); },
    extractSprite: function () {
      throw new Error('D2rlmm does not implement D2RMM.extractSprite yet.');
    }
  };
  globalThis.config = parse(__d2r_config_json);
})();
"#;

/// What a finished run tells us once the context is gone.
enum RunOutcome {
    Ok,
    /// A readable failure (a real `Error`, with a message).
    Message(String),
    /// The script threw `null`/nothing at all. QuickJS does this when it aborts on its
    /// own limits, so the reason has to be reconstructed once the context is released.
    Silent,
}

/// Runs one mod's code against the host state.
pub fn run_mod(host: Rc<RefCell<SandboxHost>>, code: &str, config_json: &str) -> Result<()> {
    let limit_bytes = js_memory_limit();
    let runtime = Runtime::new().map_err(|e| Error::msg(format!("QuickJS: {e}")))?;
    runtime.set_memory_limit(limit_bytes);
    runtime.set_max_stack_size(8 * 1024 * 1024);

    let context = Context::full(&runtime).map_err(|e| Error::msg(format!("QuickJS: {e}")))?;

    let outcome: Result<RunOutcome> = context.with(|ctx| {
        install_globals(&ctx, host.clone(), config_json)?;
        ctx.eval::<(), _>(PRELUDE.as_bytes())
            .catch(&ctx)
            .map_err(|e| Error::msg(format!("sandbox setup failed: {e}")))?;
        // Upstream D2RMM evaluates each mod as
        //   (function(){ const config = ...; <mod code> })()
        // and that wrapper is load-bearing: a top-level `return` is a *parse* error in
        // a global script but perfectly legal inside a function. Several real mods bail
        // out with `if (D2RMM.getVersion() < 1.5) { D2RMM.error(...); return; }`, so
        // without the wrapper they cannot run at all ("return not in a function").
        // The opening brace stays on line 1 so syntax errors still report the mod's own
        // line numbers; `config` is already a global from the prelude, so nothing else
        // needs injecting.
        let wrapped = format!("(function(){{{}\n}})();", code);
        match ctx.eval::<(), _>(wrapped.as_bytes()).catch(&ctx) {
            Ok(()) => Ok(RunOutcome::Ok),
            Err(CaughtError::Value(value)) if value.is_null() || value.is_undefined() => {
                Ok(RunOutcome::Silent)
            }
            Err(caught) => Ok(RunOutcome::Message(describe_failure(caught))),
        }
    });

    // The context borrows the runtime's internals; touching memory_usage() while it is
    // alive panics with "RefCell already borrowed". Release it first, then ask.
    drop(context);
    let result = match outcome {
        Ok(RunOutcome::Ok) => Ok(()),
        Ok(RunOutcome::Message(message)) => Err(Error::msg(message)),
        Ok(RunOutcome::Silent) => {
            let used_mb = runtime.memory_usage().memory_used_size.max(0) / (1024 * 1024);
            Err(Error::msg(format!(
                "the mod stopped without reporting an error — the JavaScript sandbox most likely \
                 hit its {} MB memory limit (heap in use: {} MB). If the mod is legitimately large, \
                 raise the limit with D2RLMM_JS_MEMORY_MB=<megabytes>.",
                limit_bytes / (1024 * 1024),
                used_mb,
            )))
        }
        Err(e) => Err(e),
    };

    drop(runtime);
    result
}

/// Ceiling for one mod's JavaScript heap.
///
/// This exists to bound a runaway mod, never to stop a working one. 512 MB was far
/// too small: a mod that caches every preset it touches and serialises at the end
/// (wayfinding act3 holds hundreds of them) needs several hundred MB on its own, and
/// on a 32 GB desktop a 512 MB cap is arbitrary pain. Override with
/// `D2RLMM_JS_MEMORY_MB=<megabytes>`.
const DEFAULT_JS_MEMORY_MB: usize = 4096;

fn js_memory_limit() -> usize {
    let mb = std::env::var("D2RLMM_JS_MEMORY_MB")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|mb| *mb >= 64)
        .unwrap_or(DEFAULT_JS_MEMORY_MB);
    mb * 1024 * 1024
}

/// Turns a caught QuickJS failure into something a person can act on.
///
/// Only the readable cases land here; a `null`/`undefined` throw is classified as
/// [`RunOutcome::Silent`] instead, because explaining it needs the runtime after the
/// context is released.
fn describe_failure(caught: CaughtError<'_>) -> String {
    match caught {
        CaughtError::Error(e) => e.to_string(),
        CaughtError::Exception(e) => e.to_string(),
        CaughtError::Value(value) => format!("the mod threw a non-Error value: {value:?}"),
    }
}

fn install_globals(ctx: &Ctx<'_>, host: Rc<RefCell<SandboxHost>>, config_json: &str) -> Result<()> {
    let globals = ctx.globals();

    let invoke_host = host.clone();
    let invoke = Function::new(ctx.clone(), move |name: String, args: String| -> String {
        let mut state = invoke_host.borrow_mut();
        envelope(&mut state, &name, &args)
    })
    .map_err(|e| Error::msg(format!("QuickJS: {e}")))?;
    globals
        .set("__d2r_invoke", invoke)
        .map_err(|e| Error::msg(format!("QuickJS: {e}")))?;

    let log_host = host;
    let log_fn = Function::new(ctx.clone(), move |level: String, message: String| {
        let mut state = log_host.borrow_mut();
        state.log_line(&level, message);
    })
    .map_err(|e| Error::msg(format!("QuickJS: {e}")))?;
    globals
        .set("__d2r_log", log_fn)
        .map_err(|e| Error::msg(format!("QuickJS: {e}")))?;

    globals
        .set("__d2r_config_json", config_json.to_string())
        .map_err(|e| Error::msg(format!("QuickJS: {e}")))?;
    Ok(())
}

/// Calls `invoke` and wraps the result or error in a JSON envelope.
fn envelope(host: &mut SandboxHost, name: &str, args_json: &str) -> String {
    let args: Vec<Value> = serde_json::from_str(args_json).unwrap_or_default();
    let result = match invoke(host, name, &args) {
        Ok(value) => serde_json::json!({"ok": true, "value": value}),
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
    };
    result.to_string()
}

fn arg_str(args: &[Value], index: usize, what: &str) -> Result<String> {
    args.get(index)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::msg(format!("{what} must be a string")))
}

fn arg_bool(args: &[Value], index: usize) -> bool {
    args.get(index).map(crate::schema::truthy).unwrap_or(false)
}

/// The whole native API, in one match.
fn invoke(host: &mut SandboxHost, name: &str, args: &[Value]) -> Result<Value> {
    match name {
        "getVersion" => Ok(Value::from(api_version_float())),

        "getFullVersion" => Ok(serde_json::json!([
            D2RMM_API_VERSION.0,
            D2RMM_API_VERSION.1,
            D2RMM_API_VERSION.2
        ])),

        "getModList" => {
            let list: Vec<Value> = host
                .mods_list
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "name": m.name,
                        "version": m.version,
                        "installed": m.installed,
                        "config": Value::Object(m.config.clone()),
                    })
                })
                .collect();
            Ok(Value::Array(list))
        }

        "readTsv" => {
            let path = arg_str(args, 0, "readTsv path")?;
            let remove_cr = arg_bool(args, 1);
            host.ensure_file(&path)?;
            let mod_id = host.mod_id.clone();
            // Recording the read is what stops a later write by the same mod being
            // reported as a blind overwrite: read-modify-write is the normal shape
            // of a D2RMM mod.
            host.store.note_read(&path, &mod_id);
            let text = host.text_of(&path)?;
            let tsv = Tsv::parse(&text, remove_cr);
            serde_json::to_value(tsv).map_err(|e| Error::msg(format!("readTsv: {e}")))
        }

        "writeTsv" => {
            let path = arg_str(args, 0, "writeTsv path")?;
            let data: Value = args
                .get(1)
                .cloned()
                .ok_or_else(|| Error::msg("writeTsv data must be an object"))?;
            let add_cr = arg_bool(args, 2);
            let tsv: Tsv = serde_json::from_value(data)
                .map_err(|e| Error::msg(format!("writeTsv data is not a TSV table: {e}")))?;
            let text = tsv.encode(add_cr);
            host.write_bytes(&path, text.into_bytes());
            Ok(Value::Null)
        }

        "readJson" => {
            let path = arg_str(args, 0, "readJson path")?;
            host.ensure_file(&path)?;
            let mod_id = host.mod_id.clone();
            host.store.note_read(&path, &mod_id);
            let text = host.text_of(&path)?;
            jsonc::parse(&text).map_err(|e| Error::msg(format!("readJson {path}: {e}")))
        }

        "writeJson" => {
            let path = arg_str(args, 0, "writeJson path")?;
            let data = args
                .get(1)
                .cloned()
                .ok_or_else(|| Error::msg("writeJson data is missing"))?;
            let text = jsonc::encode(&data)?;
            host.write_bytes(&path, text.into_bytes());
            Ok(Value::Null)
        }

        "readTxt" => {
            let path = arg_str(args, 0, "readTxt path")?;
            host.ensure_file(&path)?;
            let text = host.text_of(&path)?;
            let mod_id = host.mod_id.clone();
            host.store.note_read(&path, &mod_id);
            Ok(Value::String(text))
        }

        "writeTxt" => {
            let path = arg_str(args, 0, "writeTxt path")?;
            let data = arg_str(args, 1, "writeTxt data")?;
            host.write_bytes(&path, data.into_bytes());
            Ok(Value::Null)
        }

        "copyFile" => {
            let src = arg_str(args, 0, "copyFile source")?;
            let dst = arg_str(args, 1, "copyFile destination")?;
            let overwrite = arg_bool(args, 2);
            copy_file(host, &src, &dst, overwrite)?;
            Ok(Value::Null)
        }

        "getNextStringID" => next_string_id(host),

        "readSaveFile" => {
            let path = arg_str(args, 0, "readSaveFile path")?;
            let target = safe_join(&host.saves_dir, &path)?;
            match std::fs::read(&target) {
                Ok(bytes) => Ok(Value::Array(
                    bytes.into_iter().map(|b| Value::from(b)).collect(),
                )),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
                Err(e) => Err(Error::io(&target, e)),
            }
        }

        "writeSaveFile" => {
            let path = arg_str(args, 0, "writeSaveFile path")?;
            let bytes = save_bytes(args.get(1))?;
            let target = safe_join(&host.saves_dir, &path)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            std::fs::write(&target, &bytes).map_err(|e| Error::io(&target, e))?;
            host.log_line("info", format!("wrote save file {}", target.display()));
            Ok(Value::from(bytes.len()))
        }

        "error" => {
            let message = args
                .first()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Err(Error::msg(message))
        }

        other => Err(Error::msg(format!(
            "D2RMM.{other} is not implemented by D2rlmm"
        ))),
    }
}

fn api_version_float() -> f64 {
    D2RMM_API_VERSION.0 as f64 + (D2RMM_API_VERSION.1 as f64 / 10.0)
}

fn save_bytes(value: Option<&Value>) -> Result<Vec<u8>> {
    let Some(Value::Array(items)) = value else {
        return Err(Error::msg("writeSaveFile data must be an array of bytes"));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let n = item
            .as_u64()
            .ok_or_else(|| Error::msg("writeSaveFile data must be an array of bytes"))?;
        if n > 255 {
            return Err(Error::msg("writeSaveFile byte out of range"));
        }
        out.push(n as u8);
    }
    Ok(out)
}

fn next_string_id(host: &mut SandboxHost) -> Result<Value> {
    const PATH: &str = "local/lng/next_string_id.txt";

    if host.next_string_id.raw.is_none() {
        host.ensure_file(PATH)?;
        let text = host.text_of(PATH)?;
        let digits: String = text
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        host.next_string_id.current = digits.parse().unwrap_or(0);
        host.next_string_id.raw = Some(text);
    }

    let id = host.next_string_id.current;
    host.next_string_id.current += 1;

    if let Some(raw) = host.next_string_id.raw.clone() {
        // Replace the first run of digits, exactly as upstream does — the file has a
        // comment around the number in some builds.
        let start = raw.find(|c: char| c.is_ascii_digit());
        let updated = match start {
            Some(start) => {
                let end = raw[start..]
                    .find(|c: char| !c.is_ascii_digit())
                    .map(|offset| start + offset)
                    .unwrap_or(raw.len());
                format!(
                    "{}{}{}",
                    &raw[..start],
                    host.next_string_id.current,
                    &raw[end..]
                )
            }
            None => raw.clone(),
        };
        host.next_string_id.raw = Some(updated.clone());
        host.write_bytes(PATH, updated.into_bytes());
    }

    let mod_id = host.mod_id.clone();
    host.store.note_read(PATH, &mod_id);
    Ok(Value::from(id))
}

fn copy_file(host: &mut SandboxHost, src: &str, dst: &str, overwrite: bool) -> Result<()> {
    let source = safe_join(&host.mod_dir, src)?;
    if !source.exists() {
        return Err(Error::msg(format!(
            "copyFile: the mod has no \"{src}\""
        )));
    }
    let destination = crate::provider::normalize_rel(dst)
        .ok_or_else(|| Error::msg(format!("copyFile: bad destination \"{dst}\"")))?;

    if source.is_dir() {
        for entry in walkdir::WalkDir::new(&source).min_depth(1) {
            let entry = entry.map_err(|e| Error::msg(format!("copyFile: {e}")))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&source)
                .map_err(|_| Error::msg("copyFile: path escaped its root".to_string()))?;
            let target = format!("{}/{}", destination.trim_end_matches('/'), rel.to_string_lossy());
            let bytes = std::fs::read(entry.path()).map_err(|e| Error::io(entry.path(), e))?;
            write_asset(host, &target, bytes, overwrite)?;
        }
        return Ok(());
    }

    let bytes = std::fs::read(&source).map_err(|e| Error::io(&source, e))?;
    write_asset(host, &destination, bytes, overwrite)
}

fn write_asset(host: &mut SandboxHost, path: &str, bytes: Vec<u8>, overwrite: bool) -> Result<()> {
    if !overwrite && host.store.exists(path) {
        host.log_line(
            "debug",
            format!("copyFile: \"{path}\" already exists, leaving it alone"),
        );
        return Ok(());
    }
    host.write_bytes(path, bytes);
    Ok(())
}

/// Joins a relative path onto a root, refusing to leave the root.
fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let Some(clean) = crate::provider::normalize_rel(relative) else {
        return Err(Error::msg(format!("refusing unsafe path \"{relative}\"")));
    };
    Ok(root.join(clean))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filestore::FileStore;

    fn host_with_game(dir: &Path) -> SandboxHost {
        std::fs::create_dir_all(dir.join("data/global/excel")).unwrap();
        std::fs::create_dir_all(dir.join("data/local/lng")).unwrap();
        std::fs::write(
            dir.join("data/global/excel/misc.txt"),
            b"name\tmaxstack\r\nGold\t100\r\n",
        )
        .unwrap();
        std::fs::write(dir.join("data/local/lng/next_string_id.txt"), b"// 21000\n").unwrap();
        SandboxHost {
            store: FileStore::new(),
            data: GameData::open_directory(dir).unwrap(),
            mod_id: "TestMod".into(),
            mod_dir: dir.to_path_buf(),
            saves_dir: dir.join("saves"),
            mods_list: Vec::new(),
            installed: Vec::new(),
            next_string_id: NextStringId::default(),
            log: Vec::new(),
            pending_warnings: Vec::new(),
        }
    }

    fn run(host: &mut SandboxHost, code: &str, config: &str) -> Result<()> {
        // The host lives inside the Rc for the duration of the run; take it back
        // afterwards so the test can inspect it.
        let cell = Rc::new(RefCell::new(std::mem::take(host)));
        let result = run_mod(cell.clone(), code, config);
        let state = Rc::try_unwrap(cell)
            .map_err(|_| Error::msg("sandbox kept a reference to the host"))?
            .into_inner();
        *host = state;
        result
    }

    impl Default for SandboxHost {
        fn default() -> Self {
            SandboxHost {
                store: FileStore::new(),
                data: GameData::open_directory(&std::env::temp_dir()).unwrap(),
                mod_id: String::new(),
                mod_dir: std::env::temp_dir(),
                saves_dir: std::env::temp_dir(),
                mods_list: Vec::new(),
                installed: Vec::new(),
                next_string_id: NextStringId::default(),
                log: Vec::new(),
                pending_warnings: Vec::new(),
            }
        }
    }

    #[test]
    fn reads_and_writes_a_table() {
        let dir = std::env::temp_dir().join("d2rlmm-js-tsv-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        run(
            &mut host,
            r#"
            const misc = D2RMM.readTsv('global\\excel\\misc.txt', { removeCarriageReturns: true });
            misc.rows[0].maxstack = '500';
            D2RMM.writeTsv('global\\excel\\misc.txt', misc, { addCarriageReturns: true });
            "#,
            "{}",
        )
        .unwrap();
        let written = host.store.data("global/excel/misc.txt").unwrap().to_vec();
        assert_eq!(
            String::from_utf8_lossy(&written),
            "name\tmaxstack\r\nGold\t500\r\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_reaches_the_mod() {
        let dir = std::env::temp_dir().join("d2rlmm-js-config-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        run(
            &mut host,
            r#"
            if (config.stackSize !== 250) throw new Error('config missing: ' + config.stackSize);
            if (config.name !== 'Siv') throw new Error('string config missing');
            "#,
            r#"{"stackSize":250,"name":"Siv"}"#,
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_errors_surface_with_their_message() {
        let dir = std::env::temp_dir().join("d2rlmm-js-error-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        let err = run(
            &mut host,
            "throw new Error('this mod is unhappy');",
            "{}",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("this mod is unhappy"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_of_a_missing_game_file_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join("d2rlmm-js-missing-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        let err = run(
            &mut host,
            "D2RMM.readTsv('global\\excel\\nope.txt');",
            "{}",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("no file"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn string_ids_increment_across_calls() {
        let dir = std::env::temp_dir().join("d2rlmm-js-strings-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        run(
            &mut host,
            r#"
            const a = D2RMM.getNextStringID();
            const b = D2RMM.getNextStringID();
            if (b !== a + 1) throw new Error('ids not sequential: ' + a + ',' + b);
            if (a !== 21000) throw new Error('expected 21000, got ' + a);
            "#,
            "{}",
        )
        .unwrap();
        let text = String::from_utf8_lossy(host.store.data("local/lng/next_string_id.txt").unwrap()).to_string();
        assert!(text.contains("21002"), "counter not advanced: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn console_output_is_captured() {
        let dir = std::env::temp_dir().join("d2rlmm-js-console-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        run(&mut host, "console.log('hello', {a: 1});", "{}").unwrap();
        assert!(
            host.log.iter().any(|l| l.message.contains("hello")),
            "log: {:?}",
            host.log
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_claims_the_upstream_api_level() {
        let dir = std::env::temp_dir().join("d2rlmm-js-version-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        run(
            &mut host,
            r#"
            if (D2RMM.getVersion() < 1.7) throw new Error('API too old');
            const [maj, min] = D2RMM.getFullVersion();
            if (maj !== 1 || min !== 9) throw new Error('bad full version');
            "#,
            "{}",
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unimplemented_api_fails_loudly() {
        let dir = std::env::temp_dir().join("d2rlmm-js-unimpl-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut host = host_with_game(&dir);
        let err = run(&mut host, "D2RMM.extractSprite('x');", "{}").unwrap_err();
        assert!(err.to_string().contains("extractSprite"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
