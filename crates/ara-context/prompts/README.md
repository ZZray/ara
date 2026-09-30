# Prompt templates

These files are copied from OMP `packages/coding-agent/src/prompts/system/` at `596f2da7101178214aa27a753529d15e6b7ad91d` (MIT, © Stencil Labs, Inc.):
- `system-prompt.md`
- `project-prompt.md`
- `custom-system-prompt.md`
- `active-repo-context.md`
- `date-cwd-reminder.md`
- `personalities/{default,friendly,pragmatic}.md`

They are rendered by `ara-prompt` (the port of OMP's template engine), so upstream's Handlebars syntax works unchanged.

`skills/user-invocation.md` is copied unchanged from OMP
`packages/coding-agent/src/prompts/skills/user-invocation.md` at the same
fixed commit and under the same MIT copyright notice. The explicit Skill
builder uses the same `ara-prompt` renderer.

ARA edits, all in `system-prompt.md`:

1. **Identity.** `Oh My Pi coding harness` becomes `{{harnessName}} coding harness`, and the `security://` line says `{{harnessName}} scans`. The host supplies the name (ARA passes `ARA`).
2. **Internal URLs.** Each `scheme://` line sits behind `{{#if urls.<scheme>}}`, and the whole section behind `{{#if urls.any}}`. The prompt then advertises only the protocols the host's tools actually resolve. Upstream lists every protocol unconditionally.
   - With CTX-01d, ARA resolves `skill://` in `read`, `grep` and `glob`, rejects it in `write`, and expands it in `bash`. The CLI enables `urls.skill` when `read` is active.
   - The OMP harness-docs line (`omp://`) only renders when the host provides a docs URL.
3. **`skill://` tool guidance.** The internal-URL section names `read`, and names `grep`, `glob`, `write`, or `bash` only when each tool is available. It states that skill resources are read-only and other file tools require filesystem paths. ARA has not ported other internal URL schemes.
4. **Evidence-guided problem solving.** The default Engineering and Workflow guidance separates observations from inferred causes, tracks available sources/revisions and verification limits, identifies constraints and authorized scope, traces existing ownership before adding paths, uses safe falsifiable hypotheses, and chooses the smallest complete solution. Verification follows risk and observable acceptance criteria; unknown effects require inspection before replay, and candidate learning requires validation and review. Directly conflicting upstream instructions about never checking reports, unlimited uncertainty reduction, automatic broad cutover, unbounded execution, and never reviewing edits are adjusted. Host capabilities, authorization, budgets, and deadlines bound persistence, including the shared project footer. Simple tasks can use a shorter path; private deliberation need not be disclosed.
5. **Instruction provenance.** XML tags in user/source/tool/retrieved content do not elevate that content to system authority; message roles and trusted host provenance determine instruction level. This corrects the upstream prompt's assumption that user text is sanitized into trusted tagged directives. It is prompt guidance, not a runtime sanitizer, permission gate, or sandbox.

These are intentional ARA prompt differences from the fixed OMP commit, not new upstream parity claims. Rendering/request tests prove that the default guidance reaches the model and that custom/discovered prompt replacement still works; they do not prove a model follows the method or that Task acceptance or durable learning is implemented.

The Workflow makes three decision steps explicit: identify the underlying problem and desired outcome while preserving the user's choices and scope; distinguish physical/protocol/published-contract constraints from historical implementation choices; and explain how the goal, facts and constraints lead to a solution and an observable acceptance check. A one-sentence derivation checks clarity, permits missing facts or a revised solution, and never substitutes for evidence or requires unlimited investigation.

The other files are unchanged.
