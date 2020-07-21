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
    #[error("Esc was pressed during drift correction.")]
    EscPressed,
    #[error("Failed Command: {}", self)]
    CommandError(String),
    #[error(transparent)]
    CStringError(#[from] std::ffi::NulError),
    #[error(transparent)]
    IntoStringError(#[from] std::ffi::IntoStringError),
    #[error(
        "eyelink-rs received an undocumented return value from libeyelink_sys: {}",
        self
    )]
    APIError(i16),
    #[error("Data File Error {code}: {msg}")]
    DataError { code: i32, msg: String },
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
    let version = unsafe { libeyelink_sys::eyelink_get_tracker_version(&mut version_str[0]) };

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
        e => Err(EyelinkError::APIError(e)),
    }
}

pub fn break_pressed() -> Result<bool, EyelinkError> {
    let res = unsafe { libeyelink_sys::break_pressed() };

    match res {
        0 => Ok(false),
        1 => Ok(true),
        e => Err(EyelinkError::APIError(e)),
    }
}

// TODO(lukehsiao): Is this needed? Is this eyelink-specific, or can we just use Rust timing?
pub fn msec_delay(n: u32) {
    unsafe { libeyelink_sys::msec_delay(n) }
}

pub fn close_eyelink_connection() {
    unsafe { libeyelink_sys::close_eyelink_connection() }
}

pub fn open_data_file(path: &str) -> Result<(), EyelinkError> {
    let c_path = match CString::new(path) {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::CStringError(e)),
    };
    let ptr = c_path.into_raw();
    let res = unsafe {
        let res = libeyelink_sys::open_data_file(ptr);

        // Retake pointer to free memory
        let _ = CString::from_raw(ptr);

        res
    };

    match res {
        0 => Ok(()),
        e => Err(EyelinkError::APIError(e)),
    }
}

pub fn receive_data_file(src: &str) -> Result<i32, EyelinkError> {
    let c_src = match CString::new(src) {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::CStringError(e)),
    };
    let src_ptr = c_src.into_raw();

    let c_dst = match CString::new("") {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::CStringError(e)),
    };
    let dst_ptr = c_dst.into_raw();

    let res = unsafe {
        let res = libeyelink_sys::receive_data_file(src_ptr, dst_ptr, 0);

        // Retake pointer to free memory
        let _ = CString::from_raw(src_ptr);
        let _ = CString::from_raw(dst_ptr);

        res
    };

    match res {
        0 => Err(EyelinkError::DataError {
            code: 0,
            msg: "File transfer was cancelled.".to_string(),
        }),
        libeyelink_sys::FILE_CANT_OPEN => Err(EyelinkError::DataError {
            code: libeyelink_sys::FILE_CANT_OPEN,
            msg: "Cannot open file.".to_string(),
        }),
        libeyelink_sys::FILE_XFER_ABORTED => Err(EyelinkError::DataError {
            code: libeyelink_sys::FILE_XFER_ABORTED,
            msg: "Data error. Aborted.".to_string(),
        }),
        n => Ok(n),
    }
}

pub fn init_expt_graphics(info: &mut libeyelink_sys::DISPLAYINFO) -> Result<(), EyelinkError> {
    unsafe {
        match libeyelink_sys::init_expt_graphics(std::ptr::null_mut(), info) {
            0 => Ok(()),
            e => Err(EyelinkError::APIError(e)),
        }
    }
}

pub fn get_display_information() -> libeyelink_sys::DISPLAYINFO {
    let mut info: libeyelink_sys::DISPLAYINFO = libeyelink_sys::DISPLAYINFO {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
        width: 0,
        height: 0,
        bits: 0,
        palsize: 0,
        palrsvd: 0,
        pages: 0,
        refresh: 0.0,
        winnt: 0,
    };
    unsafe {
        libeyelink_sys::get_display_information(&mut info);
    }
    info
}

pub fn do_tracker_setup() {
    unsafe {
        libeyelink_sys::do_tracker_setup();
    }
}

pub fn set_calibration_colors(
    fg: &mut libeyelink_sys::SDL_Color,
    bg: &mut libeyelink_sys::SDL_Color,
) {
    unsafe {
        libeyelink_sys::set_calibration_colors(fg, bg);
    }
}

pub fn set_target_size(diameter: u16, holesize: u16) {
    unsafe {
        libeyelink_sys::set_target_size(diameter, holesize);
    }
}

pub fn do_drift_correct(x: i16, y: i16, draw: bool, allow_setup: bool) -> Result<(), EyelinkError> {
    let res = unsafe { libeyelink_sys::do_drift_correct(x, y, draw as i16, allow_setup as i16) };
    match res {
        0 => Ok(()),
        27 => Err(EyelinkError::EscPressed),
        n => Err(EyelinkError::APIError(n)),
    }
}

pub fn start_recording(
    file_samples: bool,
    file_events: bool,
    link_samples: bool,
    link_events: bool,
) -> Result<(), i16> {
    let res = unsafe {
        libeyelink_sys::start_recording(
            file_samples as i16,
            file_events as i16,
            link_samples as i16,
            link_events as i16,
        )
    };
    match res {
        0 => Ok(()),
        n => Err(n),
    }
}

pub fn stop_recording() {
    unsafe { libeyelink_sys::stop_recording() }
}

pub fn begin_realtime_mode(delay_ms: u32) {
    unsafe { libeyelink_sys::begin_realtime_mode(delay_ms) }
}

pub fn end_realtime_mode() {
    unsafe { libeyelink_sys::end_realtime_mode() }
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

    #[test]
    fn test_eyelink_receive_data_file() {
        // Should fail w/o an eyelink installed.
        match receive_data_file("test.edf") {
            Err(e) => match e {
                EyelinkError::DataError { code, msg: _ } if code != 0 => {
                    panic!("Should have failed.");
                }
                _ => (),
            },
            _ => (),
        }
    }

    #[test]
    fn test_get_display_information() {
        let info = get_display_information();
        assert_eq!(info.left, 0);
        assert_eq!(info.right, 1920);
        assert_eq!(info.top, 0);
        assert_eq!(info.bottom, 1080);
    }
}
