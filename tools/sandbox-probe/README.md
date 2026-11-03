# Sandbox probe

Tools for the class of failure where a mod dies with a bare `null` (QuickJS aborting on
its own limits) rather than an error message.

- `compare-run.js "<mod folder>"` — runs a mod's `mod.js` against a stubbed API and prints
  every effect as JSON: files written with their SHA-256, log lines, peak heap. Diff two
  outputs to prove that a change (a translation, a refactor) altered nothing.
- `mem-probe.js "<mod folder>" [entitiesPerPreset]` — sizes the run with presets like the
  real game data and reports peak heap, so you can tell whether a mod will hit the sandbox
  cap. `CONFIG='{"RoadVisible":false}'` overrides mod options to find what costs the memory.
- `make-fixture.js "<mod folder>" <out dir> [entitiesPerPreset]` — writes a synthetic
  extracted-data tree containing every file the mod asks for (the list is discovered by
  recording `readJson`), for driving `d2rlmm-cli install --extracted` end to end.

Typical sequence for "mod X aborts with a null exception":

```sh
node tools/sandbox-probe/make-fixture.js "<mod>" /tmp/fixture 2000
D2RLMM_JS_MEMORY_MB=512 d2rlmm-cli install --extracted /tmp/fixture \
    --mods /tmp/mods --direct --data-out /tmp/out     # reproduces, and names the cap
d2rlmm-cli install --extracted /tmp/fixture --mods /tmp/mods --direct --data-out /tmp/out
```
