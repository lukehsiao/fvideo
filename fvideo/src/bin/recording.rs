/// A binary for performing calibration, and then recording eye tracking data
/// for the specified amount of time while a video is played.
use std::path::PathBuf;
use std::process;

use anyhow::{anyhow, Result};
use eyelink_rs::libeyelink_sys;
use log::{error, info};
use structopt::clap::AppSettings;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(
    about,
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// Whether to run eyelink calibration or not.
    #[structopt(short, long)]
    calibrate: bool,

    /// Run in debug mode if no Eyelink is connected.
    #[structopt(short, long)]
    debug: bool,

    /// The video to play with mpv
    #[structopt(name = "VIDEO", parse(from_os_str))]
    video: PathBuf,
}

const MIN_DELAY_MS: u32 = 500;
const EDF_FILE: &str = "recording.edf";

fn end_expt(edf: &str) -> Result<()> {
    // Close and transfer EDF file
    eyelink_rs::set_offline_mode();
    eyelink_rs::msec_delay(MIN_DELAY_MS);
    eyelink_rs::eyecmd_printf("close_data_file")?;

    // Don't save the file if we aborted the experiment
    if eyelink_rs::break_pressed()? {
        eyelink_rs::close_eyelink_connection();
        return Ok(());
    }

    let conn_status = eyelink_rs::eyelink_is_connected()?;
    if conn_status != eyelink_rs::ConnectionStatus::Closed {
        let size = eyelink_rs::receive_data_file(edf)?;
        info!("Transferred {} bytes.", size);
    }

    eyelink_rs::close_eyelink_connection();
    Ok(())
}

fn initialize_eyelink(opt: &Opt) -> Result<()> {
    // Set the address of the tracker. This is hard-coded and cannot be changed.
    eyelink_rs::set_eyelink_address("100.1.1.1")?;

    if opt.debug {
        eyelink_rs::open_eyelink_connection(eyelink_rs::OpenMode::Dummy)?;
    } else {
        eyelink_rs::open_eyelink_connection(eyelink_rs::OpenMode::Real)?;
    }

    eyelink_rs::set_offline_mode();
    eyelink_rs::flush_getkey_queue();

    match eyelink_rs::open_data_file(EDF_FILE) {
        Ok(_) => (),
        Err(e) => {
            eyelink_rs::close_eyelink_connection();
            error!("{}", e);
            return Err(e.into());
        }
    }
    eyelink_rs::eyecmd_printf("add_file_preamble_text 'RECORDED BY recording.rs'")?;

    // Initialize SDL-based graphics
    let mut disp = eyelink_rs::get_display_information();
    eyelink_rs::init_expt_graphics(&mut disp)?;

    // Set display resolution
    eyelink_rs::eyecmd_printf(
        format!(
            "screen_pixel_coords = {} {} {} {}",
            disp.left, disp.top, disp.right, disp.bottom
        )
        .as_str(),
    )?;

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
        end_expt(EDF_FILE)?;
        Err(anyhow!("Eyelink is not connected."))
    } else {
        Ok(())
    }
}

/// Run a 9-point eyelink calibration
fn run_calibration() -> Result<()> {
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

    // If ESC was pressed, repeat drift correction
    loop {
        // Create screen to bg color, draw target, clear again when done, and
        // allow ESC to access setup menu before returning, rather than abort.
        match eyelink_rs::do_drift_correct(1920 / 2, 1080 / 2, true, true) {
            Err(eyelink_rs::EyelinkError::EscPressed) => continue,
            _ => break,
        }
    }

    Ok(())
}

fn start_recording() -> Result<(), i16> {
    // Give Eylink some time to switch modes in prep for recording
    eyelink_rs::set_offline_mode();
    eyelink_rs::msec_delay(50);

    // Record to EDF file and link
    eyelink_rs::start_recording(true, true, true, true)?;
    Ok(())
}

fn main() {
    pretty_env_logger::init();
    let opt = Opt::from_args();

    if let Err(e) = initialize_eyelink(&opt) {
        error!("Failed Eyelink Initialization: {}", e);
        process::exit(1);
    }

    if let Err(e) = run_calibration() {
        error!("Failed Eyelink Calibration: {}", e);
        process::exit(1);
    }

    if let Err(e) = start_recording() {
        error!("Failed starting recording: {}", e);
        process::exit(1);
    }

    if let Err(e) = end_expt(EDF_FILE) {
        error!("Failed Eyelink end_expt: {}", e);
        process::exit(1);
    }
}
