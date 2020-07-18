use std::ffi::CString;
use std::os::raw::c_char;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EyelinkError {
    #[error("Invalid IP Address {}", self)]
    InvalidIP(String),
    #[error("Unable to connect to Eyelink")]
    ConnectionError,
    #[error("Failed Command: {}", self)]
    CommandError(String),
    #[error(transparent)]
    CStringError(#[from] std::ffi::NulError),
    #[error(transparent)]
    IntoStringError(#[from] std::ffi::IntoStringError),
}

#[derive(Debug)]
pub enum OpenMode {
    Dummy,
    Real,
    NoConn,
}

pub fn set_eyelink_address(addr: &str) -> Result<(), EyelinkError> {
    let c_addr = match CString::new(addr) {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::CStringError(e)),
    };

    let ptr = c_addr.into_raw();
    unsafe {
        let res = match libeyelink_sys::set_eyelink_address(ptr) {
            0 => Ok(()),
            _ => Err(EyelinkError::InvalidIP(addr.into())),
        };

        // Retake pointer to free memory
        let _ = CString::from_raw(ptr);

        res
    }
}

pub fn open_eyelink_connection(mode: OpenMode) -> Result<(), EyelinkError> {
    let res = unsafe {
        match mode {
            OpenMode::Dummy => libeyelink_sys::open_eyelink_connection(1),
            OpenMode::Real => libeyelink_sys::open_eyelink_connection(0),
            OpenMode::NoConn => libeyelink_sys::open_eyelink_connection(-1),
        }
    };

    match res {
        0 => Ok(()),
        _ => Err(EyelinkError::ConnectionError),
    }
}

pub fn set_offline_mode() {
    unsafe { libeyelink_sys::set_offline_mode() }
}

pub fn flush_getkey_queue() {
    unsafe { libeyelink_sys::flush_getkey_queue() }
}

pub fn eyecmd_printf(cmd: &str) -> Result<(), EyelinkError> {
    let c_cmd = match CString::new(cmd) {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::CStringError(e)),
    };

    unsafe {
        match libeyelink_sys::eyecmd_printf(c_cmd.as_ptr()) {
            0 => Ok(()),
            _ => Err(EyelinkError::CommandError(cmd.into())),
        }
    }
}

pub fn eyelink_get_tracker_version() -> Result<(i16, String), EyelinkError> {
    // Must be at least length 40 per eyelink api
    let mut version_str: [c_char; 40] = [0; 40];
    let version_str_ptr: *mut c_char = &mut version_str[0];
    let version = unsafe { libeyelink_sys::eyelink_get_tracker_version(version_str_ptr) };

    let cstring = match CString::new(version_str.iter().map(|c| *c as u8).collect::<Vec<u8>>()) {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::CStringError(e)),
    };

    let sw_version = match cstring.into_string() {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::IntoStringError(e)),
    };

    Ok((version, sw_version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_eyelink_address() {
        if let Ok(_) = set_eyelink_address("jibberish") {
            panic!("Should have failed.");
        }

        if let Err(_) = set_eyelink_address("100.0.1.1") {
            panic!("Should have passed.");
        }
    }

    #[test]
    fn test_open_eyelink_connection() {
        // Should fail w/o an eyelink installed.
        if let Ok(_) = open_eyelink_connection(OpenMode::Real) {
            panic!("Should have failed.");
        }

        // Should be fine in dummy mode.
        if let Err(_) = open_eyelink_connection(OpenMode::Dummy) {
            panic!("Should have passed.");
        }
    }
}
