/// A binary for performing calibration, and then recording eye tracking data
/// for the specified amount of time while a video is played.
use std::path::PathBuf;
use std::process;

use anyhow::{anyhow, Result};
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

    // Set display resolution
    eyelink_rs::eyecmd_printf("screen_pixel_coords = 0 0 1920 1080")?;

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
        Err(anyhow!("Eyelink is not connected."))
    } else {
        Ok(())
    }
}

/// Run a 9-point eyelink calibration
fn run_calibration() -> Result<()> {
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

    dbg!(opt);
}
