//! Thin, safe wrappers around libeyelink-sys.

use std::ffi::CString;
use std::os::raw::c_char;

use c_fixed_string::CFixedStr;
use libeyelink_sys::FSAMPLE;
use thiserror::Error;

pub use libeyelink_sys;
pub mod ascparser;
pub mod eyelink;
pub mod graphics;

#[derive(Error, Debug)]
/// Wrapper for all errors working with libeyelink-sys.
pub enum EyelinkError {
    #[error("Invalid IP Address {}", self)]
    InvalidIp(String),
    #[error("Unable to connect to Eyelink")]
    ConnectionError,
    #[error("Esc was pressed during drift correction.")]
    EscPressed,
    #[error("Time expired without any data of masked types available.")]
    BlockStartError,
    #[error("No eye data is available.")]
    EyeDataError,
    #[error("No eye tracking samples available.")]
    NoSampleError,
    #[error("No new eye tracking samples available.")]
    NoNewSampleError,
    #[error("Failed Command: {}", self)]
    CommandError(String),
    #[error(transparent)]
    CStringError(#[from] std::ffi::NulError),
    #[error(transparent)]
    IntoStringError(#[from] std::ffi::IntoStringError),
    #[error(transparent)]
    Utf8Error(#[from] std::str::Utf8Error),
    #[error(
        "eyelink-rs received an undocumented return value from libeyelink_sys: {}",
        self
    )]
    ApiError(i16),
    #[error("Data File Error {code}: {msg}")]
    DataError { code: i32, msg: String },
    #[error("SDL Error: [{code}] {msg}")]
    SdlError { code: i32, msg: String },
}

#[derive(Debug, PartialEq)]
pub enum OpenMode {
    Dummy,
    Real,
    NoConn,
}

#[derive(Debug)]
pub enum EyeData {
    Left = 0,
    Right = 1,
    Binocular = 2,
}

#[derive(Debug, PartialEq)]
pub enum ConnectionStatus {
    Closed,
    Simulated,
    Normal,
    Broadcast,
}

pub fn set_eyelink_address(addr: &str) -> Result<(), EyelinkError> {
    let c_addr = CString::new(addr).map_err(EyelinkError::CStringError)?;
    unsafe {
        match libeyelink_sys::set_eyelink_address(c_addr.as_ptr() as *mut c_char) {
            0 => Ok(()),
            _ => Err(EyelinkError::InvalidIp(addr.into())),
        }
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

pub fn eyemsg_printf(msg: &str) -> Result<(), EyelinkError> {
    let c_msg = match CString::new(msg) {
        Ok(s) => s,
        Err(e) => return Err(EyelinkError::CStringError(e)),
    };

    unsafe {
        match libeyelink_sys::eyemsg_printf(c_msg.as_ptr()) {
            0 => Ok(()),
            n => Err(EyelinkError::ApiError(n)),
        }
    }
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

fn parse_sw_version(version: &str) -> Result<i16, ()> {
    // Parse major version from the form "EYELINK XX x.xx"
    let mut parts = version.trim().split(' ');
    match parts
        .next_back()
        .unwrap()
        .split('.')
        .next()
        .unwrap()
        .parse::<i16>()
    {
        Ok(n) => Ok(n),
        Err(_) => Err(()),
    }
}

pub fn eyelink_get_tracker_version() -> Result<(i16, i16), EyelinkError> {
    // Must be at least length 40 per the eyelink api
    let mut buffer: [u8; 256] = [0; 256];
    let version =
        unsafe { libeyelink_sys::eyelink_get_tracker_version(buffer.as_mut_ptr() as *mut c_char) };

    let sw_version = CFixedStr::from_bytes(&buffer)
        .to_str()
        .map_err(EyelinkError::Utf8Error)?;

    // Parse major version from the form "EYELINK XX x.xx"
    let major = parse_sw_version(sw_version).map_err(|_| EyelinkError::ApiError(0))?;

    Ok((version, major))
}

pub fn eyelink_is_connected() -> Result<ConnectionStatus, EyelinkError> {
    let res = unsafe { libeyelink_sys::eyelink_is_connected() };

    match res {
        0 => Ok(ConnectionStatus::Closed),
        -1 => Ok(ConnectionStatus::Simulated),
        1 => Ok(ConnectionStatus::Normal),
        2 => Ok(ConnectionStatus::Broadcast),
        e => Err(EyelinkError::ApiError(e)),
    }
}

pub fn break_pressed() -> Result<bool, EyelinkError> {
    let res = unsafe { libeyelink_sys::break_pressed() };

    match res {
        0 => Ok(false),
        1 => Ok(true),
        e => Err(EyelinkError::ApiError(e)),
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
    let c_path = CString::new(path).map_err(EyelinkError::CStringError)?;

    let res = unsafe { libeyelink_sys::open_data_file(c_path.as_ptr() as *mut c_char) };

    match res {
        0 => Ok(()),
        e => Err(EyelinkError::ApiError(e)),
    }
}

pub fn receive_data_file(src: &str, dst: &str) -> Result<i32, EyelinkError> {
    let c_src = CString::new(src).map_err(EyelinkError::CStringError)?;
    let c_dst = CString::new(dst).map_err(EyelinkError::CStringError)?;

    let res = unsafe {
        libeyelink_sys::receive_data_file(
            c_src.as_ptr() as *mut c_char,
            c_dst.as_ptr() as *mut c_char,
            0,
        )
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

pub fn do_tracker_setup() {
    unsafe {
        libeyelink_sys::do_tracker_setup();
    }
}

pub fn do_drift_correct(x: i16, y: i16, draw: bool, allow_setup: bool) -> Result<(), EyelinkError> {
    let res = unsafe { libeyelink_sys::do_drift_correct(x, y, draw as i16, allow_setup as i16) };
    match res {
        0 => Ok(()),
        27 => Err(EyelinkError::EscPressed),
        n => Err(EyelinkError::ApiError(n)),
    }
}

pub fn eyelink_flush_keybuttons(enable_buttons: i16) -> Result<(), EyelinkError> {
    let res = unsafe { libeyelink_sys::eyelink_flush_keybuttons(enable_buttons) };

    match res {
        0 => Ok(()),
        n => Err(EyelinkError::ApiError(n)),
    }
}

pub fn eyelink_newest_float_sample() -> Result<Box<FSAMPLE>, EyelinkError> {
    let buf = Box::new(FSAMPLE::default());
    let buf_raw = Box::into_raw(buf);

    let res = unsafe { libeyelink_sys::eyelink_newest_float_sample(buf_raw as *mut _) };
    match res {
        -1 => Err(EyelinkError::NoSampleError),
        0 => Err(EyelinkError::NoNewSampleError),
        1 => Ok(unsafe { Box::from_raw(buf_raw) }),
        n => Err(EyelinkError::ApiError(n)),
    }
}

pub fn eyelink_eye_available() -> Result<EyeData, EyelinkError> {
    let res = unsafe { libeyelink_sys::eyelink_eye_available() };

    match res {
        a if a == libeyelink_sys::RIGHT_EYE as i16 => Ok(EyeData::Right),
        a if a == libeyelink_sys::LEFT_EYE as i16 => Ok(EyeData::Left),
        a if a == libeyelink_sys::BINOCULAR as i16 => Ok(EyeData::Binocular),
        -1 => Err(EyelinkError::EyeDataError),
        n => Err(EyelinkError::ApiError(n)),
    }
}

pub fn eyelink_wait_for_block_start(
    maxwait_ms: u32,
    samples: i16,
    events: i16,
) -> Result<(), EyelinkError> {
    let res = unsafe { libeyelink_sys::eyelink_wait_for_block_start(maxwait_ms, samples, events) };
    match res {
        0 => Err(EyelinkError::BlockStartError),
        _ => Ok(()),
    }
}

pub fn start_recording(
    file_samples: bool,
    file_events: bool,
    link_samples: bool,
    link_events: bool,
) -> Result<(), EyelinkError> {
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
        n => Err(EyelinkError::ApiError(n)),
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
    use serial_test::serial;

    #[test]
    fn test_set_eyelink_address() {
        if let Ok(_) = set_eyelink_address("jibberish") {
            panic!("Should have failed.");
        }

        if let Err(_) = set_eyelink_address("100.1.1.1") {
            panic!("Should have passed.");
        }
    }

    #[test]
    #[serial]
    #[ignore] // Ignore because it fails unless connected with eyelink
    fn test_open_eyelink_connection() {
        set_eyelink_address("100.1.1.1").unwrap();
        if let Err(_) = open_eyelink_connection(OpenMode::Real) {
            panic!("Should have passed.");
        }
        close_eyelink_connection();

        // Should be fine in dummy mode.
        if let Err(_) = open_eyelink_connection(OpenMode::Dummy) {
            panic!("Should have passed.");
        }
        close_eyelink_connection();
    }

    #[test]
    #[serial]
    #[ignore] // Ignore because it fails unless connected with eyelink
    fn test_open_and_recv_data_file_connected() {
        let edf_file = "test.edf";
        set_eyelink_address("100.1.1.1").unwrap();
        if let Err(_) = open_eyelink_connection(OpenMode::Real) {
            panic!("Should have passed.");
        }
        set_offline_mode();
        flush_getkey_queue();

        match open_data_file(edf_file) {
            Ok(_) => (),
            Err(e) => {
                close_eyelink_connection();
                panic!("{}", e);
            }
        }

        // Close and transfer EDF file
        eyecmd_printf("close_data_file").unwrap();
        let conn_status = eyelink_is_connected().unwrap();
        if conn_status != ConnectionStatus::Closed {
            let size = receive_data_file(edf_file, "/tmp/test.edf").unwrap();
            assert!(size > 0, "size = {}", size);
        }

        close_eyelink_connection();
    }

    #[test]
    #[serial]
    #[ignore] // Ignore because it fails unless connected with eyelink
    fn test_eyelink_is_connected() {
        set_eyelink_address("100.1.1.1").unwrap();
        if let Err(_) = open_eyelink_connection(OpenMode::Real) {
            panic!("Should have passed.");
        }
        // Should report closed w/ no Eyelink connected
        assert_eq!(ConnectionStatus::Normal, eyelink_is_connected().unwrap());

        close_eyelink_connection();

        // Should report closed w/ no Eyelink connected
        assert_eq!(ConnectionStatus::Closed, eyelink_is_connected().unwrap());
    }

    #[test]
    fn test_eyelink_receive_data_file_disconnected() {
        // Should fail w/o an eyelink installed.
        match receive_data_file("test.edf", "") {
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
    #[serial]
    #[ignore] // Ignore because it fails unless connected with eyelink
    fn test_eyelink_get_tracker_version() {
        set_eyelink_address("100.1.1.1").unwrap();
        open_eyelink_connection(OpenMode::Real).unwrap();

        let (version, sw_version) = eyelink_get_tracker_version().unwrap();

        close_eyelink_connection();

        assert_eq!(version, 3);
        assert_eq!(sw_version, 4);
    }

    #[test]
    fn test_parse_sw_version() {
        assert_eq!(parse_sw_version("Eyelink II 4.14").unwrap(), 4);
        assert_eq!(parse_sw_version("  Eyelink I 5.0").unwrap(), 5);
        assert_eq!(parse_sw_version("Eyelink CL 4.51  ").unwrap(), 4);
    }
}
