//! Shared narrow extraction from fixed OMP tools/approval.ts, latest reviewed D owner.
//! MIT: Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Policy identity and Graph observations are shared with Extension, not duplicated.
use ara_agent::{ActualProviderError, SessionValues};
use ara_rpc::{
    WireString,
    value_heap::{ValueHandle, ValueRuntime, ValueSnapshot},
};
type Result<T> = std::result::Result<T, ActualProviderError>;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApprovalPolicy {
    Allow,
    Deny,
    Prompt,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolTier {
    Read,
    Write,
    Exec,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApprovalSource {
    Tool,
    User,
    Mode,
}
#[derive(Clone, Debug)]
pub struct ResolvedApproval {
    pub policy: ApprovalPolicy,
    pub tier: ToolTier,
    pub reason: Option<WireString>,
    pub override_prompt: bool,
    pub source: ApprovalSource,
    pub policy_key: Option<WireString>,
}
fn text(value: &ValueHandle) -> Result<Option<WireString>> {
    Ok(match value.heap().snapshot(value)? {
        ValueSnapshot::String(value) => Some(value),
        _ => None,
    })
}
fn normalize_policy(value: &ValueHandle) -> Result<Option<ApprovalPolicy>> {
    let Some(text) = text(value)? else {
        return Ok(None);
    };
    let Ok(text) = text.to_utf8() else {
        return Ok(None);
    };
    Ok(match ara_prompt::js::trim(&text).to_lowercase().as_str() {
        "allow" => Some(ApprovalPolicy::Allow),
        "deny" => Some(ApprovalPolicy::Deny),
        "prompt" => Some(ApprovalPolicy::Prompt),
        _ => None,
    })
}
fn tier(value: &ValueHandle) -> Result<Option<ToolTier>> {
    Ok(
        match text(value)?
            .as_ref()
            .and_then(|t| t.to_utf8().ok())
            .as_deref()
        {
            Some("read") => Some(ToolTier::Read),
            Some("write") => Some(ToolTier::Write),
            Some("exec") => Some(ToolTier::Exec),
            _ => None,
        },
    )
}
struct Decision {
    tier: ToolTier,
    policy: Option<ApprovalPolicy>,
    reason: Option<WireString>,
    override_prompt: bool,
    policy_key: Option<WireString>,
}
fn decision(
    tool: &ValueHandle,
    args: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<Decision> {
    let heap = tool.heap();
    let declared = heap.get(tool, &"approval".into(), runtime)?;
    let value = if matches!(
        heap.snapshot(&declared)?,
        ValueSnapshot::Function(_)
            | ValueSnapshot::Opaque {
                kind: ara_rpc::value_heap::OpaqueKind::Function,
                ..
            }
    ) {
        heap.call(&declared, &heap.undefined()?, &[args.clone()], runtime)?
    } else {
        declared
    };
    if let Some(tier) = tier(&value)? {
        return Ok(Decision {
            tier,
            policy: None,
            reason: None,
            override_prompt: false,
            policy_key: None,
        });
    }
    let mut result = Decision {
        tier: ToolTier::Exec,
        policy: None,
        reason: None,
        override_prompt: false,
        policy_key: None,
    };
    // Fixed typeof object accepts boxed objects, while excluding null/arrays.
    if matches!(
        heap.snapshot(&value)?,
        ValueSnapshot::Object
            | ValueSnapshot::Opaque {
                kind: ara_rpc::value_heap::OpaqueKind::Object
                    | ara_rpc::value_heap::OpaqueKind::BoxedNumber
                    | ara_rpc::value_heap::OpaqueKind::BoxedString
                    | ara_rpc::value_heap::OpaqueKind::BoxedBoolean
                    | ara_rpc::value_heap::OpaqueKind::BoxedBigInt,
                ..
            }
    ) {
        result.tier = tier(&heap.get(&value, &"tier".into(), runtime)?)?.unwrap_or(ToolTier::Exec);
        result.reason =
            text(&heap.get(&value, &"reason".into(), runtime)?)?.filter(|v| !v.is_empty());
        result.policy = normalize_policy(&heap.get(&value, &"policy".into(), runtime)?)?;
        result.policy_key =
            text(&heap.get(&value, &"policyKey".into(), runtime)?)?.filter(|v| !v.is_empty());
        result.override_prompt = matches!(
            heap.snapshot(&heap.get(&value, &"override".into(), runtime)?)?,
            ValueSnapshot::Boolean(true)
        );
    }
    Ok(result)
}
fn own_policy(
    record: &ValueHandle,
    key: &WireString,
    runtime: &dyn ValueRuntime,
) -> Result<Option<ApprovalPolicy>> {
    if record
        .heap()
        .own_descriptor(record, &key.clone().into(), runtime)?
        .is_none()
    {
        return Ok(None);
    }
    normalize_policy(&record.heap().get(record, &key.clone().into(), runtime)?)
}
/// Reads the original declaration/arguments and current user policy via exact
/// graph Get/Call sites. No serde conversion or schema snapshot is performed.
pub fn resolve_approval(
    tool: &ValueHandle,
    tool_name: &WireString,
    args: &ValueHandle,
    mode: &ValueHandle,
    user_config: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ResolvedApproval> {
    let decision = decision(tool, args, runtime)?;
    let policy_key = decision.policy_key.as_ref().unwrap_or(tool_name);
    let user = own_policy(user_config, policy_key, runtime)?;
    let fallback = if policy_key != tool_name && user.is_none() {
        own_policy(user_config, tool_name, runtime)?
    } else {
        None
    };
    let effective_user = user.or(fallback);
    let user_key = if user.is_some() {
        policy_key.clone()
    } else {
        tool_name.clone()
    };
    let make = |policy, override_prompt, source, policy_key, reason| ResolvedApproval {
        policy,
        tier: decision.tier,
        reason,
        override_prompt,
        source,
        policy_key,
    };
    if decision.policy == Some(ApprovalPolicy::Deny) {
        return Ok(make(
            ApprovalPolicy::Deny,
            decision.override_prompt,
            ApprovalSource::Tool,
            decision.policy_key,
            decision.reason,
        ));
    }
    if effective_user == Some(ApprovalPolicy::Deny) {
        return Ok(make(
            ApprovalPolicy::Deny,
            decision.override_prompt,
            ApprovalSource::User,
            Some(user_key),
            None,
        ));
    }
    if matches!(mode.heap().snapshot(mode)?,ValueSnapshot::String(value) if value.equals_ascii("yolo"))
    {
        if let Some(policy) = decision.policy {
            return Ok(make(
                policy,
                false,
                ApprovalSource::Tool,
                decision.policy_key,
                decision.reason,
            ));
        }
        return Ok(make(
            effective_user.unwrap_or(ApprovalPolicy::Allow),
            false,
            if effective_user.is_some() {
                ApprovalSource::User
            } else {
                ApprovalSource::Mode
            },
            effective_user.map(|_| user_key),
            None,
        ));
    }
    if decision.override_prompt {
        return Ok(make(
            if decision.policy == Some(ApprovalPolicy::Allow) {
                ApprovalPolicy::Allow
            } else {
                ApprovalPolicy::Prompt
            },
            true,
            ApprovalSource::Tool,
            decision.policy_key,
            decision.reason,
        ));
    }
    if let Some(policy @ (ApprovalPolicy::Allow | ApprovalPolicy::Prompt)) = decision.policy {
        return Ok(make(
            policy,
            false,
            ApprovalSource::Tool,
            decision.policy_key,
            decision.reason,
        ));
    }
    if let Some(policy) = effective_user {
        return Ok(make(
            policy,
            false,
            ApprovalSource::User,
            Some(user_key),
            None,
        ));
    }
    let allowed = mode_approves_tier(mode, decision.tier, runtime)?;
    Ok(make(
        if allowed {
            ApprovalPolicy::Allow
        } else {
            ApprovalPolicy::Prompt
        },
        false,
        ApprovalSource::Mode,
        None,
        if allowed { None } else { decision.reason },
    ))
}
pub fn resolve_tool_tier(
    tool: &ValueHandle,
    args: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ToolTier> {
    Ok(decision(tool, args, runtime)?.tier)
}
pub fn denial_message(resolved: &ResolvedApproval, name: &WireString) -> WireString {
    if resolved.source == ApprovalSource::Tool {
        let mut parts = vec![
            "Tool \"".into(),
            name.clone(),
            "\" is blocked by tool policy.".into(),
        ];
        if let Some(reason) = &resolved.reason {
            parts.extend(["\nReason: ".into(), reason.clone()]);
        }
        concat(&parts)
    } else {
        let key = resolved.policy_key.as_ref().unwrap_or(name);
        concat(&[
            "Tool \"".into(),
            key.clone(),
            "\" is blocked by user policy.\nTo allow: remove \"tools.approval.".into(),
            key.clone(),
            ": deny\" from config.".into(),
        ])
    }
}
fn concat(parts: &[WireString]) -> WireString {
    WireString::from_units(
        parts
            .iter()
            .flat_map(|part| part.units().iter().copied())
            .collect(),
    )
}
const POLICY_MODULE: &str = "ara-cli.fixed-approval-policy";
static MODE_TIER: ara_rpc::value_heap::CompiledFunctionSpec =
    ara_rpc::value_heap::CompiledFunctionSpec {
        source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:tools/approval.ts:100-102",
        name: "modeApprovesTier",
        length: 2,
        form: ara_rpc::value_heap::CompiledFunctionForm::Ordinary,
        capture_count: 0,
        body: |heap, _, _, arguments, runtime| {
            let function =
                heap.module_compiled_function(POLICY_MODULE, "modeApprovesTier", &MODE_TIER)?;
            let ranks = heap.get(
                &function,
                &heap.module_symbol(POLICY_MODULE, "TIER_RANK")?,
                runtime,
            )?;
            let modes = heap.get(
                &function,
                &heap.module_symbol(POLICY_MODULE, "APPROVAL_MODE_MAX_TIER")?,
                runtime,
            )?;
            let key = |value: &ValueHandle| -> std::result::Result<
                ara_rpc::value_heap::PropertyKey,
                ara_rpc::value_heap::ValueError,
            > {
                let primitive = heap.to_primitive(value, true, runtime)?;
                if matches!(heap.snapshot(&primitive)?, ValueSnapshot::Symbol { .. }) {
                    Ok(ara_rpc::value_heap::PropertyKey::Symbol(
                        heap.symbol_key(&primitive)?,
                    ))
                } else {
                    Ok(heap.to_string(&primitive, runtime)?.into())
                }
            };
            let mode = arguments.first().cloned().unwrap_or(heap.undefined()?);
            let tier = arguments.get(1).cloned().unwrap_or(heap.undefined()?);
            let left = heap.get(&ranks, &key(&tier)?, runtime)?;
            let maximum = heap.get(&modes, &key(&mode)?, runtime)?;
            let right = heap.get(&ranks, &key(&maximum)?, runtime)?;
            let left = heap.to_number(&left, runtime)?;
            let right = heap.to_primitive(&right, false, runtime)?;
            // The left rank is the original private table's finite 0/1/2. A
            // prototype-supplied BigInt on the right participates in JS comparison.
            let right = if let ValueSnapshot::BigInt(integer) = heap.snapshot(&right)? {
                integer.as_str().parse::<f64>().map_err(|_| {
                    ara_rpc::value_heap::ValueError::TypeError(
                        "Approval tier rank BigInt comparison failed".into(),
                    )
                })?
            } else {
                heap.to_number(&right, runtime)?
            };
            heap.boolean(left <= right)
        },
    };
/// Prepare fixed module-private ordinary tables before extension import.
/// The native module function retains these original same-realm constants.
pub fn prepare_approval_policy(values: &SessionValues) -> Result<()> {
    let heap = values.heap();
    let runtime = values.runtime();
    let function = heap.module_compiled_function(POLICY_MODULE, "modeApprovesTier", &MODE_TIER)?;
    let ranks_key = heap.module_symbol(POLICY_MODULE, "TIER_RANK")?;
    let modes_key = heap.module_symbol(POLICY_MODULE, "APPROVAL_MODE_MAX_TIER")?;
    if heap
        .own_descriptor(&function, &ranks_key, runtime)?
        .is_some()
        && heap
            .own_descriptor(&function, &modes_key, runtime)?
            .is_some()
    {
        return Ok(());
    }
    let ranks = heap.object()?;
    let modes = heap.object()?;
    for (key, rank) in [("read", 0.0), ("write", 1.0), ("exec", 2.0)] {
        heap.define_own_data(&ranks, key.into(), heap.number(rank)?, runtime)?;
    }
    for (key, tier) in [("always-ask", "read"), ("write", "write"), ("yolo", "exec")] {
        heap.define_own_data(&modes, key.into(), heap.string(tier.into())?, runtime)?;
    }
    heap.define_own_data(&function, ranks_key, ranks, runtime)?;
    heap.define_own_data(&function, modes_key, modes, runtime)?;
    Ok(())
}
fn mode_approves_tier(
    mode: &ValueHandle,
    tier: ToolTier,
    runtime: &dyn ValueRuntime,
) -> Result<bool> {
    let heap = mode.heap();
    let function = heap.module_compiled_function(POLICY_MODULE, "modeApprovesTier", &MODE_TIER)?;
    let tier = match tier {
        ToolTier::Read => "read",
        ToolTier::Write => "write",
        ToolTier::Exec => "exec",
    };
    Ok(heap.truthy(&heap.call(
        &function,
        &heap.undefined()?,
        &[mode.clone(), heap.string(tier.into())?],
        runtime,
    )?)?)
}
