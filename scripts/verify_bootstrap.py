"""Check the documentation bootstrap; this is not an Agent behavior test."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[1]
BASELINE = "596f2da7101178214aa27a753529d15e6b7ad91d"
REQUIRED = (
    "README.md",
    "AGENTS.md",
    ".env.example",
    "upstream/omp.lock.json",
    "THIRD_PARTY_NOTICES.md",
    "docs/knowledge/INDEX.md",
    "docs/knowledge/architecture.md",
    "docs/knowledge/upstream.md",
    "docs/knowledge/context.md",
    "docs/knowledge/integrations.md",
    "docs/knowledge/verification.md",
    "docs/roadmap.md",
    "docs/upstream-sync.md",
    "docs/upstream/feature-ledger.md",
    "docs/upstream/changes.md",
    "docs/acceptance.md",
    "docs/audit.md",
    "docs/testing-providers.md",
    "docs/evidence/README.md",
    "docs/handoff-prompt.md",
    ".ara/skills/INDEX.md",
    ".ara/skills/project-knowledge-maintenance/SKILL.md",
    ".ara/skills/omp-commit-sync/SKILL.md",
    ".ara/skills/point-delivery-audit/SKILL.md",
)
LINK = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
SHA = re.compile(r"[0-9a-f]{40}\Z")


def check() -> list[str]:
    errors: list[str] = []
    for relative in REQUIRED:
        if not (ROOT / relative).is_file():
            errors.append(f"missing: {relative}")

    lock_path = ROOT / "upstream/omp.lock.json"
    if lock_path.is_file():
        try:
            lock = json.loads(lock_path.read_text(encoding="utf-8"))
            if lock.get("baseline_commit") != BASELINE:
                errors.append("baseline_commit changed")
            if lock.get("upstream_version") != "v18.1.8":
                errors.append("upstream_version changed")
            for key in ("reviewed_upstream_commit", "ported_through_commit"):
                value = lock.get(key)
                if value is not None and not (isinstance(value, str) and SHA.fullmatch(value)):
                    errors.append(f"invalid {key}")
        except (OSError, ValueError) as exc:
            errors.append(f"invalid upstream lock: {exc}")

    env_path = ROOT / ".env.example"
    if env_path.is_file():
        for line_no, line in enumerate(env_path.read_text(encoding="utf-8").splitlines(), 1):
            if line.startswith(("OPENROUTER_API_KEY=", "ARA_TEST_API_KEY=")) and line.partition("=")[2].strip():
                errors.append(f"populated key placeholder: .env.example:{line_no}")

    for page in ROOT.rglob("*.md"):
        if ".git" in page.parts:
            continue
        for target in LINK.findall(page.read_text(encoding="utf-8")):
            target = target.split("#", 1)[0].split("?", 1)[0]
            if not target or target.startswith(("https://", "http://", "mailto:")):
                continue
            path = (page.parent / unquote(target)).resolve()
            if not path.is_relative_to(ROOT) or not path.exists():
                errors.append(f"broken local link: {page.relative_to(ROOT)} -> {target}")

    for skill in (ROOT / ".ara/skills").glob("*/SKILL.md"):
        body = skill.read_text(encoding="utf-8")
        if not re.match(r"\A---\nname: [a-z0-9-]+\ndescription: .+\n---\n", body):
            errors.append(f"invalid Skill frontmatter: {skill.relative_to(ROOT)}")

    return errors


if __name__ == "__main__":
    failures = check()
    if failures:
        for failure in failures:
            print(f"FAIL {failure}", file=sys.stderr)
        raise SystemExit(1)
    print(f"OK: {len(REQUIRED)} required files, OMP marker, key placeholders, local links, Skills")
