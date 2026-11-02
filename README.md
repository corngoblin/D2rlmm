# D2rlmm

**Diablo II: Resurrected — Linux Mod Manager.**

D2rlmm is a native GTK4 / libadwaita mod manager for D2R on Linux. It runs
[D2RMM](https://github.com/olegbl/d2rmm) mods unmodified: same mod format, same
`mod.json` config schema, same install order, same output layout. If a mod works in
D2RMM, the goal is that it works here.

It does not launch the game. D2R on Linux runs under Steam/Proton or Lutris, both of
which already launch things well; D2rlmm shows you the launch options to give them
instead of competing with them.

![D2rlmm icon](data/icons/hicolor/scalable/apps/io.github.corngoblin.D2rlmm.svg)

---

## Install

```sh
flatpak install --user io.github.corngoblin.D2rlmm.flatpak
flatpak run io.github.corngoblin.D2rlmm
```

Requires `org.gnome.Platform` 50 (the bundle declares it as a runtime repo, so
`flatpak` will offer to fetch it).

## Using it

1. **Settings → Game folder**: point at your D2R installation (the folder containing
   `Data`). If you cannot read the game's archives, or you have already extracted your
   install with [Cairn](https://github.com/corngoblin/cairn), set **Vanilla data from →
   Extracted data folder** and point at that instead.
2. **+ → Import mod from .zip** (or *from folder*). Mods live in
   `~/.config/d2rlmm/mods/<ModName>/`, exactly like D2RMM's library.
3. Flip mods on with the switches. **Order matters** — later mods win conflicts, so use
   *Move up* / *Move down* to arrange them.
4. **Configure** opens the mod's own options, generated from its `mod.json`.
5. **Install** builds the set.

### Then launch the game with the right options

The mods do nothing until the game is told to load them:

| Install target | Launch options |
| --- | --- |
| Mod folder (default) | `-mod D2rlmm -txt` |
| Data folder (`-direct` mode) | `-direct -txt` |

Paste them into the game's launch options in Steam (Properties → Launch Options) or
Lutris (Configure → System options → Additional command line arguments). Settings
shows the exact string with a copy button.

### Where the output goes

```
<game>/mods/D2rlmm/
├── D2rlmm.mpq/
│   ├── data/…              ← the merged game data
│   └── modinfo.json        ← tells the game which save folder to use
└── d2rlmm-build.json       ← which mods, which versions, when (ignored by the game)
```

The doubled name is D2R's own layout, not a typo: the game treats a *folder* named
`<name>.mpq` as a data root in `-mod` mode.

In **direct mode** the merged data is written straight into a data folder instead
(`<game>/data`, or whichever folder you pick).

---

## Compatibility

The engine implements the D2RMM mod API at version **1.9** and reports that to mods
(`D2RMM.getVersion()`), because mods gate features on it.

Supported:

- `mod.js` mods, run in an embedded QuickJS engine
- `mod.json` config schema: `checkbox`, `number`, `text`, `select`, `color`, `section`,
  `visible` / `overrideValue` bindings, `defaultValue`
- `D2RMM.readTsv` / `writeTsv` (including `removeCarriageReturns` / `addCarriageReturns`)
- `D2RMM.readJson` / `writeJson` (parsed tolerantly, written as strict JSON)
- `D2RMM.readTxt` / `writeTxt`, `copyFile`
- `D2RMM.getNextStringID`, `getVersion`, `getFullVersion`, `getModList`, `error`
- `D2RMM.readSaveFile` / `writeSaveFile`

Not supported yet:

- **`mod.ts` (TypeScript) mods.** The engine runs `mod.js`; a TypeScript-only mod is
  reported as an error that says so rather than failing mysteriously. Every mod in
  D2RMM's own example repo is `mod.js`.
- `D2RMM.extractSprite` (raises a clear error if a mod calls it)
- Nexus Mods integration (API key, update checks, `nxm://` links) — download mods from
  the site and import the zip
- The D2R save-file editor that ships inside D2RMM

## Verified against

`tests/compat-example-mods.sh` clones D2RMM's own
[example mods](https://github.com/olegbl/d2rmm.mods) and runs five of them unmodified
against a synthesised vanilla data tree, checking the merged output file by file:

| Mod | What it exercises |
| --- | --- |
| Increase Stack Size | TSV read-modify-write, number config, per-row guards |
| Numbered Rune Names | tolerant JSON parse → strict JSON write, localisation |
| Short Potion Names | JSON localisation, config selects |
| Skip Intro Videos | binary asset copy, the read-then-overwrite uninstall trick |
| Socket Punching | sections, nested config, appending rows with non-string values |

Also verified: installing twice produces identical output, and a revert removes the
generated mod folder.

**What has not been verified:** the CASC path against a real game install. Reading
D2R's archives is implemented (CascLib, vendored) but has only been exercised on
synthetic data and on a machine without the game. The extracted-data path is the
verified one — extract with Cairn, point D2rlmm at the result, and the rest of the
pipeline is identical either way. If CASC fails on your install, that is the fallback,
and it is a supported route rather than a workaround.

## Building

```sh
cargo build --release        # binaries: target/release/d2rlmm, d2rlmm-cli
cargo test                   # engine tests
./flatpak/build.sh --bundle  # io.github.corngoblin.D2rlmm.flatpak
```

The flatpak builds offline from crates pinned in `Cargo.lock`; after changing
dependencies, regenerate the sources list:

```sh
python3 flatpak/gen-cargo-sources.py > flatpak/cargo-sources.json
```

Requires GTK 4.22 and libadwaita 1.9 (what `org.gnome.Platform//50` provides).

## Headless use

`d2rlmm-cli` installs without a display — useful on a Steam Deck over SSH, or in a
script that rebuilds a mod set after a patch:

```sh
d2rlmm-cli list
d2rlmm-cli install --game ~/Games/Diablo\ II\ Resurrected \
                   --only IncreaseStackSize,SocketPunching
d2rlmm-cli install --extracted ~/d2r-data --game ~/Games/D2R --data-out ~/d2r-data/data --direct
```

Exit status is non-zero if any mod failed, and a failed run leaves an error report in
`~/.local/state/d2rlmm/logs/`.

## Logs and error reports

Every install is logged. **Main menu → Install log** shows the current run; **Save
report** writes a text file with the log, the mod list, versions, paths, and the data
source in use. That file is what makes a bug report actionable, and it contains no
account details.

Logs live in `~/.local/state/d2rlmm/logs/`.

## License

MIT. D2RMM is MIT, CascLib is MIT (vendored under `vendor/CascLib` with its own
license). Diablo II: Resurrected is a trademark of Blizzard Entertainment; this
project is unofficial and ships no game data.
