# Prompt templates

These files are copied from OMP `packages/coding-agent/src/prompts/system/` at `596f2da7101178214aa27a753529d15e6b7ad91d` (MIT, © Stencil Labs, Inc.):
- `system-prompt.md`
- `project-prompt.md`
- `custom-system-prompt.md`
- `active-repo-context.md`
- `date-cwd-reminder.md`
- `personalities/{default,friendly,pragmatic}.md`

They are rendered by `ara-prompt` (the port of OMP's template engine), so upstream's Handlebars syntax works unchanged.

ARA edits, all in `system-prompt.md`:

1. **Identity.** `Oh My Pi coding harness` becomes `{{harnessName}} coding harness`, and the `security://` line says `{{harnessName}} scans`. The host supplies the name (ARA passes `ARA`).
2. **Internal URLs.** Each `scheme://` line sits behind `{{#if urls.<scheme>}}`, and the whole section behind `{{#if urls.any}}`. The prompt then advertises only the protocols the host's tools actually resolve. Upstream lists every protocol unconditionally.
   - With CTX-01d, ARA resolves `skill://` in `read` and expands it in `bash`. The CLI enables `urls.skill` when `read` is active.
   - The OMP harness-docs line (`omp://`) only renders when the host provides a docs URL.

The other files are unchanged.
