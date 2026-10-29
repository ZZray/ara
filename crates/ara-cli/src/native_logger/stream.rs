//! One synchronous borrowed process stdout/stderr write for fixed OMP logger.
//! MIT; source notices retained in THIRD_PARTY_NOTICES.md.

pub(super) fn write_once(fd: i32, bytes: &[u8]) -> std::io::Result<usize> {
    if !matches!(fd, 1 | 2) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Logger stream must be stdout or stderr"));
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(stream: u32) -> *mut c_void;
            fn WriteFile(
                handle: *mut c_void,
                bytes: *const u8,
                length: u32,
                written: *mut u32,
                overlapped: *mut c_void,
            ) -> i32;
        }
        let stream = if fd == 1 { -11_i32 } else { -12_i32 };
        let handle = unsafe { GetStdHandle(stream as u32) };
        if handle.is_null() || handle == (-1_isize) as *mut c_void {
            return Err(std::io::Error::from_raw_os_error(6));
        }
        let length = bytes.len().min(u32::MAX as usize) as u32;
        let mut written = 0_u32;
        // Borrow the process handle. Never close, retry or write_all after an
        // unknown/partial effect; the caller receives the actual byte count.
        let ok = unsafe { WriteFile(handle, bytes.as_ptr(), length, &mut written, std::ptr::null_mut()) };
        if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(written as usize) }
    }
    #[cfg(unix)]
    {
        // Borrow fd for exactly one syscall; retain genuine Interrupted/IO.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written < 0 { Err(std::io::Error::last_os_error()) } else { Ok(written as usize) }
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Native logger process stream is unavailable"))
    }
}
