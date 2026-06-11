#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Compute a library-consumer reading order for all Rust files in the workspace.

Used to (re)generate ``codebase-reading-checklist.md``. The order guarantees
that every file a given file imports appears earlier in the list, so a reader
working top-to-bottom always has the full picture of the imported contents.

Method:
 1. Parse every ``use``/``pub use`` statement, ``mod`` declaration, and inline
    qualified path (``crate::…``, ``graphcal_*::…``, ``super::…``) in each file.
 2. Resolve paths through re-export chains (explicit ``pub use`` items and
    ``pub use …::*`` globs, the latter by checking item definitions) down to
    the file that defines the item. ``mod child;`` declarations create no
    edge — they don't import content.
 3. Condense strongly connected components (genuinely mutually dependent
    files) and topologically sort, using the current checklist order as a
    tie-break so re-runs stay stable. Within a cycle, files with fewer
    intra-cycle dependencies come first; ``tests.rs`` and ``mod.rs``-style
    entry files come last.
 4. Verify: no forward edges outside acknowledged cycles.

Output: the ordered file list, then the cycle groups (to annotate in the
checklist) and any forward edges. Stage headings in the checklist are curated
by hand; keep each stage a contiguous slice of this order.
"""

from __future__ import annotations

import heapq
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECKLIST = Path(__file__).resolve().parent / "codebase-reading-checklist.md"
CRATE_ORDER = [
    "graphcal-compiler",
    "graphcal-io",
    "graphcal-eval",
    "graphcal-fmt",
    "graphcal-lsp",
    "graphcal-cli",
]
CRATE_NAMES = {c.replace("-", "_"): c for c in CRATE_ORDER}

# Current checklist order, used as a tie-break so the output stays close to
# the curated pedagogy wherever the dependency graph allows it.
EXISTING_POS: dict[str, int] = {}
if CHECKLIST.exists():
    for i, m in enumerate(re.finditer(r"`(crates/[^`]+\.rs)`", CHECKLIST.read_text())):
        EXISTING_POS.setdefault(m.group(1), i)


def collect_files() -> list[Path]:
    return sorted(
        p.relative_to(ROOT)
        for p in (ROOT / "crates").rglob("*.rs")
        if "target" not in p.parts
    )


def crate_of(rel: Path) -> str:
    return rel.parts[1]


def module_path(rel: Path) -> tuple[str, ...] | None:
    """Module path of a src file as (crate_snake, seg, seg, ...). None for tests/."""
    crate = crate_of(rel)
    parts = rel.parts[2:]  # after crates/<crate>/
    if parts[0] != "src":
        return None
    segs = list(parts[1:])
    stem = segs[-1].removesuffix(".rs")
    segs = segs[:-1]
    if stem not in ("mod", "lib", "main"):
        segs.append(stem)
    return (crate.replace("-", "_"), *segs)


def expand_use(stmt: str) -> list[list[str]]:
    """Expand a use statement body (no 'use ', no ';') into segment lists.

    An `x as y` leaf becomes the segment "x@y" (target@alias)."""
    stmt = stmt.strip()
    if not stmt:
        return []
    brace = stmt.find("{")
    if brace == -1:
        segs = [s.strip() for s in stmt.split("::")]
        segs = [re.sub(r"\s+as\s+", "@", s) for s in segs if s]
        return [segs] if segs else []
    prefix = stmt[:brace].rstrip().rstrip("::").rstrip(":")
    # find matching close brace
    depth = 0
    for i in range(brace, len(stmt)):
        if stmt[i] == "{":
            depth += 1
        elif stmt[i] == "}":
            depth -= 1
            if depth == 0:
                inner = stmt[brace + 1 : i]
                break
    else:
        return []
    # split inner on top-level commas
    items, d, cur = [], 0, ""
    for ch in inner:
        if ch == "{":
            d += 1
        elif ch == "}":
            d -= 1
        if ch == "," and d == 0:
            items.append(cur)
            cur = ""
        else:
            cur += ch
    items.append(cur)
    pre = [s.strip() for s in prefix.split("::") if s.strip()] if prefix else []
    out = []
    for item in items:
        for sub in expand_use(item):
            out.append(pre + sub)
    return out


USE_RE = re.compile(r"^\s*(pub(?:\([^)]*\))?\s+)?use\s+(.*)$")
MOD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;")
INLINE_RE = re.compile(
    r"\b(crate|super|self|"
    + "|".join(CRATE_NAMES)
    + r")::([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)"
)


def parse_file(path: Path) -> tuple[list[list[str]], list[list[str]], list[str]]:
    """Return (use/inline segment lists, pub-use segment lists, child mod names)."""
    text = path.read_text()
    # strip block/line comments crudely
    text = re.sub(r"//[^\n]*", "", text)
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    paths: list[list[str]] = []
    pub_uses: list[list[str]] = []
    mods: list[str] = []
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        m = USE_RE.match(line)
        if m:
            is_pub = m.group(1) is not None
            stmt = m.group(2)
            while ";" not in stmt and i + 1 < len(lines):
                i += 1
                stmt += " " + lines[i].strip()
            stmt = stmt.split(";")[0]
            expanded = expand_use(stmt)
            paths.extend(expanded)
            if is_pub:
                pub_uses.extend(expanded)
        else:
            m = MOD_RE.match(line)
            if m:
                mods.append(m.group(1))
            else:
                for im in INLINE_RE.finditer(line):
                    paths.append([im.group(1)] + im.group(2).split("::"))
        i += 1
    return paths, pub_uses, mods


DEF_RE_TMPL = r"\b(?:struct|enum|fn|trait|type|const|static|union|macro)\s+{}\b|macro_rules!\s+{}\b"


def defines_item(text: str, item: str) -> bool:
    return (
        re.search(DEF_RE_TMPL.format(re.escape(item), re.escape(item)), text)
        is not None
    )


def main() -> None:
    files = collect_files()
    mod_to_file: dict[tuple[str, ...], Path] = {}
    file_to_mod: dict[Path, tuple[str, ...]] = {}
    texts: dict[Path, str] = {}
    for f in files:
        texts[f] = (ROOT / f).read_text()
        mp = module_path(f)
        if mp is not None:
            mod_to_file[mp] = f
            file_to_mod[f] = mp

    parsed = {f: parse_file(ROOT / f) for f in files}

    def absolutize(segs: list[str], f: Path) -> list[str] | None:
        """Make a use path absolute (crate-snake-rooted); None if external/unresolvable."""
        crate_snake = crate_of(f).replace("-", "_")
        in_tests = f.parts[2] == "tests"
        self_mod = file_to_mod.get(f)
        head = segs[0]
        if head in CRATE_NAMES:
            return segs
        if head == "crate":
            if in_tests:
                return None
            return [crate_snake] + segs[1:]
        if head in ("super", "self"):
            if self_mod is None:
                return None
            base = list(self_mod)
            rest = segs[1:]
            if head == "super":
                base = base[:-1]
                while rest and rest[0] == "super":
                    base = base[:-1]
                    rest = rest[1:]
            return base + rest
        # uniform path: child module of current module, or crate-root module
        if self_mod is not None and (*self_mod, head) in mod_to_file:
            return list(self_mod) + segs
        if (crate_snake, head) in mod_to_file:
            return [crate_snake] + segs
        return None  # external crate or std

    # re-export tables: file -> {exported item name -> absolute target segs},
    # and file -> [absolute glob target segs]
    reexports: dict[Path, dict[str, list[str]]] = {f: {} for f in files}
    globs: dict[Path, list[list[str]]] = {f: [] for f in files}
    for f in files:
        _, pub_uses, _ = parsed[f]
        for segs in pub_uses:
            abs_segs = absolutize(segs, f)
            if abs_segs is None:
                continue
            leaf = abs_segs[-1]
            if leaf == "*":
                globs[f].append(abs_segs[:-1])
            elif "@" in leaf:
                target, alias = leaf.split("@", 1)
                reexports[f][alias] = abs_segs[:-1] + [target]
            else:
                reexports[f][leaf] = abs_segs

    def module_prefix(segs: list[str]) -> tuple[Path, list[str]] | None:
        for k in range(len(segs), 0, -1):
            key = tuple(segs[:k])
            if key in mod_to_file:
                return mod_to_file[key], segs[k:]
        return None

    def resolve(segs: list[str], visited: frozenset = frozenset()) -> Path | None:
        """Resolve an absolute path to its defining file, following re-exports."""
        hit = module_prefix(segs)
        if hit is None:
            return None
        file, rest = hit
        if not rest:
            return file
        item = rest[0].split("@", 1)[0]
        if item == "*":
            return file
        key = (file, item)
        if key in visited:
            return file
        visited = visited | {key}
        if item in reexports[file]:
            return resolve(reexports[file][item] + rest[1:], visited)
        for g in globs[file]:
            gf = resolve(g, visited)
            if gf is None:
                continue
            if item in reexports[gf]:
                return resolve(reexports[gf][item] + rest[1:], visited)
            if defines_item(texts[gf], item):
                return gf
        return file  # defined in this module file itself (best effort)

    deps: dict[Path, set[Path]] = {f: set() for f in files}
    for f in files:
        raw_paths, _, _ = parsed[f]
        # NOTE: `mod child;` declarations deliberately create no edge — they don't
        # import content. Only use/pub-use edges order the reading.
        for segs in raw_paths:
            abs_segs = absolutize(segs, f)
            if abs_segs is None:
                continue
            target = resolve(abs_segs)
            if target and target != f:
                deps[f].add(target)

    # Tarjan SCC
    index_counter = [0]
    stack: list[Path] = []
    lowlink: dict[Path, int] = {}
    index: dict[Path, int] = {}
    on_stack: dict[Path, bool] = {}
    sccs: list[list[Path]] = []
    sys.setrecursionlimit(100000)

    def strongconnect(v: Path) -> None:
        index[v] = lowlink[v] = index_counter[0]
        index_counter[0] += 1
        stack.append(v)
        on_stack[v] = True
        for w in deps[v]:
            if w not in index:
                strongconnect(w)
                lowlink[v] = min(lowlink[v], lowlink[w])
            elif on_stack.get(w):
                lowlink[v] = min(lowlink[v], index[w])
        if lowlink[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                on_stack[w] = False
                comp.append(w)
                if w == v:
                    break
            sccs.append(comp)

    for f in files:
        if f not in index:
            strongconnect(f)

    scc_of: dict[Path, int] = {}
    for i, comp in enumerate(sccs):
        for f in comp:
            scc_of[f] = i

    # condensation edges + Kahn with priority
    scc_deps: dict[int, set[int]] = {i: set() for i in range(len(sccs))}
    for f in files:
        for d in deps[f]:
            if scc_of[f] != scc_of[d]:
                scc_deps[scc_of[f]].add(scc_of[d])

    def prio(f: Path) -> tuple:
        s = str(f)
        crate_rank = CRATE_ORDER.index(crate_of(f))
        is_test_dir = f.parts[2] == "tests"
        return (
            is_test_dir,  # integration tests last overall
            crate_rank,
            EXISTING_POS.get(s, 10_000),
            s,
        )

    scc_prio = {i: min(prio(f) for f in comp) for i, comp in enumerate(sccs)}
    indeg = {i: 0 for i in range(len(sccs))}
    rev: dict[int, set[int]] = {i: set() for i in range(len(sccs))}
    for i, ds in scc_deps.items():
        indeg[i] = len(ds)
        for d in ds:
            rev[d].add(i)

    heap = [(scc_prio[i], i) for i in range(len(sccs)) if indeg[i] == 0]
    heapq.heapify(heap)
    order: list[Path] = []
    cycle_groups: list[list[Path]] = []
    while heap:
        _, i = heapq.heappop(heap)
        comp = sorted(
            sccs[i],
            key=lambda f: (
                f.name == "tests.rs",  # test modules at the very end of a cycle
                f.name in ("mod.rs", "lib.rs", "main.rs"),  # entry files late in cycle
                len(deps[f] & set(sccs[i])),
                prio(f),
            ),
        )
        if len(comp) > 1:
            cycle_groups.append(comp)
        order.extend(comp)
        for j in rev[i]:
            indeg[j] -= 1
            if indeg[j] == 0:
                heapq.heappush(heap, (scc_prio[j], j))

    assert len(order) == len(files), f"{len(order)} != {len(files)}"

    pos = {f: i for i, f in enumerate(order)}
    violations = [
        (f, d)
        for f in files
        for d in deps[f]
        if pos[d] > pos[f] and scc_of[f] != scc_of[d]
    ]
    intra = [
        (f, d)
        for f in files
        for d in deps[f]
        if pos[d] > pos[f] and scc_of[f] == scc_of[d]
    ]
    print(f"# forward edges outside cycles: {len(violations)} (must be 0)")
    for f, d in violations:
        print(f"  VIOLATION: dependency of {f} comes later: {d}")
    print(f"# forward edges inside acknowledged cycles: {len(intra)}")
    for f, d in intra:
        print(f"  (cycle) {f} -> {d}")
    print("=== ORDER ===")
    for f in order:
        print(f)
    print()
    print("=== CYCLES (mutually dependent groups) ===")
    for comp in cycle_groups:
        print("  cycle: " + ", ".join(str(f) for f in comp))


main()
