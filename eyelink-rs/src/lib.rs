use std::ffi::CString;
use std::os::raw::c_char;

use thiserror::Error;

pub use libeyelink_sys;

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
    #[error("eyelink-rs received an undocumented return value from libeyelink_sys")]
    APIError,
}

#[derive(Debug, PartialEq)]
pub enum OpenMode {
    Dummy,
    Real,
    NoConn,
}

#[derive(Debug, PartialEq)]
pub enum ConnectionStatus {
    Closed,
    Simulated,
    Normal,
    Broadcast,
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

pub fn eyelink_get_tracker_version() -> Result<(i16, i16), EyelinkError> {
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

    // Parse major version from the form "EYELINK XX x.xx"
    let major = sw_version
        .as_str()
        .split(' ')
        .next_back()
        .unwrap()
        .split('.')
        .next()
        .expect("Unable to parse sw version")
        .parse::<i16>()
        .unwrap();

    Ok((version, major))
}

pub fn eyelink_is_connected() -> Result<ConnectionStatus, EyelinkError> {
    let res = unsafe { libeyelink_sys::eyelink_is_connected() };

    match res {
        0 => Ok(ConnectionStatus::Closed),
        -1 => Ok(ConnectionStatus::Simulated),
        1 => Ok(ConnectionStatus::Normal),
        2 => Ok(ConnectionStatus::Broadcast),
        _ => Err(EyelinkError::APIError),
    }
}

pub fn break_pressed() -> Result<bool, EyelinkError> {
    let res = unsafe { libeyelink_sys::break_pressed() };

    match res {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(EyelinkError::APIError),
    }
}

// TODO(lukehsiao): Is this needed? Is this eyelink-specific, or can we just use Rust timing?
pub fn msec_delay(n: u32) {
    unsafe { libeyelink_sys::msec_delay(n) }
}

pub fn close_eyelink_connection() {
    unsafe { libeyelink_sys::close_eyelink_connection() }
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

    #[test]
    fn test_eyelink_is_connected() {
        // Should report closed w/ no Eyelink connected
        assert_eq!(ConnectionStatus::Closed, eyelink_is_connected().unwrap())
    }
}
