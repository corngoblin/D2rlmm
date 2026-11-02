# Local patches to CascLib

CascLib is vendored here so the builds are reproducible and the flatpak can compile it
from a source tree in this repository. Changes made to it are listed below and should be
carried forward if the vendored copy is ever updated.

Upstream: https://github.com/ladislav-zezula/CascLib
Vendored from commit `6b889ec8f421cbc9baccec296c582362ab917887` (3.0).

## 1. `CascFiles.cpp` — a malformed `.build.info` no longer segfaults

`ParseFile_BuildInfo` reads the `Active!DEC:1` column and passes it straight to
`strcmp`. A `.build.info` that is truncated, or that belongs to something else, has no
such column: `CASC_CSV_LINE::operator[]` returns the null column, whose `szValue` is
`NULL`, and `strcmp(NULL, "1")` is a segmentation fault rather than an error.

That is not a theoretical input. It is what happens when someone points the app at the
wrong folder — the folder has a `.build.info` in it, so it passes every cheap check, and
then the game index parser kills the process.

Verified before the patch: pointing Cairn at a directory holding a two-line
`.build.info` took the whole GUI down (SIGSEGV in `ParseFile_BuildInfo`, via
`LoadMainFile` → `CascOpenStorage`). After the patch the same folder produces

```
could not open game archives at …: CascOpenStorage failed for … 
```

and the window carries on.

The fix is a two-line guard (`IsActiveLine`) applied at both call sites, deliberately
narrow: it changes nothing for a well-formed file and does not alter what any other
column lookup returns, which a change to the shared null column would have.

If this is ever fixed upstream, drop the patch.
