#!/usr/bin/env python3
"""List `pub` items that nothing in the workspace references.

Musk's second step is delete, and this is the input to it. Run it, then decide
per item — this script deliberately does not fail a build, because "unreferenced
today" and "should be removed" are different claims and only a human can close
the gap.

Method: a name declared exactly once and appearing exactly once across every
tracked .rs (its own definition) is referenced by nothing — including tests,
benches and fuzz targets, which are searched too. Text matching, so it cannot
see through macros; treat output as candidates, not a verdict.

Usage: scripts/find-dead-code.py [--crate <name>]
"""
import collections
import re
import subprocess
import sys

files = subprocess.run(
    ["git", "ls-files", "*.rs"], capture_output=True, text=True
).stdout.split()
src = {f: open(f).read() for f in files}
allsrc = "\n".join(src.values())

only = None
if "--crate" in sys.argv:
    only = sys.argv[sys.argv.index("--crate") + 1]

defs = collections.defaultdict(list)
for f, s in src.items():
    if "/src/" not in f or (only and f"/{only}/" not in f):
        continue
    for m in re.finditer(
        r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:fn|struct|enum|trait|const|static)\s+(\w+)",
        s,
        re.M,
    ):
        defs[m.group(1)].append(f)

hits = [
    (w[0], n)
    for n, w in defs.items()
    if len(w) == 1 and len(re.findall(rf"\b{re.escape(n)}\b", allsrc)) == 1
]
for f, n in sorted(hits):
    print(f"{f}: {n}")
print(f"\n{len(hits)} pub item(s) referenced nowhere in the workspace.")
