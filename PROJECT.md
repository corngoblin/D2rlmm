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

## Verification state

- 50 core unit tests pass.
- `tests/compat-example-mods.sh` runs five real upstream mods against synthesised
  vanilla data and asserts the merged output; also proves idempotence and revert.
- The CASC path is **not** verified against a real game install (no D2R on this
  machine). Extracted-data path is the verified one.

## RESUME HERE — "continue d2rlmm"

State at the pause (2026-11-02, evening):

**Done and verified**
- Core engine: 50 unit tests green (`~/.hermes/cache/scratch/d2rlmm-sdk.sh 'cargo test -p d2rlmm-core'`).
- Real-mod compatibility: `tests/compat-example-mods.sh` runs five unmodified upstream
  mods (Increase Stack Size, Numbered Rune Names, Short Potion Names, Skip Intro
  Videos, Socket Punching) against synthesised vanilla data; asserts merged output,
  idempotence, and revert. All checks pass.
- GTK app compiles in the SDK and in a Fedora 44 container (`d2rlmm-ui`), both
  GTK 4.22.5 / libadwaita 1.9.4.
- Private repo created and remote configured: `github.com/corngoblin/D2rlmm`.
- Packaging files written: manifest, desktop file, metainfo, icon SVG, build.sh,
  cargo-sources.json, README, CHANGELOG, LICENSE, PROJECT.md.

**Immediate blocker (next thing to fix)**
- `./flatpak/build.sh --bundle` compiles, stages and runs `appstreamcli compose`
  successfully, then **fails at export**:
  `error: .../io.github.corngoblin.D2rlmm.svg is not a valid icon: Format not recognized`
  The Rust build and the appstream metadata are fine — only icon validation fails.
  Suspects, in order:
  1. gdk-pixbuf in `org.flatpak.Builder` lacking the SVG loader → test with a
     known-good SVG (e.g. Cairn's icon, which exported successfully) to tell
     "my file is bad" from "the builder cannot read SVG at all".
  2. The file itself: re-check with `rsvg-convert -o /tmp/x.png <svg>` and
     `gdk-pixbuf-thumbnailer`.
  Fix options: correct the SVG, or additionally install PNGs
  (`data/icons/hicolor/{128x128,256x256,512x512}/apps/...png`, rendered with
  `rsvg-convert`, and add the install lines to the manifest) and let export use PNG.
  Do **not** ship a bundle with no icon.

**Then, in order**
1. Re-run `./flatpak/build.sh --bundle` → expect `io.github.corngoblin.D2rlmm.flatpak`.
2. Install it (`./flatpak/build.sh --install` or `flatpak install --user`), then prove
   the *installed* copy: `flatpak run --command=d2rlmm io.github.corngoblin.D2rlmm --version`
   (the `--version` path returns before `app.run()`, so it needs no display), plus
   `flatpak ps --columns=application`.
3. Headless UI verification with the persistent-X11 harness (see below), which is
   **not yet done**. Everything about the UI is unproven so far except that it builds.
4. `git add -A && git commit && git push` (repo exists, remote `origin` set, branch `main`).
   Nothing has been committed or pushed yet.
5. Create release `v0.1.0` with the `.flatpak` asset named
   `io.github.corngoblin.D2rlmm.flatpak` (version-free filename — the tag carries it;
   the user has corrected this before).
6. Report.

**UI harness, ready to use (app binary already built)**
- Container `d2rlmm-ui` (Fedora 44) is up, with Xvfb, xdotool, ImageMagick, and the
  app built at `/srv/d2rlmm/target-fc44/debug/d2rlmm` (host path
  `~/Projects/d2rlmm/target-fc44/debug/d2rlmm`). Run as **root** inside the
  container: rootless podman maps container-root to host uid 1000, so `dev` (uid
  1000 in the container) cannot write to the mount.
- Seeded state lives at `/root/.config/d2rlmm` (6 real mods, 3 enabled,
  `data_source: preextracted`, game + extracted path pointing at
  `/srv/d2rlmm/testdata/compat/game`, output mod name `UIDemo`).
- Suggested persistent session (state survives between `podman exec` calls):
  ```sh
  podman exec -d d2rlmm-ui sh -c 'Xvfb :99 -screen 0 1400x900x24 &
    dbus-daemon --session --address=unix:path=/tmp/bus --fork
    mkdir -p /tmp/xdg && chmod 700 /tmp/xdg'
  podman exec -d d2rlmm-ui env DISPLAY=:99 DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/bus \
    HOME=/root XDG_RUNTIME_DIR=/tmp/xdg GSK_RENDERER=cairo GDK_BACKEND=x11 \
    sh -c 'cd /srv/d2rlmm && ./target-fc44/debug/d2rlmm > /root/app.log 2>&1'
  podman exec d2rlmm-ui env DISPLAY=:99 sh -c 'import -window root /root/01.png'
  podman cp d2rlmm-ui:/root/01.png ~/Projects/d2rlmm/testdata/ui/01.png
  ```
  Then `xdotool` for clicks, `podman cp` files out to look at them, and check
  `/root/app.log` for GTK CRITICALs after every interaction. Verify the *install
  button* end to end: after clicking it, assert
  `/srv/d2rlmm/testdata/compat/game/mods/UIDemo/UIDemo.mpq/data/global/excel/misc.txt`
  exists and contains `Tome of Town Portal<TAB>1<TAB>100<TAB>`.
- Screens to capture for the report: mod list, a mod's configure dialog (schema
  generated), Settings (shows the two install targets), install log.
