# UI review: what D2RMM does, what D2rlmm does, and what to change

Notes taken by reading upstream's renderer (`~/Projects/d2rmm/src/renderer/react`) against
D2rlmm's GTK app (`crates/d2rlmm-app`). Not a wish list — each item names the upstream
behaviour it comes from.

## The mental model problem (the big one)

In both apps the mod list is a **staging area**: toggling a switch changes nothing on disk.
Install *builds* the game's data from the enabled set, and removing a mod means "untick it,
then install again". Upstream expresses that with state; D2rlmm said nothing about it, so
unticking looked like a no-op.

Upstream: `ModsContext` keeps the *installed* list (`id` + resolved config per mod),
`useIsInstallConfigChanged` compares it with what an install would produce now, and
`ModInstallButton` renders differently when they differ — outlined + hollow `Save` icon when
in sync, contained + filled `Save` when there is something to apply.

D2rlmm, before 0.1.3: one button, one label (`Install (N)`), identical whether or not the
selection matched the game.

**Done (0.1.3):** the installer now records what it installed (`installed.json` in the state
dir, resolved config included) and the header button reflects it:

| state | label | style |
|---|---|---|
| nothing enabled | `Uninstall` | destructive |
| selection differs from the game | `Apply changes (N)` | suggested |
| selection matches the game | `Installed (N)` | flat |

## What upstream has that D2rlmm does not

1. **Per-mod metadata at a glance.** `ModListItem` shows chips for version, author, website,
   data-mod status and a settings chip; the row's hover menu carries settings, website,
   update, download, Nexus id, rename, open folder, delete. D2rlmm's rows have a switch and a
   context menu — the version/author are not visible, and a mod that failed last run is not
   marked.
2. **Search/filter over the library.** Upstream virtualises and filters the list
   (`react-window` + search). D2rlmm rebuilds a `GtkListBox`; fine at 18 mods, not at 200.
3. **Reorder by drag.** Upstream uses `react-beautiful-dnd` with a drag handle. D2rlmm moves
   mods with menu actions; order matters (two mods writing the same file) and is not obvious.
4. **Install progress in context.** Upstream drives a `LoadingButton` + progress dialog. D2rlmm
   pulses a `GtkProgressBar` in the toolbar; the log is only readable afterwards.
5. **Confirm the destructive path.** Upstream's uninstall is a distinct button; D2rlmm's
   `Uninstall` is the same header button with a red style and no confirmation dialog.
6. **Keyboard**: `Ctrl+I`-style accelerators for install/reload are absent in D2rlmm.

## GNOME-fit notes (where "a bit odd" comes from)

- The header carries two unrelated things: *act on the set* (Install) and *app menu*. That is
  correct for GNOME, but the button's meaning changes under the user (Install → Uninstall)
  without a transition, which reads as a bug.
- `AdwBanner` is currently reused for three different messages (no game folder, unreadable
  folders, install failures). GNOME guidance is one persistent issue per banner; failures
  belong in a dialog or the log.
- Row subtitles show description text; a second line with `version · author` plus a warning
  icon for "failed last install" would carry more information at the same height.
- The empty state has a button but no explanation of the mod folder layout, which is the first
  thing a new user needs (upstream links its docs).

## Order of work, cheapest-first

1. *Done 0.1.3* — pending-changes state on the header button.
2. Confirm before uninstall ("Remove N mods and restore vanilla?").
3. Row subtitle: `version · author`, plus a warning icon when the last run failed for that mod.
4. `Ctrl+I` / `Ctrl+R` accelerators; tooltip the button with the shortcut.
5. Empty-state paragraph describing the folder layout (`mod.json`, `mod.js`, assets relative).
6. Move failed-mod reporting out of the banner into the log dialog, leaving the banner for
   configuration problems.
7. Drag-to-reorder (or up/down buttons) with the order explained ("later mods win").
8. Search entry above the list (only worth it past ~50 mods).

## Do not copy from upstream

- Its Electron/MUI look; the point of D2rlmm is that it feels native.
- Its update/Nexus integration; out of scope by decision.
- Silent swallowing of non-Error throws — that is one of the things this port set out to do
  better (see the `Null"null"` episode in PROJECT.md).
