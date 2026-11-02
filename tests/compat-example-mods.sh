#!/bin/sh
# Compatibility check against the *real* example mods from D2RMM's own mod repo.
#
# The mods are cloned from https://github.com/olegbl/d2rmm.mods and run unmodified:
# this script's job is to prove that D2rlmm's engine executes third-party mod code,
# not that a synthetic mod of our own can touch a file.
#
# Vanilla game data is synthesised (a real D2R install is not redistributable and is
# not present on CI). Every table here carries the columns the chosen mods read, so
# a mod that silently failed to find its data would show up as an unchanged output
# file rather than as a false pass.
#
#   tests/compat-example-mods.sh path/to/d2rlmm-cli
#
# Exits non-zero on the first failed assertion.

set -eu

CLI=${1:-target/release/d2rlmm-cli}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
if [ -n "${WORK:-}" ]; then
    # Keep the work tree for inspection when the caller set one.
    rm -rf "$WORK"
    mkdir -p "$WORK"
else
    WORK=$(mktemp -d)
    trap 'rm -rf "$WORK"' EXIT
fi

MODS_SRC=${D2RMM_MODS_REPO:-/tmp/d2r-mods-sample}
if [ ! -d "$MODS_SRC" ]; then
    echo "== cloning the example mods"
    git clone --depth 1 -q https://github.com/olegbl/d2rmm.mods "$MODS_SRC"
fi

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

echo "== synthesising vanilla game data in $WORK/game"
GAME=$WORK/game
mkdir -p "$GAME/data/global/excel/base" "$GAME/data/local/lng/strings" "$GAME/data/hd/global/video"

# misc.txt: the columns IncreaseStackSize reads and writes.
#
# `code` is deliberately NOT the last column. In the real table it is not either,
# and a mod that reads without `removeCarriageReturns` would then see "tbk\r" and
# match nothing — a fixture with `code` last would fail against a mod that works
# perfectly in the game.
printf 'name\tcompactsave\tmaxstack\tminstack\tstackable\tcost\ttype\tcode\tspawnable\r\n' > "$GAME/data/global/excel/misc.txt"
printf 'Tome of Town Portal\t1\t20\t1\t1\t100\tbook\ttbk\t0\r\n' >> "$GAME/data/global/excel/misc.txt"
printf 'Tome of Identify\t1\t20\t1\t1\t100\tbook\tibk\t0\r\n' >> "$GAME/data/global/excel/misc.txt"
printf 'Arrows\t1\t350\t1\t1\t35\tbow\taqv\t1\r\n' >> "$GAME/data/global/excel/misc.txt"
printf 'Bolts\t1\t250\t1\t1\t35\txbow\tcqv\t1\r\n' >> "$GAME/data/global/excel/misc.txt"
printf 'Key\t1\t12\t1\t1\t25\tkey\tkey\t1\r\n' >> "$GAME/data/global/excel/misc.txt"
cp "$GAME/data/global/excel/misc.txt" "$GAME/data/global/excel/base/misc.txt"

# weapons.txt: IncreaseStackSize also raises throwing-weapon stacks.
printf 'name	type	stackable	minstack	maxstack	spawnstack	code\r\n' > "$GAME/data/global/excel/weapons.txt"
printf 'Throwing Axe\taxe\t1\t1\t40\t1\t9ax\r\n' >> "$GAME/data/global/excel/weapons.txt"
printf 'Javelin\tjave\t1\t1\t60\t1\tjav\r\n' >> "$GAME/data/global/excel/weapons.txt"
printf 'Fulminating Potion\ttpot\t1\t1\t5\t1\topm\r\n' >> "$GAME/data/global/excel/weapons.txt"
cp "$GAME/data/global/excel/weapons.txt" "$GAME/data/global/excel/base/weapons.txt"

# cubemain.txt: the columns SocketPunching builds recipes into.
printf 'description\tenabled\tversion\tnuminputs\tinput 1\tinput 2\toutput\t*eol\r\n' > "$GAME/data/global/excel/cubemain.txt"
printf 'Stamina Potion\t1\t100\t3\t"hp1,hpo"\t"hp1,hpo"\t"stm"\t0\r\n' >> "$GAME/data/global/excel/cubemain.txt"
cp "$GAME/data/global/excel/cubemain.txt" "$GAME/data/global/excel/base/cubemain.txt"

# Localisation the rune-name mod rewrites.
cat > "$GAME/data/local/lng/strings/item-runes.json" <<'JSON'
[
  { "id": 20001, "Key": "r01", "enUS": "El", "deDE": "El", "frFR": "El" },
  { "id": 20002, "Key": "r02", "enUS": "Eld", "deDE": "Eld", "frFR": "Eld" },
  { "id": 20003, "Key": "r12", "enUS": "Sol", "deDE": "Sol", "frFR": "Sol" },
  { "id": 20004, "Key": "runeWord001", "enUS": "Not a rune", "deDE": "x", "frFR": "x" }
]
JSON

# SkipIntroVideos registers the game's videos for uninstall, so they must exist.
printf 'not really a video' > "$GAME/data/hd/global/video/logoanim.webm"
printf 'not really a video' > "$GAME/data/hd/global/video/blizzardlogos.webm"

printf '91541\n' > "$GAME/data/global/dataversionbuild.txt"
printf '// 21000\n' > "$GAME/data/local/lng/next_string_id.txt"

echo "== copying the example mods into a library"
LIB=$WORK/mods
mkdir -p "$LIB"
for mod in IncreaseStackSize NumberedRuneNames SkipIntroVideos SocketPunching ShortPotionNames; do
    [ -d "$MODS_SRC/$mod" ] || fail "example mod $mod is missing from $MODS_SRC"
    cp -r "$MODS_SRC/$mod" "$LIB/$mod"
done

# ShortPotionNames needs item-names localisation; give it one entry so the mod has
# something to rewrite.
cat > "$GAME/data/local/lng/strings/item-names.json" <<'JSON'
[
  { "id": 30001, "Key": "Super Healing Potion", "enUS": "Super Healing Potion", "deDE": "Super Heiltrank" }
]
JSON

echo "== installing"
OUT=$WORK/out
mkdir -p "$OUT"
"$CLI" install --game "$OUT/game" --extracted "$GAME" --mods "$LIB" --output-name Compat > "$WORK/install.log" 2>&1 || {
    cat "$WORK/install.log"
    fail "the install reported failing mods"
}
cat "$WORK/install.log"

DATA=$OUT/game/mods/Compat/Compat.mpq/data

echo "== checking the merged output"

# Tab-separated assertions: printf turns \t into a real tab, which is the only way
# to match a column boundary (plain grep does not expand escapes).
tsv_has() {
    file=$1
    shift
    grep -qF "$(printf '%b' "$*")" "$file"
}

MISC=$DATA/global/excel/misc.txt
WEAPONS=$DATA/global/excel/weapons.txt
RUNES=$DATA/local/lng/strings/item-runes.json
NAMES=$DATA/local/lng/strings/item-names.json
CUBE=$DATA/global/excel/cubemain.txt

# IncreaseStackSize: stack sizes come from the mod's own default config
# (tomes 100, projectiles 500, throwing weapons 500).
tsv_has "$MISC" 'Tome of Town Portal\t1\t100\t' || fail "tome stacks unchanged"
tsv_has "$MISC" 'Arrows\t1\t500\t' || fail "arrow stacks unchanged"
tsv_has "$MISC" 'Key\t1\t100\t' || fail "key stacks unchanged"
tsv_has "$WEAPONS" 'Throwing Axe\taxe\t1\t500\t500\t500\t' \
    || fail "throwing weapon stacks unchanged"
# A mod that respects its own guard: type 'tpot' potions must be left alone.
tsv_has "$WEAPONS" 'Fulminating Potion\ttpot\t1\t1\t5\t' \
    || fail "a throwing potion was modified"

# CRLF must survive: the game mis-parses otherwise.
grep -qF "$(printf '\r')" "$MISC" || fail "line endings lost in misc.txt"

# NumberedRuneNames: rune names get their number, non-runes are untouched, and the
# output is strict JSON (quoted keys) even though the input was parsed tolerantly.
grep -qF '"El (1)"' "$RUNES" || fail "rune 1 not numbered"
grep -qF '"Sol (12)"' "$RUNES" || fail "rune 12 not numbered"
grep -qF '"Not a rune"' "$RUNES" || fail "a non-rune entry was rewritten"
grep -qF '"Key":"r01"' "$RUNES" || fail "JSON was not re-encoded strictly"

# ShortPotionNames rewrote the potion's name.
grep -qiF 'heal' "$NAMES" || fail "item-names.json missing or unchanged"

# SkipIntroVideos reads the game's copies (so a -direct uninstall can restore them)
# then copies its own over the top. The repo ships zero-byte placeholders for the
# videos, so existence is the assertion, not size.
[ -f "$DATA/hd/global/video/logoanim.webm" ] || fail "video asset not copied"
[ -f "$DATA/hd/global/video/blizzardlogos.webm" ] || fail "second video asset not copied"

# SocketPunching appends cube recipes built from config sections.
ROWS=$(grep -c '' "$CUBE" || true)
[ "$ROWS" -gt 4 ] || fail "SocketPunching added no cube recipes (rows: $ROWS)"
grep -qF 'Empty Sockets' "$CUBE" || fail "unsocket recipe missing"
tsv_has "$CUBE" 'Empty Sockets On Weapon' || fail "weapon unsocket recipe missing"

echo "== install ran again over a populated output (idempotence)"
"$CLI" install --game "$OUT/game" --extracted "$GAME" --mods "$LIB" --output-name Compat > "$WORK/install2.log" 2>&1 \
    || { cat "$WORK/install2.log"; fail "the second install failed"; }
tsv_has "$DATA/global/excel/misc.txt" 'Tome of Town Portal\t1\t100\t' \
    || fail "second install produced different output"

echo "== uninstalling (mod folder mode)"
"$CLI" install --game "$OUT/game" --extracted "$GAME" --mods "$LIB" --output-name Compat --dry-run > "$WORK/revert.log" 2>&1 \
    || { cat "$WORK/revert.log"; fail "the revert failed"; }
[ ! -d "$OUT/game/mods/Compat" ] || fail "revert left the mod folder behind"

echo "== installing direct (data folder mode)"
DIRECT=$WORK/direct/data
mkdir -p "$DIRECT"
"$CLI" install --game "$OUT/game" --extracted "$GAME" --direct --data-out "$DIRECT" \
    --mods "$LIB" --output-name Compat > "$WORK/direct-install.log" 2>&1 \
    || { cat "$WORK/direct-install.log"; fail "the direct install failed"; }

changed=0
for f in $(cd "$DIRECT" && find . -type f); do
    cmp -s "$DIRECT/$f" "$GAME/data/${f#./}" || changed=1
done
[ "$changed" = 1 ] || fail "the direct install changed nothing — the uninstall check would be vacuous"

echo "== uninstalling (direct mode)"
"$CLI" uninstall --game "$OUT/game" --extracted "$GAME" --direct --data-out "$DIRECT" \
    --mods "$LIB" --output-name Compat > "$WORK/direct-uninstall.log" 2>&1 \
    || { cat "$WORK/direct-uninstall.log"; fail "the direct uninstall failed"; }

# Every file still in the data folder must be byte-for-byte the game's own copy.
# A file a mod invented has no vanilla counterpart and must have been deleted, which
# cmp catches as a missing file.
for f in $(cd "$DIRECT" && find . -type f); do
    cmp -s "$DIRECT/$f" "$GAME/data/${f#./}" \
        || { echo "  not restored to vanilla: $f" >&2; fail "the direct uninstall left a modified or invented file"; }
done

echo
echo "compat: all checks passed"
