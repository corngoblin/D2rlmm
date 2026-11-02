#!/bin/sh
# Build (and optionally install / bundle) D2rlmm as a flatpak.
#
#   ./flatpak/build.sh                 build into .flatpak-repo
#   ./flatpak/build.sh --install       build, then install for this user
#   ./flatpak/build.sh --bundle        build, then export D2rlmm.flatpak
#   ./flatpak/build.sh --bundle --install
#
# flatpak-builder runs inside org.flatpak.Builder, so none of this needs root. The
# state dir, the build dir and the repo all live under the repository so they are on
# the same filesystem — flatpak-builder refuses a repo on another one.
#
# The bundle name carries no version on purpose: the release tag does, and a stable
# filename is what lets an updater script find the asset.

set -eu

cd "$(dirname "$0")/.."
ROOT=$(pwd)
APP_ID=io.github.corngoblin.D2rlmm
# Spelled out because a repo can hold several branches, and `flatpak install
# <app-id>` then asks which one it means — unanswerable in a script.
BRANCH=$(python3 -c "import json;print(json.load(open('$APP_ID.json')).get('branch','master'))" 2>/dev/null || echo master)
MANIFEST=$APP_ID.json
REPO=$ROOT/.flatpak-repo
BUILD=$ROOT/.flatpak-build
STATE=$ROOT/.flatpak-builder
BUNDLE=$ROOT/$APP_ID.flatpak
RUNTIME_REPO=https://flathub.org/repo/flathub.flatpakrepo

INSTALL=0
BUNDLE_OUT=0
for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=1 ;;
        --bundle) BUNDLE_OUT=1 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

# Crate sources are generated from Cargo.lock; regenerate after touching dependencies.
if [ ! -f flatpak/cargo-sources.json ]; then
    echo "== generating crate sources from Cargo.lock"
    python3 flatpak/gen-cargo-sources.py > flatpak/cargo-sources.json
fi

echo "== building $APP_ID"
# --user inside the builder resolves to the sandbox's own data dir, so the real
# per-user installation is passed through explicitly.
flatpak run \
    --env=FLATPAK_USER_DIR="$HOME/.local/share/flatpak" \
    --command=flatpak-builder \
    org.flatpak.Builder \
    --user --force-clean --sandbox \
    --state-dir="$STATE" \
    --repo="$REPO" \
    "$BUILD" \
    "$MANIFEST"

if [ "$BUNDLE_OUT" = 1 ]; then
    echo "== exporting $APP_ID.flatpak"
    # The branch is positional, not an option: `--branch=` is rejected here.
    flatpak build-bundle --arch=x86_64 --runtime-repo="$RUNTIME_REPO" \
        "$REPO" "$BUNDLE" "$APP_ID" "$BRANCH"
    ls -la "$BUNDLE"
fi

if [ "$INSTALL" = 1 ]; then
    echo "== installing for the current user"
    flatpak install --user --reinstall --noninteractive -y "$REPO" "app/$APP_ID/x86_64/$BRANCH"
    echo "== installed:"
    flatpak run --command=d2rlmm-cli "$APP_ID" version
fi

echo "== done"
