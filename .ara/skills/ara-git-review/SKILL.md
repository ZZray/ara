---
name: ara-git-review
description: Read-only review of a specified ARA Git diff, commit, or files with exact target-version coverage and actionable defect evidence.
---

# Review the selected code

1. Read `AGENTS.md` and the relevant requirement. Resolve the user's target: staged, unstaged, worktree, commit, branch range, or named files. Record repository, base/target revisions, status, exclusions, and changed `(path, status)` entries. Never substitute the current worktree for staged or committed content.
2. If `ocr` is available and the target is a Git change, `ocr delegate preview --format json` can select files and rules without calling another model. Use the corresponding `--commit` or `--from/--to` form and `ocr delegate rule --format json <paths>`; record exclusions and tool errors. If unavailable, use Git directly. Do not install tools, change OCR configuration, or silently widen the target during a read-only review.
3. Read each target version, the necessary callers/contracts, relevant tests, and fixed OMP source where behavior is being ported. Report only reachable defects introduced, exposed, or worsened by the target. A lint hit, style preference, or absent comment is not by itself a defect.
4. For each finding give severity, target-version path/line, triggering input or state, actual consequence, supporting code or executed evidence, and a small correction. Keep unconfirmed risks separate. Continue coverage after the first finding or state which entries were skipped and why.
5. Report `reviewed`, `skipped`, and `scope-excluded` entries; zero reviewable files is `N/A`, not 100% coverage. State checks actually run. This Skill is read-only and does not claim compilation, runtime testing, or acceptance. Use `ara-rust-core-review` or `ara-provider-review` when those areas changed.
