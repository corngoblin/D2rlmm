# Changelog

## 0.1.0 — 2026-11-02

First release.

### Mod engine

- Runs unmodified D2RMM `mod.js` mods in an embedded QuickJS sandbox, with the
  `D2RMM` API at upstream version 1.9.
- Full `mod.json` config schema: `checkbox`, `number`, `text`, `select`, `color`,
  `section`, plus `visible` / `overrideValue` / `defaultValue` bindings.
- TSV and JSON game data read-modify-write, with the same carriage-return handling
  as upstream and JavaScript-style value coercion on write.
- Installs claim nothing until every mod has run: a failure leaves the previous
  install untouched, and a run where every mod fails does not delete it either.
- Mod-conflict warnings when one mod overwrites another's file without reading it.
- Install order is the merge order; reordering rebuilds the set.

### Install targets

- **Mod folder** (default): `mods/<Name>/<Name>.mpq/data` plus `modinfo.json`, the
  layout `-mod <Name> -txt` loads.
- **Data folder** (`-direct`): writes into the game's `data` folder, or any data
  folder you choose — including a tree extracted by Cairn or D2R File Extractor.
- Vanilla data can come from the game's own CASC archives or from an extracted data
  folder, so the app works on a machine whose archives cannot be read.

### Interface

- GTK4 / libadwaita, GNOME 50 runtime.
- Mod list with enable switches, ordering, per-mod configure dialogs generated from
  each mod's schema, and install progress.
- Mod import from `.zip` (either archive layout) or from a folder.
- Install log viewer, and a **Save error report** action that writes the log plus
  environment, mod list and versions to a single file.
- No game-launch feature by design; the correct launch options are shown with a copy
  button, and shown differently for each install target.

### Command line

- `d2rlmm-cli list | install | version` for headless installs and scripting.

### Known limitations

- TypeScript mods (`mod.ts`) are not run; they are reported clearly.
- `D2RMM.extractSprite` is not implemented.
- No Nexus Mods integration and no save-file editor.
- The CASC reader is implemented but not yet exercised against a real game install;
  the extracted-data path is the verified one.
