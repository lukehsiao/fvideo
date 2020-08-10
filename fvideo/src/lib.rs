#![forbid(unsafe_code)]
#![forbid(warnings)]

//! Convenient helper functions for running common Eyelink operations.

pub mod ascparser;

use eyelink_rs::{libeyelink_sys, EyelinkError};
use log::{error, info};
use thiserror::Error;

const MIN_DELAY_MS: u32 = 500;

#[derive(Error, Debug)]
pub enum FvideoError {
    #[error(transparent)]
    EyelinkError(#[from] EyelinkError),
    #[error("Unable to transfer data file: {self}")]
    TransferError(String),
    #[error("Unable to initialize SDL1.2")]
    SDLError,
}

/// Initilize a connection with the Eyelink 1000.
///
/// This also sets the Eyelink's experimental settings (e.g., saccade velocity
/// and acceleration thresholds). A more complete of commands can be found on
/// SR Research's [parser configuration page][pc].
///
/// [pc]: http://download.sr-support.com/dispdoc/cmds9.html
pub fn initialize_eyelink(mode: eyelink_rs::OpenMode) -> Result<(), FvideoError> {
    // Set the address of the tracker. This is hard-coded and cannot be changed.
    eyelink_rs::set_eyelink_address("100.1.1.1")?;

    // TODO(lukehsiao): Dummy behavior is not tested. I don't know what happens.
    eyelink_rs::open_eyelink_connection(mode)?;

    eyelink_rs::set_offline_mode();
    eyelink_rs::flush_getkey_queue();

    let (version, sw_version) = eyelink_rs::eyelink_get_tracker_version()?;

    match version {
        0 => info!("Eyelink not connected."),
        1 => {
            eyelink_rs::eyecmd_printf("saccade_velocity_threshold = 35")?;
            eyelink_rs::eyecmd_printf("saccade_acceleration_threshold = 9500")?;
        }
        2 => {
            // 0 = standard sensitivity
            eyelink_rs::eyecmd_printf("select_parser_configuration 0")?;
            eyelink_rs::eyecmd_printf("scene_camera_gazemap = NO")?;
        }
        _ => {
            // 0 = standard sensitivity
            eyelink_rs::eyecmd_printf("select_parser_configuration 0")?;
        }
    }

    // Set link data
    eyelink_rs::eyecmd_printf(
        "link_event_filter = LEFT,RIGHT,FIXATION,SACCADE,BLINK,BUTTON,INPUT",
    )?;
    if sw_version >= 4 {
        eyelink_rs::eyecmd_printf(
            "link_sample_data = LEFT,RIGHT,GAZE,GAZERES,AREA,STATUS,HTARGET,INPUT",
        )?;
    } else {
        eyelink_rs::eyecmd_printf("link_sample_data = LEFT,RIGHT,GAZE,GAZERES,AREA,STATUS,INPUT")?;
    }

    let conn_status = eyelink_rs::eyelink_is_connected()?;
    if conn_status == eyelink_rs::ConnectionStatus::Closed || eyelink_rs::break_pressed()? {
        eyelink_rs::close_eyelink_connection();
    }
    Ok(())
}

/// Run a 9-point eyelink calibration and single drift correction.
///
/// Must be called after calling [`initialize_eyelink`].
///
/// This will switch the Eyelink computer to allow you to conduct a calibration
/// using the connected display. After accepting the calibration on the Eyelink
/// PC, hitting <ESC> will move on to a single drift correct point. Hitting
/// <ENTER> after the calibration will toggle to display the camera view to the
/// user.
pub fn run_calibration() -> Result<(), FvideoError> {
    // Initialize SDL-based graphics
    match sdl::init(&[sdl::sdl::InitFlag::Video]) {
        true => (),
        false => return Err(FvideoError::SDLError),
    }
    let mut disp = eyelink_rs::get_display_information();
    eyelink_rs::init_expt_graphics(None, Some(&mut disp))?;

    // Set display resolution
    eyelink_rs::eyecmd_printf(
        format!(
            "screen_pixel_coords = {} {} {} {}",
            disp.left, disp.top, disp.right, disp.bottom
        )
        .as_str(),
    )?;
    let mut target_fg_color: libeyelink_sys::SDL_Color = libeyelink_sys::SDL_Color {
        r: 0,
        g: 0,
        b: 0,
        unused: 255,
    };
    let mut target_bg_color: libeyelink_sys::SDL_Color = libeyelink_sys::SDL_Color {
        r: 200,
        g: 200,
        b: 200,
        unused: 255,
    };

    eyelink_rs::set_calibration_colors(&mut target_fg_color, &mut target_bg_color);

    eyelink_rs::do_tracker_setup();

    // Once ESC is pressed, do a drift correction.
    // Clear screen to bg color, draw target, clear again when done, and
    // allow ESC to access setup menu before returning, rather than abort.
    while let Err(eyelink_rs::EyelinkError::EscPressed) = eyelink_rs::do_drift_correct(
        (disp.width as i16) / 2,
        (disp.height as i16) / 2,
        true,
        true,
    ) {}

    // Close graphics once we're done w/ calibration
    eyelink_rs::close_expt_graphics();

    Ok(())
}

/// Stop the eyetrace recording and transfer the file to the local machine.
///
/// TODO(lukehsiao): `edf` should be "test.edf" for now. There is an untriaged
/// bug for other file names.
pub fn stop_recording(edf: &str) -> Result<(), FvideoError> {
    // End recording
    eyelink_rs::end_realtime_mode();
    eyelink_rs::msec_delay(100);
    eyelink_rs::stop_recording();

    // Close and transfer EDF file
    eyelink_rs::set_offline_mode();
    eyelink_rs::msec_delay(MIN_DELAY_MS);
    eyelink_rs::eyecmd_printf("close_data_file")?;

    // Don't save the file if we aborted the experiment
    if eyelink_rs::break_pressed()? {
        info!("Skipping EDF transfer due to abort.");
        eyelink_rs::close_eyelink_connection();
        return Ok(());
    }

    let conn_status = eyelink_rs::eyelink_is_connected()?;
    if conn_status != eyelink_rs::ConnectionStatus::Closed {
        let size = eyelink_rs::receive_data_file(edf, edf)?;
        info!("Transferred {} bytes.", size);
        Ok(())
    } else {
        Err(FvideoError::TransferError(edf.to_string()))
    }
}

/// Start an eyetrace recording.
///
/// TODO(lukehsiao): `edf` should be "test.edf" for now. There is an untriaged
/// bug for other file names.
pub fn start_recording(edf: &str) -> Result<(), FvideoError> {
    match eyelink_rs::open_data_file(edf) {
        Ok(_) => (),
        Err(e) => {
            eyelink_rs::close_eyelink_connection();
            error!("{}", e);
            return Err(e.into());
        }
    }
    eyelink_rs::eyecmd_printf("add_file_preamble_text 'RECORDED BY recording.rs'")?;

    let (_, sw_version) = eyelink_rs::eyelink_get_tracker_version()?;

    // Set EDF file contents
    eyelink_rs::eyecmd_printf(
        "file_event_filter = LEFT,RIGHT,FIXATION,SACCADE,BLINK,MESSAGE,BUTTON,INPUT",
    )?;
    if sw_version >= 4 {
        eyelink_rs::eyecmd_printf(
            "file_sample_data = LEFT,RIGHT,GAZE,GAZERES,AREA,STATUS,HTARGET,INPUT",
        )?;
    } else {
        eyelink_rs::eyecmd_printf("file_sample_data = LEFT,RIGHT,GAZE,GAZERES,AREA,STATUS,INPUT")?;
    }

    // Give Eyelink some time to switch modes in prep for recording
    eyelink_rs::set_offline_mode();
    eyelink_rs::msec_delay(50);

    // Record to EDF file and link
    eyelink_rs::start_recording(true, true, true, true)?;

    // Start recording for a bit before displaying stimulus
    eyelink_rs::begin_realtime_mode(100);

    Ok(())
}
