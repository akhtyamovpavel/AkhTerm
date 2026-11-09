//! Register the bundled interface font with this process's Fontconfig instance.

use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};

const GEIST: &[u8] = include_bytes!("../assets/fonts/Geist-Variable.ttf");
const MFD_CLOEXEC: c_uint = 1;

extern "C" {
    fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
    fn FcConfigGetCurrent() -> *mut c_void;
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const u8) -> c_int;
}

pub fn load() -> io::Result<File> {
    let fd = unsafe { memfd_create(c"akhterm-geist".as_ptr(), MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(GEIST)?;

    // Fontconfig opens the file again when drawing glyphs. Keep it open until GTK exits.
    let path = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
    let added = unsafe { FcConfigAppFontAddFile(FcConfigGetCurrent(), path.as_ptr().cast()) };
    if added == 0 {
        return Err(io::Error::other("Fontconfig could not register Geist"));
    }
    Ok(file)
}
