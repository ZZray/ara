//! Host-owned Date internal slots and clock/local-calendar port for the fixed
//! OMP logger consumers. Registry keys never retain a heap or Session lease.
//! Wider parsing and unverified local gap/range behavior remain explicit gaps.

use ara_rpc::value_heap::{
    CapabilityId, NodeId, RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap,
    ValueRuntime, ValueSnapshot,
};
use chrono::{Datelike, Local, MappedLocalTime, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

mod environment;
mod parse;
#[cfg(windows)]
mod windows_timezone;

type Result<T> = std::result::Result<T, ValueError>;
pub(super) const DATE_PORT: CapabilityId = CapabilityId(0x4152415f44415445);
const TIME_CLIP_LIMIT: f64 = 8_640_000_000_000_000.0;

#[derive(Clone, Copy, Debug)]
pub struct DateLocalParts {
    pub year: i32,
    /// One-based local month, unlike the JS getter's zero-based result.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
    /// JS getTimezoneOffset: UTC minus local, in minutes.
    pub timezone_offset: f64,
}
impl DateLocalParts {
    /// Normalize a requested local day against the existing local month.
    /// Conversion/coercion and invalid Date handling belong to the caller.
    pub fn wall_time_with_day(self, day: i64) -> Result<NaiveDateTime> {
        let first = NaiveDate::from_ymd_opt(self.year, self.month, 1)
            .and_then(|date| {
                date.and_hms_milli_opt(self.hour, self.minute, self.second, self.millisecond)
            })
            .ok_or(ValueError::UnsupportedOperation(
                "Host Date local calendar range is unavailable",
            ))?;
        let days = day
            .checked_sub(1)
            .and_then(chrono::TimeDelta::try_days)
            .ok_or(ValueError::UnsupportedOperation(
                "Host Date local day range is unavailable",
            ))?;
        first
            .checked_add_signed(days)
            .ok_or(ValueError::UnsupportedOperation(
                "Host Date local day is outside chrono range",
            ))
    }
}

/// The Host owns this service. Implementations must not strongly retain a
/// Session, values, factory, runtime or graph heap. The default has no owner
/// fields; controlled hosts can inject clock/timezone policy without config.
pub trait DateHost: Send + Sync {
    /// Preparation initializes the real process TZ once. Controlled hosts keep
    /// their own policy and do not accidentally initialize the system service.
    fn initialize_timezone_env(&self) -> Result<()> {
        Ok(())
    }
    fn timezone_env(&self) -> Result<Option<ara_rpc::WireString>> {
        Err(ValueError::UnsupportedOperation(
            "Host TZ environment getter is not installed",
        ))
    }
    fn set_timezone_env(&self, _value: &ara_rpc::WireString) -> Result<()> {
        Err(ValueError::UnsupportedOperation(
            "Host TZ environment setter is not installed",
        ))
    }
    fn delete_timezone_env(&self) -> Result<()> {
        Err(ValueError::UnsupportedOperation(
            "Host TZ environment deletion is not installed",
        ))
    }
    fn now_ms(&self) -> Result<f64>;
    /// Host-owned Date.parse grammar and local timezone policy. A controlled
    /// clock alone is not a production parser and must opt in explicitly.
    fn parse_ms(&self, _source: &ara_rpc::WireString) -> Result<f64> {
        Err(ValueError::UnsupportedOperation(
            "Host Date.parse is not installed",
        ))
    }
    fn local_parts(&self, utc_ms: i64) -> Result<DateLocalParts>;
    fn local_day_to_utc(&self, current: DateLocalParts, requested_day: i64) -> Result<f64>;
}

pub struct SystemDateHost;
/// Explicit Host checkpoint for a serialized process TZ observation. Raw TZ
/// alone cannot restore a rejected value's previously installed canonical zone.
pub struct SystemDateTimezoneSnapshot {
    #[cfg(windows)]
    inner: windows_timezone::TimezoneSnapshot,
}
impl SystemDateHost {
    pub fn snapshot_timezone_env() -> Result<SystemDateTimezoneSnapshot> {
        #[cfg(windows)]
        {
            Ok(SystemDateTimezoneSnapshot {
                inner: windows_timezone::snapshot_timezone_env()?,
            })
        }
        #[cfg(not(windows))]
        {
            Err(ValueError::UnsupportedOperation(
                "System TZ mutation checkpoint requires the Windows Host",
            ))
        }
    }
    pub fn restore_timezone_env(snapshot: SystemDateTimezoneSnapshot) -> Result<()> {
        #[cfg(windows)]
        {
            windows_timezone::restore_timezone_env(snapshot.inner)
        }
        #[cfg(not(windows))]
        {
            let _ = snapshot;
            Err(ValueError::UnsupportedOperation(
                "System TZ mutation restore requires the Windows Host",
            ))
        }
    }
}
impl DateHost for SystemDateHost {
    fn initialize_timezone_env(&self) -> Result<()> {
        #[cfg(windows)]
        {
            windows_timezone::initialize_timezone_env()
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }
    fn timezone_env(&self) -> Result<Option<ara_rpc::WireString>> {
        #[cfg(windows)]
        {
            windows_timezone::timezone_env()
        }
        #[cfg(not(windows))]
        {
            Err(ValueError::UnsupportedOperation(
                "System TZ environment getter requires the Windows Host",
            ))
        }
    }
    fn set_timezone_env(&self, value: &ara_rpc::WireString) -> Result<()> {
        #[cfg(windows)]
        {
            windows_timezone::set_timezone_env(value)
        }
        #[cfg(not(windows))]
        {
            let _ = value;
            Err(ValueError::UnsupportedOperation(
                "System TZ environment setter requires the Windows Host",
            ))
        }
    }
    fn delete_timezone_env(&self) -> Result<()> {
        #[cfg(windows)]
        {
            windows_timezone::delete_timezone_env()
        }
        #[cfg(not(windows))]
        {
            Err(ValueError::UnsupportedOperation(
                "System TZ environment deletion requires the Windows Host",
            ))
        }
    }
    fn parse_ms(&self, source: &ara_rpc::WireString) -> Result<f64> {
        self.initialize_timezone_env()?;
        let Some(parsed) = parse::parse(source.units()) else {
            return Ok(f64::NAN);
        };
        let utc_ms = if let Some(offset) = parsed.offset_seconds {
            parsed.wall_ms - i64::from(offset) * 1000
        } else {
            local_parse_to_utc(parsed.wall_ms)?
        };
        Ok(time_clip(utc_ms as f64))
    }
    fn now_ms(&self) -> Result<f64> {
        Ok(match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as f64,
            Err(before) => -(before.duration().as_millis() as f64),
        })
    }
    fn local_parts(&self, utc_ms: i64) -> Result<DateLocalParts> {
        let utc = chrono::DateTime::<Utc>::from_timestamp_millis(utc_ms).ok_or(
            ValueError::UnsupportedOperation("Host Date instant is outside chrono range"),
        )?;
        let local = utc.with_timezone(&Local);
        Ok(DateLocalParts {
            year: local.year(),
            month: local.month(),
            day: local.day(),
            hour: local.hour(),
            minute: local.minute(),
            second: local.second(),
            millisecond: local.timestamp_subsec_millis(),
            timezone_offset: -f64::from(local.offset().local_minus_utc()) / 60.0,
        })
    }
    fn local_day_to_utc(&self, current: DateLocalParts, requested_day: i64) -> Result<f64> {
        let wall = current.wall_time_with_day(requested_day)?;
        let millis = match Local.from_local_datetime(&wall) {
            MappedLocalTime::Single(value) => value.timestamp_millis(),
            MappedLocalTime::Ambiguous(first, second) => {
                first.timestamp_millis().min(second.timestamp_millis())
            }
            MappedLocalTime::None => {
                return Err(ValueError::UnsupportedOperation(
                    "Host Date local gap resolution has not been source/oracle accepted",
                ));
            }
        };
        Ok(millis as f64)
    }
}

fn local_parse_to_utc(wall_ms: i64) -> Result<i64> {
    #[cfg(windows)]
    {
        let offset = windows_timezone::installed_local_offset_ms(wall_ms)?;
        Ok(wall_ms - i64::from(offset))
    }
    #[cfg(not(windows))]
    {
        let wall = chrono::DateTime::<Utc>::from_timestamp_millis(wall_ms).ok_or(
            ValueError::UnsupportedOperation(
                "Host Date.parse local instant is outside chrono range",
            ),
        )?;
        match Local.from_local_datetime(&wall.naive_utc()) {
            MappedLocalTime::Single(value) => Ok(value.timestamp_millis()),
            MappedLocalTime::Ambiguous(first, second) => {
                Ok(first.timestamp_millis().min(second.timestamp_millis()))
            }
            MappedLocalTime::None => Err(ValueError::UnsupportedOperation(
                "Host Date.parse local gap requires an ICU timezone adapter",
            )),
        }
    }
}

pub(super) fn time_clip(value: f64) -> f64 {
    if !value.is_finite() || value.abs() > TIME_CLIP_LIMIT {
        f64::NAN
    } else if value == 0.0 {
        0.0
    } else {
        let clipped = value.trunc();
        if clipped == 0.0 { 0.0 } else { clipped }
    }
}

/// Actual Date state is not an ordinary JS data/symbol property. Copying a
/// prototype or own descriptors cannot fabricate a registered Date receiver.
/// The same heap token and non-recycled node identity are checked every time.
pub(super) struct NativeDateDomain {
    heap_token: u64,
    host: Arc<dyn DateHost>,
    slots: Mutex<HashMap<NodeId, f64>>,
    timezone_environment: OnceLock<NodeId>,
}
impl NativeDateDomain {
    pub(super) fn new(heap_token: u64, host: Arc<dyn DateHost>) -> Self {
        Self {
            heap_token,
            host,
            slots: Mutex::new(HashMap::new()),
            timezone_environment: OnceLock::new(),
        }
    }
    fn same_heap(&self, value: &ValueHandle) -> Result<()> {
        if value.heap().heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }
    fn value(&self, receiver: &ValueHandle) -> Result<f64> {
        self.same_heap(receiver)?;
        self.slots
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .get(&receiver.node_id())
            .copied()
            .ok_or_else(|| ValueError::TypeError("Type error".into()))
    }
    fn store(&self, receiver: &ValueHandle, value: f64, register: bool) -> Result<()> {
        self.same_heap(receiver)?;
        if register && !matches!(receiver.heap().snapshot(receiver)?, ValueSnapshot::Object) {
            return Err(ValueError::WrongRuntimeReply(
                "Native Date constructor receiver",
            ));
        }
        let mut slots = self.slots.lock().map_err(|_| ValueError::PoisonedHeap)?;
        if register {
            if slots.contains_key(&receiver.node_id()) {
                return Err(ValueError::UnsupportedOperation(
                    "Date internal slot is already initialized",
                ));
            }
            slots.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        } else if !slots.contains_key(&receiver.node_id()) {
            return Err(ValueError::TypeError("Type error".into()));
        }
        slots.insert(receiver.node_id(), value);
        Ok(())
    }
    pub(super) fn resolve(
        &self,
        request: &RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply>> {
        if let Some(result) = self.resolve_environment(request, effective) {
            return Some(result);
        }
        let RuntimeRequest::Call {
            capability,
            receiver,
            arguments,
        } = request
        else {
            return None;
        };
        if *capability != DATE_PORT {
            return None;
        }
        Some(self.invoke(receiver, arguments).map(RuntimeReply::Value))
    }
    fn invoke(&self, receiver: &ValueHandle, arguments: &[ValueHandle]) -> Result<ValueHandle> {
        self.same_heap(receiver)?;
        let heap = receiver.heap();
        let ValueSnapshot::String(operation) =
            heap.snapshot(arguments.first().ok_or(ValueError::InvalidNode)?)?
        else {
            return Err(ValueError::WrongRuntimeReply("Native Date operation"));
        };
        let operation = operation
            .to_utf8()
            .map_err(|_| ValueError::WrongRuntimeReply("Native Date operation"))?;
        let number = |index| -> Result<f64> {
            match heap.snapshot(arguments.get(index).ok_or(ValueError::InvalidNode)?)? {
                ValueSnapshot::Number(value) => Ok(value),
                _ => Err(ValueError::WrongRuntimeReply(
                    "Native Date numeric argument",
                )),
            }
        };
        if operation == "parse" {
            let ValueSnapshot::String(source) =
                heap.snapshot(arguments.get(1).ok_or(ValueError::InvalidNode)?)?
            else {
                return Err(ValueError::WrongRuntimeReply(
                    "Native Date.parse string argument",
                ));
            };
            return heap.number(self.host.parse_ms(&source)?);
        }
        if operation == "now" {
            return heap.number(self.host.now_ms()?);
        }
        if operation == "copy" {
            let source = arguments.get(1).ok_or(ValueError::InvalidNode)?;
            self.same_heap(source)?;
            let value = self
                .slots
                .lock()
                .map_err(|_| ValueError::PoisonedHeap)?
                .get(&source.node_id())
                .copied();
            return match value {
                Some(value) => heap.number(value),
                None => heap.undefined(),
            };
        }
        if operation == "register" {
            self.store(receiver, time_clip(number(1)?), true)?;
            return Ok(receiver.clone());
        }
        if operation == "setDate" {
            // The compiled body already read t before observable ToNumber.
            // No slot lock spans a Host timezone callback or graph operation.
            self.value(receiver)?;
            let initial = number(1)?;
            let day = number(2)?;
            let next = if !initial.is_finite() || !day.is_finite() {
                f64::NAN
            } else if day.abs() > 1_000_000_000.0 {
                // Even across the full Date interval and all real local UTC
                // offsets this requested day is outside TimeClip's interval.
                f64::NAN
            } else {
                let parts = self.host.local_parts(initial as i64)?;
                time_clip(self.host.local_day_to_utc(parts, day.trunc() as i64)?)
            };
            self.store(receiver, next, false)?;
            return heap.number(next);
        }
        let value = self.value(receiver)?;
        if operation == "getTime" || operation == "valueOf" {
            return heap.number(value);
        }
        if operation == "toISOString" {
            if !value.is_finite() {
                return Err(ValueError::RangeError("Invalid Date".into()));
            }
            let utc = chrono::DateTime::<Utc>::from_timestamp_millis(value as i64).ok_or(
                ValueError::UnsupportedOperation("Native Date ISO instant is outside chrono range"),
            )?;
            let year = utc.year();
            let year = if (0..=9999).contains(&year) {
                format!("{year:04}")
            } else {
                format!("{year:+07}")
            };
            return heap.string(
                format!(
                    "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                    utc.month(),
                    utc.day(),
                    utc.hour(),
                    utc.minute(),
                    utc.second(),
                    utc.timestamp_subsec_millis(),
                )
                .into(),
            );
        }
        if !value.is_finite() {
            return heap.number(f64::NAN);
        }
        let parts = self.host.local_parts(value as i64)?;
        heap.number(match operation.as_str() {
            "getFullYear" => f64::from(parts.year),
            "getMonth" => f64::from(parts.month - 1),
            "getDate" => f64::from(parts.day),
            "getHours" => f64::from(parts.hour),
            "getMinutes" => f64::from(parts.minute),
            "getSeconds" => f64::from(parts.second),
            "getMilliseconds" => f64::from(parts.millisecond),
            "getTimezoneOffset" => parts.timezone_offset,
            _ => {
                return Err(ValueError::WrongRuntimeReply(
                    "Unknown Native Date operation",
                ));
            }
        })
    }
}
