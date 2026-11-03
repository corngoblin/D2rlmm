# PROJECT.md — working notes

Not user documentation (that is README.md); this is the context a future session
needs before touching the code.

## What this is

A Linux-native reimplementation of D2RMM's mod engine plus a GTK4/libadwaita front
end. Written in Rust, shipped as a flatpak, at `github.com/corngoblin/D2rlmm`
(private). Upstream reference source is checked out at `~/Projects/d2rmm`; the
example mods are at `~/Projects/d2rmm.mods`.

Prior art in the same family: `~/Projects/cairn` (extracts D2R's CASC data; this
project reuses its `casc-sys` crate and its flatpak build machinery verbatim).

## Layout

- `crates/d2rlmm-core` — everything that is not a widget:
  - `js.rs` QuickJS sandbox; one native entry point (`invoke`) plus a JS prelude
    that builds the `D2RMM` global. Values cross as JSON envelopes.
  - `installer.rs` run mods in order → flush. Owns the D2R output layout.
  - `filestore.rs` merged in-memory tree + mod-conflict warnings.
  - `schema.rs` `mod.json` config schema, defaults, binding expressions.
  - `provider.rs` vanilla data from CASC (`casc-sys`) or an extracted folder.
  - `mods.rs` library discovery, `mod.json`/`config.json`, zip/folder import.
  - `logbook.rs` install log + the error report format.
  - `tsv.rs` / `jsonc.rs` D2R's TSV and not-quite-JSON formats.
- `crates/d2rlmm-app` — GTK4/libadwaita binary `d2rlmm`. Widgets are built first,
  signals wired after (`MainWindow::wire`) because handlers need `Rc<Self>`.
- `crates/d2rlmm-cli` — `d2rlmm-cli list|install|version`; also the headless test
  harness.
- `vendor/CascLib` — vendored CascLib, built by `casc-sys/build.rs`.

## Build environments

- **Compile/dev:** inside `org.gnome.Sdk//50` (GTK 4.22.5, libadwaita 1.9.4, rust
  1.98) with `--share=network`. Helper script in
  `~/.hermes/cache/scratch/d2rlmm-sdk.sh`. No cargo/rustc on the host.
- **UI verification:** container `d2rlmm-ui` (Fedora 44, same GTK/libadwaita
  versions), project mounted at `/srv`, `CARGO_TARGET_DIR=/srv/d2rlmm/target-fc44`.
  Xvfb + dbus-run-session + `GSK_RENDERER=cairo`; screenshot with ImageMagick
  `import`; drive with `xdotool`. The app must not be run on the host — the host has
  no GTK development environment and no Rust.

## Traps found the hard way

- The `libadwaita` crate's lib name is `libadwaita`, not `adw`: depend on it as
  `adw = { package = "libadwaita", … }` or every `adw::` path fails.
- The prelude must not `JSON.stringify` arguments that `call()` already serialises —
  that produces a JSON string of a JSON string and `writeTsv` rejects it.
- `readTsv`/`readJson` **must** record a read, or every read-modify-write mod gets a
  bogus "blind overwrite" warning.
- A read must suppress *both* warnings in `FileStore::write`, not just the conflict.
- TSV values are coerced on write (`Value` → string, JS style): real mods assign
  numbers (`row.enabled = 1`) and upstream accepts it.
- A revert re-runs the mods (upstream does too) — the touched-file set is only
  knowable by asking the mod again. Only the flush differs.
- If every mod failed and nothing was produced, do not delete the previous output.
- Fixtures: a table column the mod compares against must not be the *last* column,
  or it carries the `\r` and the comparison silently fails while the mod "works".
- **`Rc<MainWindow>` must be kept alive outside `connect_activate`.** Every handler in
  `window.rs` upgrades a `Weak`, so the `Rc` returned by `MainWindow::new()` needs an
  owner that outlives the activate closure — it is stored in a `thread_local`
  (`main.rs::LIVING_WINDOW`). Dropped on return, the window still draws and animates and
  popovers still open (GTK owns those), while *every* app handler becomes a no-op: switches
  flip but never save, buttons do nothing, menu items do nothing, rows do not activate.
  Looks exactly like a frozen app; it is a dead one. Verified the hard way in the container.
- **An app icon's XML comment must not sit before `<svg>`.** librsvg's gdk-pixbuf loader
  sniffs only the first bytes for `<?xml`/`<svg`, so a comment ahead of the root element
  makes flatpak's export validation fail with `is not a valid icon: Format not recognized`
  — while `rsvg-convert` happily renders the same file. Keep the comment inside `<svg>`.
  (Also ship 64/128/256/512 PNGs; they are what small sizes actually use.)
- **`AdwPreferencesGroup::set_description` and row subtitles are Pango markup.** A literal
  `<name>` in user-facing text drops the whole string with `Element "markup" was closed,
  but the currently open element is "name"` in the log and nothing on screen. Escape it as
  `&lt;name&gt;`.
- **Direct mode has no folder to rebuild, so uninstall needs a written record.** In
  mod-folder mode "uninstall" is just "delete and rebuild the folder", which is why
  switching a mod off works. Direct mode writes *into* the game's data tree: the mod that
  wrote a file cannot be asked again once it is switched off (or deleted from the library),
  so nothing would ever discover that file and it stays there overriding vanilla. Every
  direct install therefore records the paths it wrote in
  `<state>/direct-install.json`, the next install restores whatever the current mod set no
  longer produces, and a revert unions the record with the current store before restoring.
- **A restore must create its parent folders.** `io_at` only wraps the io error — it does
  not `mkdir -p`. A revert that restores a vanilla file under a tree the mod replaced dies
  with `No such file or directory (os error 2)` halfway through, leaving a half-reverted
  install; `paths::ensure_dir(parent)` first.
- **A stopped container keeps a stale `/tmp/.X99-lock` and X socket, and Xvfb then refuses
  to start** (`Server is already active for display 99` in `/root/xvfb.log`, no display for
  `xdotool`). Probe the *server* (`DISPLAY=:99 xdotool getdisplaygeometry`), remove lock +
  socket, start again. The session bus has the same trap: a socket file left behind makes
  GTK log `Unable to acquire session bus: Connection refused`, so probe it with
  `dbus-send … org.freedesktop.DBus.GetId` before reusing `/tmp/bus`.

## Verification state

- 54 core unit tests pass.
- `tests/compat-example-mods.sh` runs five real upstream mods against synthesised
  vanilla data and asserts the merged output; also proves idempotence, a mod-folder
  uninstall, and (0.1.1) a direct-mode install followed by `d2rlmm-cli uninstall`, after
  which every file left in the data folder is byte-identical to vanilla.
- **Uninstall (0.1.1)** is verified in the app too: with all switches off the header button
  becomes a red **Uninstall**, clicking it deletes `mods/UIDemo` (screenshot
  `testdata/ui/14-uninstall-button.png`, `15-uninstalled.png`), and switching a single mod
  off then pressing Install rebuilds without it (misc.txt/weapons.txt leave the output,
  `d2rlmm-build.json` lists the remaining three).
- **UI is now driven, not just compiled** (2026-11-02, container `d2rlmm-ui`, GTK
  4.22.5 / libadwaita 1.9.4, debug *and* release profiles): mod list renders, a mod's
  configure dialog is generated from `mod.json` (Increase Stack Size → 4 steppers),
  editing a value saves to the mod's `config.json`, Reset clears it, a switch flips and
  persists to `settings.json` while the Install label goes (3) → (4), Settings shows
  both install targets, menu → Install log shows the run log, and Install writes
  `mods/UIDemo/UIDemo.mpq` (9 files; misc.txt `Tome of Town Portal … 100`, rune names
  numbered, videos replaced) with the toast `Installed 4 mod(s), 9 file(s) written`.
  Screenshots: `testdata/ui/`.
- Flatpak: `./flatpak/build.sh --bundle --install` succeeds end to end (icons export,
  `appstreamcli compose` Success, bundle ~1.6 MB), the installed copy answers
  `flatpak run --command=d2rlmm … --version` → `D2rlmm 0.1.0 (D2RMM API 1.9)`,
  `ldd /app/bin/d2rlmm | grep -c "not found"` → 0, and the packaged binary contains the
  fixed markup string, so the shipped build is the fixed one.
- The CASC path is **not** verified against a real game install (no D2R on this
  machine). Extracted-data path is the verified one.
- **Not proven:** the packaged window photographed on a real desktop. This host is
  headless; driving the *installed sandbox* needed a synthetic X display that flatpak's
  own X11 proxy refused (`Failed to open display`). Pixels therefore come from the
  container harness with the same toolkit versions, packaged behaviour from the checks
  above. Running the installed app on a real session is the one check left for a human.

## State — 2026-11-02 (0.1.1 shipped; RESUME HERE on "continue d2rlmm")

Both releases are published and installed on this machine:

- `v0.1.0` — first release (engine, UI, flatpak, icons). Fixed on the way: dropped
  `Rc<MainWindow>`, icon comment before `<svg>`, unescaped `<name>`.
- `v0.1.1` — uninstall. The Install button becomes a red **Uninstall** with nothing
  enabled; direct mode records what it wrote (`<state>/direct-install.json`) so a mod that
  is off or deleted can still be undone; reverts create parent folders; `d2rlmm-cli
  uninstall`; menu entry "Uninstall mods (restore vanilla)".
- Assets keep the version-free name `io.github.corngoblin.D2rlmm.flatpak`; the tag carries
  the version. Release digests (what was tested = what is published):
  0.1.0 `sha256:ef8898ec…481d4`, 0.1.1 `sha256:2c9f0ed2…e710a`.
- 54 core tests; `tests/compat-example-mods.sh` (five real upstream mods) passes through
  the **installed bundle's own CLI**, including the direct install → uninstall leg.

### Waiting on the user

He is testing 0.1.1 on his own desktop (it is installed there). What he is exercising, and
what to ask about first next time:

1. The **Uninstall** button in the real session (only the container harness saw it so far).
2. Whether unticking a mod and pressing Install removes it for him — and which mode he is
   actually in (mod folder vs `-direct`), since the two have different undo paths.
3. Whether the restored-to-vanilla behaviour in `-direct` mode is what he expects; the
   current choice is *restore the game's copy* rather than deleting, so an unfamiliar data
   folder is never emptied. If he would rather have files deleted when they came from the
   archives, that is a design change, not a bug fix.

### Then, in order

1. CASC path against a real game install — the only data source never exercised. Needs his
   D2R install or a copy of its archives.
2. Anything in the "Known limitations" list (TypeScript mods, `D2RMM.extractSprite`, no
   Nexus integration, no save editor).
3. Only if asked: the packaged window photographed on a real desktop (blocked here — see
   "Not proven" above).

## UI harness (how the screenshots were made)

- Container `d2rlmm-ui` (Fedora 44) with Xvfb, xdotool, ImageMagick, procps,
  librsvg2-tools; project mounted at `/srv`; run as **root** inside (rootless podman
  maps container-root to host uid 1000, so `dev` cannot write to the mount).
- Session script: `~/.hermes/cache/scratch/d2rlmm-session.sh` copies to `/root/session.sh`
  and starts Xvfb `:99` plus a session bus at `unix:path=/tmp/bus`.
- Launch:
  ```sh
  podman exec -d d2rlmm-ui env DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/bus \
    HOME=/root XDG_RUNTIME_DIR=/tmp/xdg-root GSK_RENDERER=cairo GDK_BACKEND=x11 \
    sh -c 'cd /srv/d2rlmm && exec ./target-fc44/debug/d2rlmm > /root/app.log 2>&1'
  ```
- Drive with `xdotool` (`mousemove` + `mousedown 1` + `mouseup 1`, ~0.15 s apart — a
  bare `click 1` was eaten in this no-WM session), capture `import -window root`,
  `podman cp` out, and check `/root/app.log` after every interaction for
  `CRITICAL`/markup warnings. The only log noise expected in the container is the
  portal `Inhibit` warning and the MESA DRI3 warning.
- Coordinates that work at 1400x900 with the seeded state: Configure row 1 (749, 93),
  Install (707, 27), hamburger (631, 27), Settings item (572, 80), row-4 switch
  (68, 279), dialog Save (640, 47) / Reset (195, 47) / close (696–714, 47).

## Known issue found 2026-11-03: heavy mods die with `Null"null"`

Siv installed the (translated) *wayfinding mode* 5.0 mod set — five separate mods, act1–act5 —
and act3 aborted with:

```
[ERROR] [wayfinding mode act3] Exception generated by quickjs: Null"null"
```

Every other mod applied; only act3's writes are missing. Diagnosis:

- That message is `CaughtError::Value` in rquickjs (`Display for CaughtError` prints
  `Exception generated by quickjs: {value:?}`), i.e. **the mod threw a non-Error value — here
  `null`**. It is not a mod-authoring error: every error our sandbox raises is
  `new Error(...)` with text ("the game has no file …", "readJson …: …", "writeTxt data must be
  a string"). A bare null carries no message at all.
- The mod is **not** at fault, and neither was the translation — `mod.js` code is byte-identical
  between the Korean original and the English version once comments are stripped (verified with a
  quote-aware scanner), and `mod.json` differs only in label text.
- The failing call is the 4th `pushRoadEntity` of the Durance of Hate block, i.e. call 175 of 183
  (the log stops at 95.08 %, and 174/183 = 95.08 %). Pure JS: three `readJson` calls and object
  pushes, no boundary operation that can fail on its own.
- Most likely cause: **the sandbox memory cap**. `js.rs` sets
  `set_memory_limit(512 * 1024 * 1024)`. act3 is the heaviest mod by far — it caches *every*
  preset it touches in `jsonFileCache` and only serialises at the very end, while each entity
  batch multiplies through `stringifyPretty`. Measured scaling of this exact mod (synthetic presets,
  Node heap): ~370 KB presets → ~300 MB peak; ~560 KB → ~420 MB; ~670 KB → ~500 MB; ~1.1 MB →
  ~930 MB. So any act whose presets average ≳ 700 KB crosses the cap, and act3 is exactly that act.
  Turning the road layer off drops the same run to ~420 MB, arrows off as well → ~180 MB.
- QuickJS reports its memory-limit abort as a null exception, which is why the report is useless.

### Fix (0.1.2) — implemented, awaiting a build

1. Replace the bare `.map_err(|e| Error::msg(e.to_string()))` around `ctx.eval` with a match on
   `CaughtError`: for `Value(v)` report `is_null()/is_undefined()` as "the mod stopped with no
   value — the QuickJS sandbox most likely hit its memory limit", and append
   `ctx.runtime().memory_usage()` plus the configured cap. Never print `Null"null"` again.
2. Raise the cap and make it overridable: default 2 GB, `D2RLMM_JS_MEMORY_MB` to change it.
   Upstream D2RMM has no cap at all; ours should bound runaway mods, not working ones.
3. Optional: log the cap and peak usage per mod at `debug` level so a future failure is one grep away.

Workaround for the user right now: in Act 3's options turn off *Show road markers* (predicted to
land ~420 MB, well under the cap), or leave act3 disabled — acts 1, 2, 4 and 5 installed fine.

### Verified 2026-11-03 (0.1.2 fix)

End-to-end on the real engine, `d2rlmm-cli` built from this tree, act3's unchanged mod.js,
a synthetic extracted-data tree sized like real presets (378 preset files, ~1.5 MB each):

| sandbox cap | result |
|---|---|
| `D2RLMM_JS_MEMORY_MB=512` (old) | fails: "the mod stopped without reporting an error — the JavaScript sandbox most likely hit its 512 MB memory limit (heap in use: 380 MB)" — no more `Null"null"` |
| default (4096) | succeeds: 183 steps, **378 files written**, 147 s, output 279 MB |

The 378 files match what the same mod writes under a Node stub, and "Total: 183" matches the
step count derived from Siv's 0.1.1 log (his run died at 95.08 % = step 174 of 183). So the cap
was the cause, and raising it is the fix.

Trap found while doing this: `runtime.memory_usage()` **panics with "RefCell already borrowed"**
if the `Context` is still alive. Classify the caught value inside `context.with`, drop the
context, *then* query memory and build the message. The first version of this patch crashed
instead of reporting, which is how the trap surfaced.
