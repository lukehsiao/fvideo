//! Thin, safe wrappers around libeyelink-sys.

use std::ffi::CString;
use std::mem::MaybeUninit;
use std::os::raw::c_char;

use c_fixed_string::CFixedStr;
use thiserror::Error;

pub use libeyelink_sys;

#[derive(Error, Debug)]
/// Wrapper for all errors working with libeyelink-sys.
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
    #[error(transparent)]
    Utf8Error(#[from] std::str::Utf8Error),
    #[error(
        "eyelink-rs received an undocumented return value from libeyelink_sys: {}",
        self
    )]
    APIError(i16),
    #[error("Data File Error {code}: {msg}")]
    DataError { code: i32, msg: String },
    #[error("SDL Error: [{code}] {msg}")]
    SDLError { code: i32, msg: String },
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
    let c_addr = CString::new(addr).map_err(EyelinkError::CStringError)?;
    unsafe {
        match libeyelink_sys::set_eyelink_address(c_addr.as_ptr() as *mut c_char) {
            0 => Ok(()),
            _ => Err(EyelinkError::InvalidIP(addr.into())),
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
            n => Err(EyelinkError::APIError(n)),
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
    let major = parse_sw_version(sw_version).map_err(|_| EyelinkError::APIError(0))?;

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
    let c_path = CString::new(path).map_err(EyelinkError::CStringError)?;

    let res = unsafe { libeyelink_sys::open_data_file(c_path.as_ptr() as *mut c_char) };

    match res {
        0 => Ok(()),
        e => Err(EyelinkError::APIError(e)),
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

/// Initialize Eyelink's SDL-based experimental graphics.
///
/// **Warning**: In our experience, calling this function can cause a segfault
/// from the underlying eyelink libraries. To avoid this, you should initialize
/// SDL yourself first.
/// ```ignore
/// match sdl::init(&[sdl::sdl::InitFlag::Video]) {
///     true => (),
///     false => return Err(()),
/// }
/// eyelink_rs::init_expt_graphics(None, None)?;
/// ```
pub fn init_expt_graphics(
    hwnd: Option<&mut libeyelink_sys::SDL_Surface>,
    info: Option<&mut libeyelink_sys::DISPLAYINFO>,
) -> Result<(), EyelinkError> {
    let hwnd = match hwnd {
        Some(s) => s,
        None => std::ptr::null_mut(),
    };
    let info = match info {
        Some(s) => s,
        None => std::ptr::null_mut(),
    };
    unsafe {
        match libeyelink_sys::init_expt_graphics(hwnd, info) {
            0 => Ok(()),
            e => Err(EyelinkError::APIError(e)),
        }
    }
}

pub fn close_expt_graphics() {
    unsafe { libeyelink_sys::close_expt_graphics() }
}

/// Get display information using Eyelink's SDL-based library.
///
/// **Warning**: In our experience, calling this function can cause a segfault
/// from the underlying eyelink libraries. To avoid this, you should initialize
/// SDL yourself first.
/// ```ignore
/// match sdl::init(&[sdl::sdl::InitFlag::Video]) {
///     true => (),
///     false => return Err(()),
/// }
/// let disp = eyelink_rs::get_display_information();
/// ```
pub fn get_display_information() -> libeyelink_sys::DISPLAYINFO {
    unsafe {
        let mut info: MaybeUninit<libeyelink_sys::DISPLAYINFO> = MaybeUninit::uninit();
        libeyelink_sys::get_display_information(info.as_mut_ptr());
        info.assume_init()
    }
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
        n => Err(EyelinkError::APIError(n)),
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

    #[test]
    #[ignore] // Ignore because it fails unless connected with a display
    fn test_get_display_information() {
        let info = get_display_information();
        assert_eq!(info.left, 0);
        assert_eq!(info.right, 1919);
        assert_eq!(info.top, 0);
        assert_eq!(info.bottom, 1079);
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert_eq!(info.bits, 32);
        assert_eq!(info.palsize, 0);
        assert_eq!(info.palrsvd, 0);
        assert_eq!(info.pages, 1);
        assert_eq!(info.refresh, 60.0);
        assert_eq!(info.winnt, -1);
    }

    #[test]
    #[serial]
    #[ignore] // Ignore because it fails unless connected with a display
    fn test_init_expt_graphics() {
        set_eyelink_address("100.1.1.1").unwrap();
        open_eyelink_connection(OpenMode::Real).unwrap();
        set_offline_mode();
        flush_getkey_queue();

        let mut disp = libeyelink_sys::DISPLAYINFO {
            left: 0,
            right: 1919,
            top: 0,
            bottom: 1079,
            width: 1920,
            height: 1080,
            bits: 32,
            palsize: 0,
            palrsvd: 0,
            pages: 1,
            refresh: 60.0,
            winnt: -1,
        };
        init_expt_graphics(None, Some(&mut disp)).unwrap();

        close_expt_graphics();
        close_eyelink_connection();
    }
}
