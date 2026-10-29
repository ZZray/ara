//! Host-owned snapshot of fixed OMP `utils/schema/adapt.ts::NO_STRICT`.
//!
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d (MIT),
//! `packages/utils/src/env.ts` and `packages/ai/src/utils/schema/adapt.ts`.
//! ARA names the flag `ARA_NO_STRICT`; its exact truthy spellings and one-time
//! startup sampling follow the source. Portable AI receives only the bool.

use crate::daily_model_config::DailyEnvironment;
use crate::model_config_values::{ConfigValueEnvironment, ProcessConfigEnvironment};
use std::sync::OnceLock;

pub const NO_STRICT_ENV: &str = "ARA_NO_STRICT";

/// Fixed `parseFlag(value)` with its default false; no trim or case folding.
pub fn parse_no_strict(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "Y" | "y" | "TRUE" | "true" | "YES" | "yes" | "ON" | "on"))
}

/// Immutable policy for one Host startup, retaining the original environment
/// spelling so existing Daily environment closures can forward the snapshot.
#[derive(Clone)]
pub struct SchemaPolicySnapshot {
    raw_no_strict: Option<String>,
    no_strict: bool,
}

impl SchemaPolicySnapshot {
    pub fn capture(env: &dyn DailyEnvironment) -> Self {
        let raw_no_strict = env.get(NO_STRICT_ENV);
        let no_strict = parse_no_strict(raw_no_strict.as_deref());
        Self { raw_no_strict, no_strict }
    }

    pub fn no_strict(&self) -> bool {
        self.no_strict
    }

    /// Freeze only this policy key; all other environment reads stay current.
    pub fn environment_value(&self, name: &str, env: &dyn DailyEnvironment) -> Option<String> {
        if name == NO_STRICT_ENV { self.raw_no_strict.clone() } else { env.get(name) }
    }
}

static PROCESS_SCHEMA_POLICY: OnceLock<SchemaPolicySnapshot> = OnceLock::new();

pub fn process_schema_policy() -> &'static SchemaPolicySnapshot {
    PROCESS_SCHEMA_POLICY
        .get_or_init(|| SchemaPolicySnapshot::capture(&|name: &str| std::env::var(name).ok()))
}

/// The reference Host calls this before model/route resolution, matching the
/// source's module-load constant rather than sampling at each request.
pub fn initialize_process_schema_policy() {
    let _ = process_schema_policy();
}

/// Process-backed Daily resolver input, shared by reload and side-route hosts.
pub fn process_daily_environment(name: &str) -> Option<String> {
    process_schema_policy().environment_value(name, &|name: &str| ProcessConfigEnvironment.get(name))
}
