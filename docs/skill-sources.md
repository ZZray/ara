# Skill source switches

ARA's reference CLI discovers Agents, Claude and Codex Skills by default;
OpenCode Skills are off. Each compatible-source switch covers both user and
project locations. Native ARA, GitHub and configured custom sources keep their
existing controls.

| Source | User location | Project location | ARA default |
| --- | --- | --- | --- |
| Agents | `~/.agents/skills`, `~/.agent/skills` | `.agents/skills`, `.agent/skills` | on |
| Claude | `~/.claude/skills` or host `CLAUDE_CONFIG_DIR` override | `.claude/skills` | on |
| Codex | `~/.codex/skills` | `.codex/skills` | on |
| OpenCode | `~/.config/opencode/skills` | `.opencode/skills` | off |

These are the existing fixed-source loaders, including their ancestor traversal
and host-supplied extra homes. OpenCode's user location preserves the pinned
OMP path; `~/.opencode/skills` is not added as a new alias.

## CLI

`--skill-sources` supplies the complete enabled set of these four sources:

```sh
ara --skill-sources agents,claude,codex,opencode -p "Use the available Skills"
ara --skill-sources codex --skills "*-project" -p "Use a project Skill"
ara --skill-sources "" -p "Use the remaining Skills"
ara --no-skills -p "Run without Skills"
```

The empty selection disables only the four compatible sources; native and
other sources may still load. `--no-skills` disables all Skill discovery.
Whitespace and empty comma-separated entries are ignored; unknown source IDs
are usage errors (exit 2), before a model request or Session is created.

Print, REPL and RPC initialization use the same selection for prompt listings,
tool `skill://` mappings and REPL Skill dispatch. This is host startup
configuration, not a live RPC settings command.

## Host SDK and persistence

The host can serialize `ara_discovery::SkillSourceSwitches` as JSON, persist it
at its own location, and bind its four booleans to UI controls:

```json
{"agents":true,"claude":true,"codex":true,"opencode":false}
```

Missing JSON fields use these ARA defaults; unknown fields and non-boolean
values are rejected. The host applies the saved configuration when assembling
the next discovery/configuration snapshot:

```rust
use ara_discovery::{SkillSourceSwitches, SkillsSettings};

let settings = SkillsSettings {
    source_switches: Some(SkillSourceSwitches::default()),
    ..SkillsSettings::default()
};
// discovery.load_skills(cwd, &settings)
```

`SkillsSettings::default().source_switches` is **None**, preserving fixed OMP's
legacy per-level defaults for existing SDK callers. `Some(switches)` overrides
the corresponding legacy flags for both levels and opts enabled foreign user
sources in. An off source is excluded before its loader scans any directory,
even when legacy opt-ins, wildcard user opt-ins or a Claude directory override
are present. `ProviderPolicy.disabled` still vetoes an enabled source.

The switches are scoped to Skill discovery. Context files, `SYSTEM.md`, provider
policy, source priority and deduplication retain their existing behavior.
ARA's Core supplies the configuration; persistence and UI belong to the host.

See [executed evidence](evidence/skill-source-switches.md).
