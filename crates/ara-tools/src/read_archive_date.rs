//! Narrow Host calendar port for fixed OMP CAB/RAR4 local DOS timestamps.
//! The Host owns process TZ and gap/overlap policy; Core only normalizes the
//! six archive integer fields before the Host's existing Date parser.
use ara_agent::ToolError;

#[derive(Clone, Copy, Debug)]
pub struct ArchiveLocalDate {
    pub year: i32,
    /// One based raw month: RAR4 admits 0..15 and Date normalizes the overflow.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}
pub trait ArchiveDatePort: Send + Sync {
    fn to_utc_ms(&self, fields: ArchiveLocalDate) -> Result<f64, ToolError>;
}
impl ArchiveLocalDate {
    /// Equivalent raw-component normalization for the bounded CAB/RAR fields.
    /// CAB validates its own fields first; RAR4 intentionally has no such gate.
    /// This string has no UTC suffix: it is input to the existing Host local
    /// Date parser, never a replacement timezone implementation.
    pub fn normalized_local_iso(self) -> String {
        let month_index = i64::from(self.month) - 1;
        let year = i64::from(self.year) + month_index.div_euclid(12);
        let month = month_index.rem_euclid(12) + 1;
        let first = days_from_civil(year, month, 1);
        let seconds = (first + i64::from(self.day) - 1) * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second);
        let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
        let time = seconds.rem_euclid(86_400);
        let hour = time / 3600;
        let minute = time / 60 % 60;
        let second = time % 60;
        // Actual archive years are 1980..2107; normalized range is 1979..2108.
        let year = if (0..=9999).contains(&year) { format!("{year:04}") } else { format!("{year:+07}") };
        format!("{year}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
    }
}
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + day - 1 - 719468
}
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}
