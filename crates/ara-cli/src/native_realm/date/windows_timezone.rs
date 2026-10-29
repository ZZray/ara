//! Windows Host timezone conversion using the system ICU calendar API.
//! Bun's DateCache uses FORMER for both gaps and overlaps. This adapter uses
//! the same operation over the full Date interval; no chrono range truncation.
//! It changes only Date.parse; existing Date getters/setDate retain their port.
use ara_rpc::{WireString, value_heap::ValueError};
use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr::NonNull, sync::Mutex};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
}
type Open = unsafe extern "C" fn(*const u16, i32, *const u8, i32, *mut i32) -> *mut c_void;
type Close = unsafe extern "C" fn(*mut c_void);
type SetMillis = unsafe extern "C" fn(*mut c_void, f64, *mut i32);
type Offset = unsafe extern "C" fn(*const c_void, i32, i32, *mut i32, *mut i32, *mut i32);
type GregorianChange = unsafe extern "C" fn(*mut c_void, f64, *mut i32);
type CanonicalZone = unsafe extern "C" fn(*const u16, i32, *mut u16, i32, *mut i8, *mut i32) -> i32;

struct Library(NonNull<c_void>);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0.as_ptr());
        }
    }
}
struct Icu {
    library: Library,
    open: Open,
    close: Close,
    set_millis: SetMillis,
    offset: Offset,
    gregorian_change: GregorianChange,
    canonical_zone: CanonicalZone,
}
impl Icu {
    fn load() -> Result<Self, &'static str> {
        let name: Vec<u16> = "icu.dll\0".encode_utf16().collect();
        // The fixed DLL name is resolved only in Windows System32, never cwd,
        // a project directory, PATH, or a configurable provider location.
        let library = Library(
            NonNull::new(unsafe {
                LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), 0x0000_0800)
            })
            .ok_or("System ICU calendar library is unavailable")?,
        );
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                let address =
                    unsafe { GetProcAddress(library.0.as_ptr(), concat!($name, "\0").as_ptr()) };
                if address.is_null() {
                    return Err("System ICU calendar function is unavailable");
                }
                // Each fixed symbol's C ABI is taken from unicode/ucal.h;
                // the retained library outlives every call and calendar.
                unsafe { std::mem::transmute::<*mut c_void, $ty>(address) }
            }};
        }
        let open = symbol!("ucal_open", Open);
        let close = symbol!("ucal_close", Close);
        let set_millis = symbol!("ucal_setMillis", SetMillis);
        let offset = symbol!("ucal_getTimeZoneOffsetFromLocal", Offset);
        let gregorian_change = symbol!("ucal_setGregorianChange", GregorianChange);
        let canonical_zone = symbol!("ucal_getCanonicalTimeZoneID", CanonicalZone);
        Ok(Self {
            library,
            open,
            close,
            set_millis,
            offset,
            gregorian_change,
            canonical_zone,
        })
    }
}
struct Calendar<'a> {
    owner: &'a Icu,
    pointer: NonNull<c_void>,
}
impl Drop for Calendar<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.owner.close)(self.pointer.as_ptr());
        }
    }
}

fn canonical_timezone(icu: &Icu, zone: &[u16]) -> Result<Option<Vec<u16>>, &'static str> {
    let mut canonical = vec![0_u16; 64];
    let input_length = i32::try_from(zone.len()).map_err(|_| "Host timezone name is too long")?;
    loop {
        let (mut status, mut system_id) = (0, 0_i8);
        let needed = unsafe {
            (icu.canonical_zone)(
                zone.as_ptr(),
                input_length,
                canonical.as_mut_ptr(),
                canonical.len() as i32,
                &mut system_id,
                &mut status,
            )
        };
        if status == 15 && needed >= 0 {
            let count =
                usize::try_from(needed).map_err(|_| "Host timezone canonical length is invalid")?;
            canonical
                .try_reserve(count.saturating_add(1).saturating_sub(canonical.len()))
                .map_err(|_| "Host timezone allocation failed")?;
            canonical.resize(count + 1, 0);
            continue;
        }
        // A rejected name must not replace the installed override.
        if status == 1 {
            return Ok(None);
        }
        if status > 0 || needed < 0 || needed as usize > canonical.len() {
            return Err("Host timezone override is not recognized by system ICU");
        }
        canonical.truncate(needed as usize);
        return Ok(Some(canonical));
    }
}

#[derive(Default)]
struct ProcessTimezone {
    initialized: bool,
    installed: Option<Vec<u16>>,
}
// Fixed WebKit's override is process-wide, not per Date/Realm/Session. This
// service is the CLI main logical Host; worker-local env emulation is absent.
static PROCESS_TIMEZONE: Mutex<ProcessTimezone> = Mutex::new(ProcessTimezone {
    initialized: false,
    installed: None,
});
fn initialize(state: &mut ProcessTimezone) -> super::Result<()> {
    if !state.initialized {
        let initial = read_timezone();
        state.installed = match initial.as_ref().filter(|value| !value.is_empty()) {
            Some(zone) => canonical_timezone(
                &Icu::load().map_err(ValueError::UnsupportedOperation)?,
                zone,
            )
            .map_err(ValueError::UnsupportedOperation)?,
            None => None,
        };
        state.initialized = true;
    }
    Ok(())
}
fn read_timezone() -> Option<Vec<u16>> {
    std::env::var_os("TZ").map(|value| value.encode_wide().collect())
}
pub(super) fn initialize_timezone_env() -> super::Result<()> {
    let mut state = PROCESS_TIMEZONE
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?;
    initialize(&mut state)
}
pub(super) fn timezone_env() -> super::Result<Option<WireString>> {
    let mut state = PROCESS_TIMEZONE
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?;
    initialize(&mut state)?;
    Ok(read_timezone().map(WireString::from_units))
}
fn write_timezone(value: Option<&[u16]>) -> super::Result<()> {
    let name = [b'T' as u16, b'Z' as u16, 0];
    let mut terminated = value.map(|value| value.to_vec());
    if let Some(value) = &mut terminated {
        value.push(0);
    }
    let pointer = terminated
        .as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr());
    if unsafe { SetEnvironmentVariableW(name.as_ptr(), pointer) } == 0 {
        let error = std::io::Error::last_os_error();
        // Deleting an already absent TZ still clears the Date override.
        if value.is_some() || error.raw_os_error() != Some(203) {
            return Err(ValueError::UnsupportedOperation(
                "Windows parent TZ environment mutation failed",
            ));
        }
    }
    Ok(())
}
pub(super) fn set_timezone_env(value: &WireString) -> super::Result<()> {
    let mut state = PROCESS_TIMEZONE
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?;
    initialize(&mut state)?;
    // Fixed Bun applyTZFromString checks WTF String UTF16 length < 32. Parse
    // coercion is performed outside this lock by the invoking effective realm.
    let installed = if value.is_empty() {
        Some(None)
    } else if value.units().len() < 32 {
        canonical_timezone(
            &Icu::load().map_err(ValueError::UnsupportedOperation)?,
            value.units(),
        )
        .map_err(ValueError::UnsupportedOperation)?
        .map(Some)
    } else {
        None
    };
    // A successful raw environment write and installed-zone update form one
    // synchronous Host commit. Invalid raw TZ is written but preserves history.
    write_timezone(Some(value.units()))?;
    if let Some(installed) = installed {
        state.installed = installed;
    }
    Ok(())
}
pub(super) fn delete_timezone_env() -> super::Result<()> {
    let mut state = PROCESS_TIMEZONE
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?;
    initialize(&mut state)?;
    write_timezone(None)?;
    state.installed = None;
    Ok(())
}
pub(super) fn installed_local_offset_ms(wall_ms: i64) -> super::Result<i32> {
    let mut state = PROCESS_TIMEZONE
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?;
    initialize(&mut state)?;
    local_offset_ms(wall_ms, state.installed.as_deref()).map_err(ValueError::UnsupportedOperation)
}
pub(super) struct TimezoneSnapshot {
    raw: Option<Vec<u16>>,
    installed: Option<Vec<u16>>,
}
pub(super) fn snapshot_timezone_env() -> super::Result<TimezoneSnapshot> {
    let mut state = PROCESS_TIMEZONE
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?;
    initialize(&mut state)?;
    Ok(TimezoneSnapshot {
        raw: read_timezone(),
        installed: state.installed.clone(),
    })
}
pub(super) fn restore_timezone_env(snapshot: TimezoneSnapshot) -> super::Result<()> {
    let mut state = PROCESS_TIMEZONE
        .lock()
        .map_err(|_| ValueError::PoisonedHeap)?;
    write_timezone(snapshot.raw.as_deref())?;
    state.installed = snapshot.installed;
    state.initialized = true;
    Ok(())
}

pub(super) fn local_offset_ms(wall_ms: i64, timezone: Option<&[u16]>) -> Result<i32, &'static str> {
    let icu = Icu::load()?;
    let _retained_library = &icu.library;
    let (zone, length) = match timezone.filter(|zone| !zone.is_empty()) {
        Some(zone) => (
            zone.as_ptr(),
            i32::try_from(zone.len()).map_err(|_| "Host timezone name is too long")?,
        ),
        None => (std::ptr::null(), 0),
    };
    let mut status = 0;
    let pointer = unsafe { (icu.open)(zone, length, c"".as_ptr().cast(), 0, &mut status) };
    // ICU can return a calendar with a failure status; retain it before checking
    // status so even this path closes the original allocation exactly once.
    let calendar = NonNull::new(pointer).map(|pointer| Calendar {
        owner: &icu,
        pointer,
    });
    if status > 0 {
        return Err("System ICU calendar open failed");
    }
    let calendar = calendar.ok_or("System ICU calendar allocation failed")?;
    // Source moves the Gregorian cutover to the minimum Date instant. ICU's
    // default 1582 cutover must not replace ECMAScript's proleptic calendar.
    unsafe {
        (icu.gregorian_change)(
            calendar.pointer.as_ptr(),
            -8_640_000_000_000_000.0,
            &mut status,
        );
    }
    // Source explicitly ignores an unsupported Gregorian change. Reset that
    // local status so it does not poison the subsequent independent operation.
    if status == 16 {
        status = 0;
    }
    if status > 0 {
        return Err("System ICU Gregorian calendar setup failed");
    }
    unsafe {
        (icu.set_millis)(calendar.pointer.as_ptr(), wall_ms as f64, &mut status);
    }
    if status > 0 {
        return Err("System ICU local date is unavailable");
    }
    let (mut raw, mut dst) = (0, 0);
    // UTimeZoneLocalOption::UCAL_TZ_LOCAL_FORMER = 0x04. Source uses
    // FORMER for both nonexistent and duplicated wall times.
    unsafe {
        (icu.offset)(
            calendar.pointer.as_ptr(),
            4,
            4,
            &mut raw,
            &mut dst,
            &mut status,
        );
    }
    if status > 0 {
        return Err("System ICU local timezone conversion failed");
    }
    raw.checked_add(dst)
        .ok_or("System ICU local timezone offset overflow")
}
