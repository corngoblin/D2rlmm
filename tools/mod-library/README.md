# Mod library tools

For the *user's* mod library (`~/Projects/mods`), not for D2rlmm itself.

- `audit_mods.py <mod dir> [...]` — for each mod: manifest fields, every `D2RMM.copyFile`
  source checked against the filesystem (missing sources are the usual reason a ported mod
  silently does nothing), duplicate destinations, and counts of the read/readTsv/readTxt/
  readSaveFile round-trips that a naive MPQ converter emits.
- `rewrite_converted.py <mod dir> [...] [--apply]` — rewrites auto-converted MPQ ports into
  minimal asset-replacement mods: one `copyFile` per file, no read/write round-trips, no
  `readSaveFile` on game data, and drops MPQ metadata targets (`modinfo.json`). Writes
  `mod.js.bak` beside the original when `--apply` is used.

Rewriting one of these is safe to verify: install the old and the new version with
`d2rlmm-cli` into two output dirs and diff every file's hash. The seven mods rewritten on
2026-11-03 came out with 307/307 and 1413/1413 identical files (the only difference being the
dropped `modinfo.json`).
