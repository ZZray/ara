# Reviewed experience: the first self-evolution slice

2026-10-15. This is a HandWave Node Host capability, separate from the root Shared
Core evolution plan and fixed OMP parity. Its implementation and actual execution
are recorded in [the delivery evidence](../evidence/evolution-20261015.md).

## Goal, ownership and promotion

The Agent can turn a real failed tool and subsequent recovery into a conditional
lesson candidate, then retrieve it after explicit user adoption in a later Run.
This is reviewed experience reuse. It is not model-weight training, unattended
code rewriting, automatic Skill installation or a universal reasoning upgrade.
Native Session/controller/cwd identity remains the existing storage boundary;
experiences do not automatically cross Web Sessions, devices or cloud projects.

Reuse the existing Memory candidate/accepted/revoked state and provenance chain.
`Memory.experience` is optional with a serde default, so old sidecars remain
loadable. There is no second receipt ledger or background model worker. The
original native transcript remains the audit source; original tool/reference
records and the 256-message/16 MiB checkpoint limit are unchanged.

## Observe and propose

`experience_propose` accepts lesson (up to 2048 UTF-8 bytes), conditions (1024),
active source IDs, and 2..8 actual tool-call IDs. At least one source must be an
active objective/steering record from the current Run. The host resolves pairs
against only the current Run's native transcript, in result order. It rejects
missing, duplicated, mismatched, unsupported or cross-Run calls. Selected failed
receipts must precede selected successful ones. Reference/memory/experience
tools cannot be used as recovery evidence. At most one proposal is allowed per
Run, including after revocation; regular sourced memory proposals retain their
separate existing contract.

The snapshot gate checks cancellation, checkpoint failure and context failure
before and after cloning the native messages. It does not mistake Emitter's
in-memory append after a failed save for durable evidence. The current proposal
call is still unresolved, so only selected completed historical pairs are
validated; the full active transcript is not required to be closed prematurely.

Opaque protocol IDs proved unreliable for real-model copying. After a real
failure/recovery, the Host therefore exposes a provider-only short directory of
the latest actual failed receipt and subsequent latest successful receipt. It
contains only exact IDs/names/error flags, at most 2 KiB, and no output body.
It is neither persisted nor reused across Runs and does not add a model/tool
call. If inserting this optional metadata would exceed the context byte budget,
it is omitted without displacing original user/tool evidence. The Agent must
copy actual directory IDs; `collect` remains strict and never accepts guesses.

Evidence copies run, tool IDs/names, actual error flags, full text-output SHA256
and at most 1024 bytes per UTF-8 excerpt. Output is already grant-redacted by the
Host; hashes describe those canonical redacted text blocks joined by newline.
A truncated excerpt cannot validate its full-output hash by itself. This is
provenance and tool-return evidence: a success exit does not establish that the
Task passed or that the proposed causal explanation generalizes.

Proposals use clone → validate → sidecar save → replace. Cancellation observed at the pre-commit gate or a failed save cannot replace the active Store with a partial proposal.
Ordinary OS-user file authority remains; the ledger is not tamper-proof against
that same account. Existing sidecar temporary-file cleanup behavior is unchanged.

## Review, retrieve, correct

The controller can inspect candidates with `/memory list`, explicitly adopt
`/memory accept mem-ID`, and revoke with `/memory revoke mem-ID`. Only exact real
controller input performs those operations. Tool output, retrieved text, model
prose or a brief cannot issue an adoption command. The existing user command
records the adoption receipt without a model request.

Candidates are excluded from experience retrieval and accepted working views.
Their proposal arguments are hidden from projected provider history and summary
input; raw audit remains. This does not erase an assistant's separate discussion
of a candidate from ordinary conversation or convert that discussion into truth.

`experience_search` retrieves only explicitly accepted experiences with current
source revisions and cloud stamp. It performs Unicode lowercase literal matching
on lesson and conditions, returns at most 3, and measures the complete JSON
response against 8192 bytes. `output_limited` exposes a truncated result budget.
The default working view carries only the count/retrieval hint, avoiding repeated
injection of lesson text and copied evidence. Recall remains a scoped suggestion;
the Agent must inspect whether conditions match the current environment.

Source correction/revocation or memory revocation uses the same existing
projection watermark to invalidate derived context. Restart retains candidates
without promotion. No model success or test fixture can silently adopt a lesson.

## Cost and next gates

The foreground model may propose once after actual recovery. This consumes the
foreground call allowance and can require an extra tool round. There is no
background reflection call, and no unrelated verification command should be
executed solely to manufacture learning evidence. The ID directory itself adds
no model or tool call.
The retrieval ceiling and conditional policy bound context growth; no measured
Token saving or general speedup is claimed. Single-task timings and unknown
supplier usage are kept in evidence.

Future slices need their own reviewed gates: a visible local-candidate editor,
explicit cross-Session/project promotion through the existing cloud ownership
contract, retention of environment applicability and negative feedback, and
repeat-task evaluation before promoting a recipe into a Skill or regression
test. None of these is automatically granted by this candidate tool.
