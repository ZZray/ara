# Audit procedure

Use the [point delivery audit Skill](../.ara/skills/point-delivery-audit/SKILL.md) for each implementation point. Audit the change that will actually be delivered. Start with `git status`, the target commit or staged diff, its requirement-to-file mapping, and the relevant OMP source or ARA decision. Review every changed file and critical hunk for ownership, state transitions, permissions, cancellation, unknown usage, credential exposure, persistent format, and compatibility with the shared Core boundary.

Use [Git change review](../.ara/skills/ara-git-review/SKILL.md) for exact scope and coverage, then [Rust Agent Core review](../.ara/skills/ara-rust-core-review/SKILL.md) or [Provider wire review](../.ara/skills/ara-provider-review/SKILL.md) for the changed domain. Available independent tools (for example local `ocr` or a read-only reviewer) can add scope/rule coverage; inspect actual test output as well. The bootstrap does not run a reviewer automatically. Record which tool and version actually ran, or mark the missing independent review as a gap. A reviewer result is evidence, not permission to widen scope or claim runtime success. For security-sensitive or concurrent code, get an independent second view.

Review record:

```text
Requirement and tested commit:
Changed files/hunks and why each is necessary:
Independent review command/result:
Semantic findings (trigger, impact, source location, evidence):
Tests actually run and results:
Missing/blocked evidence:
Decision: accept this point / request changes / not tested
```

Do not force a defect from style preference or an untriggered hypothesis. Do not accept a point with a known failing test, missing mandatory real task evidence, or changed source after the recorded test. Task acceptance is separate from a successful Run and from a Git commit.
