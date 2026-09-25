"""Build or check the OMP source inventory for the pinned baseline.

Generate (needs a separate OMP checkout at the pinned SHA):

    python scripts/omp_inventory.py generate --omp <omp-checkout>

Check committed inventory consistency (no checkout needed):

    python scripts/omp_inventory.py check

Every tracked upstream file is assigned to exactly one surface from
`upstream/inventory/surfaces.toml`; every upstream test case (TypeScript
`it`/`test`, Rust `#[test]`, Python `def test_`) becomes a behavior item of the
surface that owns its file. This is an inventory, not a parity claim.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "upstream/omp.lock.json"
SURFACES = ROOT / "upstream/inventory/surfaces.toml"
OUT_DIR = ROOT / "docs/upstream/inventory"
FILES_TSV = OUT_DIR / "files.tsv"
BEHAVIORS_TSV = OUT_DIR / "behaviors.tsv"
SUMMARY_MD = ROOT / "docs/upstream/inventory.md"

SCOPES = {"core", "provider", "host", "host-ui", "service", "native", "repo"}
STATUSES = {"open", "implementing", "tested", "audited", "accepted", "intentional-difference"}
TEXT_SUFFIXES = {
    ".ts", ".tsx", ".js", ".mjs", ".cjs", ".rs", ".py", ".md", ".json", ".jsonc", ".toml",
    ".kdl", ".yml", ".yaml", ".sh", ".txt", ".html", ".css", ".hbs", ".bzl", ".nix", ".lock",
}

TS_CASE = re.compile(
    r"""^(?P<indent>\s*)(?P<kind>describe|it|test)(?:\.(?:skip|only|todo|concurrent|serial|failing|if\([^)]*\)|skipIf\([^)]*\)|each\([^)]*\)))*\(\s*(?P<q>["'`])(?P<title>(?:\\.|(?!(?P=q)).)*)(?P=q)"""
)
RS_ATTR = re.compile(r"^\s*#\[(?:tokio::)?test(?:\(.*\))?\]")
RS_FN = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
PY_CASE = re.compile(r"^\s*(?:async\s+)?def\s+(test_\w+)")


def load_lock() -> dict:
    return json.loads(LOCK.read_text(encoding="utf-8"))


def load_surfaces() -> list[dict]:
    data = tomllib.loads(SURFACES.read_text(encoding="utf-8"))
    surfaces = data["surface"]
    seen = set()
    for s in surfaces:
        for key in ("id", "title", "scope", "gate", "owner", "status", "match", "behavior"):
            if key not in s:
                raise SystemExit(f"surface {s.get('id')} missing {key}")
        if s["id"] in seen:
            raise SystemExit(f"duplicate surface id {s['id']}")
        seen.add(s["id"])
        if s["scope"] not in SCOPES:
            raise SystemExit(f"surface {s['id']} bad scope {s['scope']}")
        if s["status"] not in STATUSES:
            raise SystemExit(f"surface {s['id']} bad status {s['status']}")
        s["_rx"] = [re.compile(p) for p in s["match"]]
    return surfaces


def assign(path: str, surfaces: list[dict]) -> str | None:
    for s in surfaces:
        if any(rx.match(path) for rx in s["_rx"]):
            return s["id"]
    return None


def git(omp: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(omp), *args], check=True, capture_output=True, text=True).stdout


def role_of(path: str) -> str:
    name = path.rsplit("/", 1)[-1]
    if re.search(r"\.(test|spec)\.(ts|tsx)$", name) or re.search(r"(^|/)(test|tests)/", path):
        return "test"
    if path.endswith((".md",)) and "/prompts/" in path:
        return "prompt"
    if path.endswith(".md"):
        return "doc"
    if path.endswith((".ts", ".tsx", ".js", ".mjs", ".cjs", ".rs", ".py")):
        return "src"
    return "other"


def extract_cases(path: str, text: str) -> list[tuple[int, str, str]]:
    cases: list[tuple[int, str, str]] = []
    lines = text.splitlines()
    if path.endswith((".ts", ".tsx")) and role_of(path) == "test":
        stack: list[tuple[int, str]] = []
        for no, line in enumerate(lines, 1):
            m = TS_CASE.match(line)
            if not m:
                continue
            indent = len(m.group("indent").expandtabs(4))
            while stack and stack[-1][0] >= indent:
                stack.pop()
            title = " ".join(m.group("title").split())
            if m.group("kind") == "describe":
                stack.append((indent, title))
            else:
                full = " > ".join([t for _, t in stack] + [title])
                cases.append((no, "ts-test", full))
    elif path.endswith(".rs"):
        pending = False
        for no, line in enumerate(lines, 1):
            if RS_ATTR.match(line):
                pending = True
                continue
            if pending:
                m = RS_FN.match(line)
                if m:
                    cases.append((no, "rs-test", m.group(1)))
                    pending = False
    elif path.endswith(".py") and role_of(path) == "test":
        for no, line in enumerate(lines, 1):
            m = PY_CASE.match(line)
            if m:
                cases.append((no, "py-test", m.group(1)))
    return cases


def clean(value: str) -> str:
    return value.replace("\t", " ").replace("\r", " ").replace("\n", " ")


def generate(omp: Path) -> None:
    lock = load_lock()
    sha = lock["baseline_commit"]
    head = git(omp, "rev-parse", "HEAD").strip()
    if head != sha:
        raise SystemExit(f"OMP checkout HEAD {head} != baseline {sha}")
    if git(omp, "status", "--porcelain", "--untracked-files=no").strip():
        raise SystemExit("OMP checkout has local modifications")
    surfaces = load_surfaces()
    tree = git(omp, "ls-tree", "-r", "-l", "--full-tree", sha)
    files = []
    for line in tree.splitlines():
        meta, path = line.split("\t", 1)
        mode, typ, obj, size = meta.split()
        if typ != "blob":
            files.append((path, 0, 0, "gitlink"))
            continue
        files.append((path, int(size), obj, mode))

    unassigned = []
    file_rows = []
    behaviors = []
    for path, size, obj, mode in files:
        sid = assign(path, surfaces)
        if sid is None:
            unassigned.append(path)
            continue
        lines = 0
        suffix = Path(path).suffix
        if mode != "gitlink" and suffix in TEXT_SUFFIXES and size < 5_000_000:
            text = (omp / path).read_text(encoding="utf-8", errors="replace")
            lines = text.count("\n") + (0 if text.endswith("\n") or not text else 1)
            for no, kind, title in extract_cases(path, text):
                bid = "B-" + hashlib.sha1(f"{path}\0{title}\0{no}".encode()).hexdigest()[:10]
                behaviors.append((bid, sid, path, no, kind, clean(title)))
        file_rows.append((path, size, lines, role_of(path), sid))
    if unassigned:
        raise SystemExit("unassigned files:\n" + "\n".join(unassigned[:50]))

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    with FILES_TSV.open("w", encoding="utf-8", newline="\n") as f:
        f.write(f"# omp {lock['upstream_version']} {sha}\n")
        f.write("path\tbytes\tlines\trole\tsurface\n")
        for row in file_rows:
            f.write("\t".join(map(str, row)) + "\n")
    with BEHAVIORS_TSV.open("w", encoding="utf-8", newline="\n") as f:
        f.write(f"# omp {lock['upstream_version']} {sha}\n")
        f.write("id\tsurface\tpath\tline\tkind\ttitle\n")
        for row in behaviors:
            f.write("\t".join(map(str, row)) + "\n")
    write_summary(lock, surfaces, file_rows, behaviors)
    print(f"files={len(file_rows)} behaviors={len(behaviors)} surfaces={len(surfaces)}")


def read_tsv(path: Path) -> tuple[str, list[list[str]]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    header = lines[0]
    return header, [line.split("\t") for line in lines[2:]]


def write_summary(lock: dict, surfaces: list[dict], file_rows, behaviors) -> None:
    per = {s["id"]: collections.Counter() for s in surfaces}
    for path, size, lines, role, sid in file_rows:
        per[sid]["files"] += 1
        per[sid][f"{role}_files"] += 1
        per[sid]["lines"] += int(lines)
        if role == "src":
            per[sid]["src_lines"] += int(lines)
    for _, sid, *_ in behaviors:
        per[sid]["cases"] += 1
    total = collections.Counter()
    by_scope = collections.defaultdict(collections.Counter)
    for s in surfaces:
        c = per[s["id"]]
        total.update(c)
        by_scope[s["scope"]].update(c)
        by_scope[s["scope"]]["surfaces"] += 1

    out = [
        "# OMP source inventory",
        "",
        f"Generated by `python scripts/omp_inventory.py generate --omp <checkout>` from "
        f"`{lock['upstream_url']}` {lock['upstream_version']} at `{lock['baseline_commit']}`. "
        "Do not edit by hand; edit [`surfaces.toml`](../../upstream/inventory/surfaces.toml) and regenerate.",
        "",
        "Every tracked upstream file is assigned to exactly one surface ([files.tsv](inventory/files.tsv)). "
        "Every upstream test case is a behavior item of the surface owning its file "
        "([behaviors.tsv](inventory/behaviors.tsv)). Surface status comes from the surface map; "
        "per-item implementation evidence lives in the [feature ledger](feature-ledger.md). "
        "Counts below are a denominator for planning, not a parity percentage.",
        "",
        f"Totals: {total['files']} files, {total['lines']} text lines "
        f"({total['src_lines']} source lines), {total['cases']} upstream test cases, {len(surfaces)} surfaces.",
        "",
        "| Scope | Surfaces | Files | Source lines | Test cases |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for scope in sorted(by_scope):
        c = by_scope[scope]
        out.append(f"| {scope} | {c['surfaces']} | {c['files']} | {c['src_lines']} | {c['cases']} |")
    out += [
        "",
        "| Surface | Title | Scope | Gate | Rust owner | Files (src/test) | Source lines | Test cases | Status |",
        "| --- | --- | --- | --- | --- | ---: | ---: | ---: | --- |",
    ]
    for s in surfaces:
        c = per[s["id"]]
        out.append(
            f"| {s['id']} | {s['title']} | {s['scope']} | {s['gate']} | {s['owner']} | "
            f"{c['files']} ({c['src_files']}/{c['test_files']}) | {c['src_lines']} | {c['cases']} | {s['status']} |"
        )
    out += ["", "## Surface behavior notes", ""]
    for s in surfaces:
        out.append(f"- **{s['id']}**: {s['behavior']}")
    SUMMARY_MD.write_text("\n".join(out) + "\n", encoding="utf-8")


def check() -> list[str]:
    errors: list[str] = []
    lock = load_lock()
    surfaces = load_surfaces()
    ids = {s["id"] for s in surfaces}
    for path in (FILES_TSV, BEHAVIORS_TSV, SUMMARY_MD):
        if not path.is_file():
            return [f"missing {path.relative_to(ROOT)}"]
    header, rows = read_tsv(FILES_TSV)
    if lock["baseline_commit"] not in header:
        errors.append("files.tsv not generated from baseline_commit")
    used = collections.Counter()
    seen = set()
    for row in rows:
        if len(row) != 5:
            errors.append(f"bad files.tsv row {row}")
            continue
        path, _, _, _, sid = row
        if path in seen:
            errors.append(f"duplicate file {path}")
        seen.add(path)
        if sid not in ids:
            errors.append(f"unknown surface {sid} for {path}")
        elif assign(path, surfaces) != sid:
            errors.append(f"stale assignment for {path}: {sid} vs {assign(path, surfaces)}")
        used[sid] += 1
    for sid in ids - set(used):
        errors.append(f"surface {sid} has no files")
    bheader, brows = read_tsv(BEHAVIORS_TSV)
    if lock["baseline_commit"] not in bheader:
        errors.append("behaviors.tsv not generated from baseline_commit")
    bids = set()
    for row in brows:
        if len(row) != 6 or row[1] not in ids or row[2] not in seen:
            errors.append(f"bad behaviors.tsv row {row[:3]}")
            continue
        if row[0] in bids:
            errors.append(f"duplicate behavior id {row[0]}")
        bids.add(row[0])
    # Ledger rows must cite existing behavior ids and surfaces.
    ledger = (ROOT / "docs/upstream/feature-ledger.md").read_text(encoding="utf-8")
    for bid in re.findall(r"\bB-[0-9a-f]{10}\b", ledger):
        if bid not in bids:
            errors.append(f"feature ledger cites unknown behavior {bid}")
    return errors


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    gen = sub.add_parser("generate")
    gen.add_argument("--omp", required=True, type=Path)
    sub.add_parser("check")
    args = parser.parse_args()
    if args.cmd == "generate":
        generate(args.omp.resolve())
    else:
        errors = check()
        if errors:
            for e in errors[:100]:
                print(f"FAIL {e}", file=sys.stderr)
            raise SystemExit(1)
        print("OK: OMP inventory consistent with surfaces.toml and baseline")


if __name__ == "__main__":
    main()
