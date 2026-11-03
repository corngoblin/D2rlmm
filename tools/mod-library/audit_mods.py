#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Audit auto-converted D2RMM mods: copyFile sources vs reality, and API misuse."""
import json
import re
import sys
from pathlib import Path

COPY = re.compile(r'D2RMM\.copyFile\(\s*"([^"]+)"\s*,\s*"([^"]+)"', re.S)
READS = {
    "readJson": re.compile(r'D2RMM\.readJson\(\s*"([^"]+)"'),
    "readTsv": re.compile(r'D2RMM\.readTsv\(\s*"([^"]+)"'),
    "readTxt": re.compile(r'D2RMM\.readTxt\(\s*"([^"]+)"'),
    "readSaveFile": re.compile(r'D2RMM\.readSaveFile\(\s*"([^"]+)"'),
}
BINARY_EXT = {".sprite", ".dc6", ".dcc", ".cof", ".mpq", ".bin", ".wav", ".ogg", ".bik", ".ttf"}

for mod_dir in sys.argv[1:]:
    root = Path(mod_dir)
    js = root / "mod.js"
    print(f"===== {root.name}")
    if not js.exists():
        print("  no mod.js")
        continue
    code = js.read_text(encoding="utf-8", errors="replace")
    manifest = json.loads((root / "mod.json").read_text(encoding="utf-8"))
    print(f"  manifest: name={manifest.get('name')!r} version={manifest.get('version')!r} "
          f"author={manifest.get('author')!r} config entries={len(manifest.get('config') or [])}")
    print(f"  mod.js: {len(code.splitlines())} lines, {len(code)} bytes")

    pairs = COPY.findall(code)
    missing, present = [], 0
    dup = {}
    for src, dst in pairs:
        dup[dst] = dup.get(dst, 0) + 1
        if (root / src).exists():
            present += 1
        else:
            missing.append(src)
    print(f"  copyFile calls: {len(pairs)}  sources present: {present}  missing: {len(missing)}")
    for src in missing[:10]:
        print(f"    MISSING SOURCE: {src}")
    if len(missing) > 10:
        print(f"    ... and {len(missing) - 10} more")

    repeated = {d: n for d, n in dup.items() if n > 1}
    if repeated:
        print(f"  destinations copied more than once: {len(repeated)}")
        for d, n in list(repeated.items())[:5]:
            print(f"    {d} x{n}")

    # API misuse: read/write round-trips, and reads of binary files
    print("  read calls (each one is a no-op or a hazard):")
    for api, rx in READS.items():
        hits = rx.findall(code)
        if not hits:
            continue
        binary = [h for h in hits if Path(h).suffix.lower() in BINARY_EXT]
        print(f"    {api}: {len(hits)}"
              + (f"  ({len(binary)} on binary-looking files)" if binary else ""))
    odd_targets = [dst for _, dst in pairs if dst.startswith(("data/", "/", "content/"))]
    if odd_targets:
        print(f"  destinations that look like MPQ layout rather than data/ targets: {len(odd_targets)}")
        for d in odd_targets[:5]:
            print(f"    {d}")
    print()
