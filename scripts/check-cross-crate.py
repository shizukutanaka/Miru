#!/usr/bin/env python3
"""Find definitions and uses that disagree across the workspace.

WHY THIS EXISTS

The offline gate cannot compile across crate boundaries, and the branch history
says that gap is not theoretical — twice a change to a definition in one crate
was not carried to its use in another, and neither the gate nor review caught
it:

    8539eb0  SessionEvent gained a field; the literal in state.rs did not
    8c7822a  session::run lost a parameter; the call site still passed it

Both are checked, and both are verified against the commit where they happened:
running this on 8539eb0^ and 8c7822a^ reports the real defect in each, and it is
clean on current HEAD. That is the bar for the checks being worth having.

The argument-count check took two attempts. Matching a call to a definition by
bare name does not work — `run` is defined in several crates, so the rule that
resolves the ambiguity discards the very call worth checking. Resolving a
*path-qualified* call within the calling file's own crate does work:
`crate::session::run(..)` names one module in one crate, so the comparison is
sound and needs no guessing.

WHAT IT DOES NOT DO

Trait bounds, generic arguments, wrong API calls into a dependency, lifetimes,
and any call that is not path-qualified — all still invisible. This closes two
specific holes; it does not make `cargo test --workspace` optional.

BIAS

Deliberately under-reports. A false positive costs a person an investigation and
teaches them to distrust the check; a miss leaves them exactly where they were.
So anything ambiguous — a literal with `..base`, a defaulted or non_exhaustive
struct, a method call, a bare-name call, a closure in the argument list, a name
defined more than once in the same crate — is skipped rather than guessed at.

Usage:
  check-cross-crate.py [--root DIR] [-v]
"""

import argparse
import os
import re
import subprocess
import sys
from collections import defaultdict

# ── Source collection ────────────────────────────────────────────────────────


def rust_files(root):
    """Tracked .rs files under root, excluding tests/benches/fuzz.

    Only `src/` is considered: a test may legitimately construct a type through
    a helper this crude parser cannot follow.
    """
    out = subprocess.run(
        ["git", "-C", root, "ls-files", "*.rs"], capture_output=True, text=True
    ).stdout.split()
    return [f for f in out if "/src/" in f]


def strip_noise(src):
    """Blank out comments and string literals so braces inside them don't count.

    Replaced with spaces rather than removed, so byte offsets stay valid.
    """
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        two = src[i : i + 2]
        if two == "//":
            j = src.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                out[k] = " "
            i = j
        elif two == "/*":
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            for k in range(i, j):
                if out[k] != "\n":
                    out[k] = " "
            i = j
        elif src[i] == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    j += 1
                    break
                j += 1
            # Blank the contents but KEEP the quotes: erasing them too turns
            # `f(a, "x")` into `f(a,   )`, which then reads as a one-argument
            # call. That produced 184 false positives on clean code.
            for k in range(i + 1, min(j, n) - 1):
                if out[k] != "\n":
                    out[k] = " "
            i = j
        else:
            i += 1
    return "".join(out)


def match_delim(src, start, open_ch, close_ch):
    """Index just past the delimiter closing the one at `start`, or -1."""
    depth = 0
    i = start
    while i < len(src):
        if src[i] == open_ch:
            depth += 1
        elif src[i] == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return -1


def split_top(body, sep=","):
    """Split on `sep` at nesting depth zero (so generics and calls stay whole)."""
    parts, depth, cur = [], 0, []
    for ch in body:
        if ch in "([{<":
            depth += 1
        elif ch in ")]}>":
            depth -= 1
        if ch == sep and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    if "".join(cur).strip():
        parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


def line_of(src, idx):
    return src.count("\n", 0, idx) + 1


# ── Struct literals vs their definitions ─────────────────────────────────────

SKIP_ATTRS = ("non_exhaustive", "serde(default)", "derive(Default)")


def collect_structs(files, root):
    """name -> set of required field names, for structs safe to check.

    Skipped: generic structs, tuple/unit structs, anything with a defaulting or
    non_exhaustive attribute, per-field #[serde(default)], and any name defined
    more than once in the workspace.
    """
    found = defaultdict(list)
    # Every definition seen, including ones skipped below. A name defined twice
    # is ambiguous at a use site even when only one definition is checkable —
    # miru-capture and miru-common both define AudioFrame, and matching a
    # literal of one against the fields of the other invents a mismatch.
    seen = defaultdict(int)
    for rel in files:
        src = strip_noise(open(os.path.join(root, rel), encoding="utf-8").read())
        for m in re.finditer(r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)", src, re.M):
            seen[m.group(1)] += 1
        for m in re.finditer(r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)([^;{]*)\{", src, re.M):
            name, generics = m.group(1), m.group(2)
            if "<" in generics:
                continue
            head = src[max(0, m.start() - 400) : m.start()]
            if any(a in head for a in SKIP_ATTRS):
                continue
            end = match_delim(src, src.index("{", m.start()), "{", "}")
            if end < 0:
                continue
            body = src[src.index("{", m.start()) + 1 : end - 1]
            fields, defaulted = set(), False
            for part in split_top(body):
                if "serde(default)" in part or "#[serde(skip" in part:
                    defaulted = True
                    break
                fm = re.search(r"(?:^|\])\s*(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:", part)
                if fm:
                    fields.add(fm.group(1))
            if defaulted or not fields:
                continue
            found[name].append(fields)
    return {n: f[0] for n, f in found.items() if len(f) == 1 and seen[n] == 1}


def check_struct_literals(files, root, structs):
    problems = []
    for rel in files:
        raw = open(os.path.join(root, rel), encoding="utf-8").read()
        src = strip_noise(raw)
        for name, required in structs.items():
            for m in re.finditer(rf"\b{re.escape(name)}\s*\{{", src):
                # `struct X {` / `impl X {` / `enum X {` are definitions.
                before = src[max(0, m.start() - 40) : m.start()]
                if re.search(r"\b(struct|enum|impl|trait|union)\s+$", before):
                    continue
                # `fn f() -> Type {` — a return type, and what follows is the
                # function body, not a field list.
                if re.search(r"->\s*$", before):
                    continue
                brace = src.index("{", m.start())
                end = match_delim(src, brace, "{", "}")
                if end < 0:
                    continue
                body = src[brace + 1 : end - 1]
                if ".." in body:  # functional update: the rest comes from base
                    continue
                present = set()
                ok = True
                for part in split_top(body):
                    # Every entry must be `name: value` or the shorthand `name`.
                    # Anything else means this is not a struct literal at all
                    # (a block, a match arm), so we must not judge it.
                    fm = re.match(r"^(\w+)\s*(?::|$)", part)
                    if not fm or re.search(r"[;{}]", part):
                        ok = False
                        break
                    present.add(fm.group(1))
                if not ok or not present:
                    continue
                missing = required - present
                extra = present - required
                if missing or extra:
                    problems.append(
                        (
                            rel,
                            line_of(src, m.start()),
                            name,
                            sorted(missing),
                            sorted(extra),
                        )
                    )
    return problems


# ── Path-qualified calls vs their definitions ────────────────────────────────
#
# Matching a call to a definition by bare name does not work: `run` is defined
# in several crates, so either the check is ambiguous or the rule that resolves
# the ambiguity throws away the case worth checking. A path-qualified call names
# the module it is calling into — `crate::session::run(..)` — and that resolves
# to exactly one file, so the arity comparison is sound.


def crate_of(rel):
    """The crate a source file belongs to, as the path above its `src/`.

    Two crates each define `run` in a `session.rs`, so the module stem alone is
    ambiguous — and `crate::session::run` means *this* crate, which makes the
    caller's own crate the correct scope to resolve in.
    """
    return rel.split("/src/")[0] if "/src/" in rel else os.path.dirname(rel)


def module_functions(files, root):
    """(crate, module stem, fn name) -> (arity, rel path), for free functions.

    Keyed by the file stem because `foo::bar()` resolves to `bar` defined in
    `foo.rs` or `foo/mod.rs`. Methods and duplicate definitions are dropped.
    """
    found = defaultdict(list)
    for rel in files:
        stem = os.path.basename(rel)[:-3]
        if stem == "mod":
            stem = os.path.basename(os.path.dirname(rel))
        src = strip_noise(open(os.path.join(root, rel), encoding="utf-8").read())
        for m in re.finditer(r"\bfn\s+(\w+)\s*(?:<[^>]*>)?\s*\(", src):
            paren = src.index("(", m.end() - 1)
            end = match_delim(src, paren, "(", ")")
            if end < 0:
                continue
            params = split_top(src[paren + 1 : end - 1])
            if params and params[0].lstrip("&").lstrip().startswith(("self", "mut self")):
                continue  # a method; the call site would be `.name(..)`
            found[(crate_of(rel), stem, m.group(1))].append((len(params), rel))
    return {k: v[0] for k, v in found.items() if len(v) == 1}


def check_qualified_calls(files, root, fns):
    problems = []
    for rel in files:
        src = strip_noise(open(os.path.join(root, rel), encoding="utf-8").read())
        for m in re.finditer(r"(?<![.\w])(?:crate::|self::|super::)?(\w+)::(\w+)\s*\(", src):
            mod_name, fn_name = m.group(1), m.group(2)
            # An uppercase first segment is a type, so this is an associated
            # function, not a module path.
            if mod_name[:1].isupper():
                continue
            hit = fns.get((crate_of(rel), mod_name, fn_name))
            if hit is None:
                continue
            arity, def_rel = hit
            paren = src.index("(", m.end() - 1)
            end = match_delim(src, paren, "(", ")")
            if end < 0:
                continue
            raw = src[paren + 1 : end - 1]
            # A closure's parameter list has commas that no bracket encloses,
            # so the split below would miscount. Skip rather than guess.
            if "|" in raw:
                continue
            args = split_top(raw)
            if len(args) != arity:
                problems.append(
                    (rel, line_of(src, m.start()), f"{mod_name}::{fn_name}", arity, len(args), def_rel)
                )
    return problems


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=".")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()
    root = os.path.abspath(args.root)

    files = rust_files(root)
    structs = collect_structs(files, root)
    lit = check_struct_literals(files, root, structs)
    fns = module_functions(files, root)
    calls = check_qualified_calls(files, root, fns)

    for rel, line, name, missing, extra in lit:
        detail = []
        if missing:
            detail.append("missing " + ", ".join(missing))
        if extra:
            detail.append("unknown " + ", ".join(extra))
        print(f"{rel}:{line}: {name} literal — {'; '.join(detail)}")
    for rel, line, name, want, got, def_rel in calls:
        print(f"{rel}:{line}: {name}() takes {want} argument(s), {got} passed ({def_rel})")
    if args.verbose:
        print(
            f"\nchecked {len(structs)} struct(s) and {len(fns)} module function(s) "
            f"across {len(files)} file(s)",
            file=sys.stderr,
        )
    total = len(lit) + len(calls)
    if total:
        print(f"\n{total} definition/use mismatch(es).")
    sys.exit(1 if total else 0)


if __name__ == "__main__":
    main()
