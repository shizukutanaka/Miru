#!/usr/bin/env python3
"""Report .rs files that no `mod` declaration in their crate can reach.

An orphaned file is invisible to rustc: it is never compiled, its tests never
run, and no amount of cargo verification will look at it. `cargo build` says
nothing, so this is one of the few real defects a registry-less environment can
still find.

Search is deliberately crude — any `mod <name>;` or `#[path = "..."]` anywhere
in the crate counts, regardless of cfg. Under-reporting is the right bias: a
false positive costs someone an investigation, a missed orphan costs nothing
visible until a feature silently does not exist.
"""
import re, subprocess, sys
from collections import defaultdict

files = subprocess.run(["git", "ls-files", "*.rs"], capture_output=True, text=True).stdout.split()
by_crate = defaultdict(list)
for f in files:
    if "/src/" not in f:
        continue
    by_crate[f.split("/src/")[0]].append(f)

orphans = []
for crate, paths in by_crate.items():
    decls, pathattrs = set(), set()
    for p in paths:
        src = open(p).read()
        decls |= set(re.findall(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*[;{]", src, re.M))
        pathattrs |= {v.split("/")[-1] for v in re.findall(r'#\[path\s*=\s*"([^"]+)"\]', src)}
    for p in paths:
        base = p.split("/")[-1]
        if base in ("lib.rs", "main.rs", "mod.rs") or base in pathattrs:
            continue
        # build.rs and bin/ targets are entry points cargo finds by convention
        if "/src/bin/" in p or base == "build.rs":
            continue
        if base[:-3] not in decls:
            orphans.append(p)

for p in sorted(orphans):
    print(p)
sys.exit(1 if orphans else 0)
