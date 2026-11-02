#!/usr/bin/env python3
"""Turn Cargo.lock into the source list flatpak-builder needs.

A flatpak build has no network, so every crate has to arrive as a source entry with
a checksum. Cargo.lock already records the sha256 of every crate it resolved, so the
whole list can be generated from it — no vendored copy of half of crates.io in the
repository, and nothing to keep in sync by hand.

Run from the repository root after any dependency change:

    ./flatpak/gen-cargo-sources.py > flatpak/cargo-sources.json

The output is the same shape `flatpak-cargo-generator.py` produces: each crate is
unpacked under cargo/vendor/, a cargo config points at that directory, and the
build runs `cargo build --offline`.
"""

import json
import sys
import tomllib
from pathlib import Path

CRATE_URL = "https://static.crates.io/crates/{name}/{name}-{version}.crate"
CARGO_CONFIG = """[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "cargo/vendor"
"""


def main() -> int:
    lock_path = Path("Cargo.lock")
    if not lock_path.exists():
        print("no Cargo.lock — run `cargo fetch` first", file=sys.stderr)
        return 1

    with lock_path.open("rb") as handle:
        lock = tomllib.load(handle)

    sources = []
    skipped = []
    for package in lock.get("package", []):
        # A workspace member has no source and no checksum: it is built from the
        # repository, not downloaded.
        if package.get("source") != "registry+https://github.com/rust-lang/crates.io-index":
            skipped.append(package["name"])
            continue
        name, version = package["name"], package["version"]
        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": CRATE_URL.format(name=name, version=version),
                "sha256": package["checksum"],
                "dest": f"cargo/vendor/{name}-{version}",
            }
        )
        # A vendored directory needs a checksum file per crate, or `cargo build
        # --offline` refuses the source with "failed to read checksums". An empty
        # `files` map means the per-file hashes are not checked; the package hash
        # still pins the crate.
        #
        # An `inline` source takes `dest` as a *directory* and needs
        # `dest-filename` for the name: with the whole path in `dest` alone,
        # flatpak-builder stops with `Property "dest-filename" must be set`.
        sources.append(
            {
                "type": "inline",
                "contents": json.dumps(
                    {"files": {}, "package": package["checksum"]}, separators=(",", ":")
                ),
                "dest": f"cargo/vendor/{name}-{version}",
                "dest-filename": ".cargo-checksum.json",
            }
        )

    sources.append(
        {
            "type": "inline",
            "contents": CARGO_CONFIG,
            "dest": "cargo",
            "dest-filename": "config.toml",
        }
    )

    json.dump(sources, sys.stdout, indent=2)
    sys.stdout.write("\n")
    print(
        f"# {len(skipped)} crates skipped, {(len(sources) - 1) // 2} crates fetched",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
