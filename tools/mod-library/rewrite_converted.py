#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Rewrite auto-converted MPQ->D2RMM mods into clean, minimal asset-replacement mods.

    rewrite_converted.py <mod dir> [...] [--apply]

What the generated version does for every single file:
  1. readJson + writeJson      (reads vanilla, writes it back unchanged)
  2. readTsv  + writeTsv       (throws and is swallowed for non-TSV files)
  3. readTxt  + writeTxt       (for binary assets this truncates at the first NUL!)
  4. readSaveFile/writeSaveFile on a *game data* path — the wrong API entirely
  5. copyFile                  (the only step that does anything)

Steps 1-3 exist because the converter wanted "read before write" to avoid a blind
overwrite warning. One copyFile is all that is needed; the engine records the read
itself. Steps 2-4 are pure exception churn: SpecialNavi throws and catches 5,656
exceptions per install.

Also dropped: targets outside the game's data tree (modinfo.json and friends are MPQ
descriptors, not game content).
"""

import json
import re
import sys
from pathlib import Path

COPY = re.compile(r'D2RMM\.copyFile\(\s*"([^"]+)"\s*,\s*"([^"]+)"', re.S)

HEADER = """// {name}
//
// Asset replacement port of the original MPQ mod. Each entry copies one file from
// this mod's content/ folder into the game's data/ tree. Nothing else is touched, so
// there is nothing to merge and nothing to undo by hand: D2rlmm/D2RMM restore the
// vanilla file when the mod is switched off.
"""


def is_metadata_target(dst: str) -> bool:
    """modinfo.json and other MPQ-level descriptors are not game content."""
    d = dst.replace("\\", "/").lower()
    return d == "modinfo.json" or d.startswith("data/modinfo.json")


def rewrite(mod_dir: Path, apply: bool) -> dict:
    js_path = mod_dir / "mod.js"
    manifest_path = mod_dir / "mod.json"
    code = js_path.read_text(encoding="utf-8")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

    pairs = []
    skipped = []
    seen = set()
    for src, dst in COPY.findall(code):
        if is_metadata_target(dst):
            skipped.append(dst)
            continue
        if dst in seen:
            continue
        seen.add(dst)
        pairs.append((src, dst))

    missing = [src for src, _ in pairs if not (mod_dir / src).exists()]

    body = [HEADER.format(name=manifest.get("name", mod_dir.name))]
    body.append("const FILES = [")
    for src, dst in pairs:
        body.append(f'  ["{src}", "{dst}"],')
    body.append("];\n")
    body.append("for (const [src, dst] of FILES) {")
    body.append("  D2RMM.copyFile(src, dst, true);")
    body.append("}")
    body.append("")
    body.append('console.log(`${FILES.length} file(s) replaced`);')
    new_code = "\n".join(body) + "\n"

    description = manifest.get("description", "")
    if description.startswith("Converted MPQ Mod"):
        manifest["description"] = (
            "Replaces game assets with the files from this port of the original MPQ mod "
            f"({len(pairs)} file{'s' if len(pairs) != 1 else ''})."
        )

    stats = {
        "mod": mod_dir.name,
        "pairs": len(pairs),
        "dropped_metadata": skipped,
        "missing_sources": missing,
        "old_lines": len(code.splitlines()),
        "new_lines": len(new_code.splitlines()),
    }

    if apply and not missing:
        (mod_dir / "mod.js.bak").write_text(code, encoding="utf-8")
        js_path.write_text(new_code, encoding="utf-8")
        manifest_path.write_text(json.dumps(manifest, indent=4, ensure_ascii=False) + "\n",
                                 encoding="utf-8")
    return stats


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    apply = "--apply" in sys.argv
    for raw in args:
        stats = rewrite(Path(raw), apply)
        print(json.dumps(stats, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
