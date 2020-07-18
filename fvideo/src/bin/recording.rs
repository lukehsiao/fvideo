/// A binary for performing calibration, and then recording eye tracking data
/// for the specified amount of time while a video is played.
use std::ffi::CString;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use log::info;
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

    unsafe {
        // Must be at least length 40 per eyelink api
        let mut version_str: [i8; 40] = [0; 40];
        let version = libeyelink_sys::eyelink_get_tracker_version(version_str.as_mut_ptr());

        match version {
            0 => info!("Eyelink not connected."),
            1 => {
                info!("Settings for Eyelink I...");
                let cmd = CString::new("saccade_velocity_threshold = 35").unwrap();
                match libeyelink_sys::eyecmd_printf(cmd.as_ptr()) {
                    0 => (),
                    _ => return Err(anyhow!("Unable to set saccade_velocity_threshold.")),
                }
                let cmd = CString::new("saccade_acceleration_threshold = 9500").unwrap();
                match libeyelink_sys::eyecmd_printf(cmd.as_ptr()) {
                    0 => (),
                    _ => return Err(anyhow!("Unable to set saccade_acceleration_threshold.")),
                }
            }
            2 => {
                // 0 = standard sensitivity
                let cmd = CString::new("select_parser_configuration 0").unwrap();
                match libeyelink_sys::eyecmd_printf(cmd.as_ptr()) {
                    0 => (),
                    _ => return Err(anyhow!("Unable to set select_parser_configuration.")),
                }
                let cmd = CString::new("scene_camera_gazemap = NO").unwrap();
                match libeyelink_sys::eyecmd_printf(cmd.as_ptr()) {
                    0 => (),
                    _ => return Err(anyhow!("Unable to set scene_camera_gazemap.")),
                }
            }
            _ => {
                // 0 = standard sensitivity
                let cmd = CString::new("select_parser_configuration 0").unwrap();
                match libeyelink_sys::eyecmd_printf(cmd.as_ptr()) {
                    0 => (),
                    _ => return Err(anyhow!("Unable to set select_parser_configuration.")),
                }
            }
        }

        let cmd =
            CString::new("link_event_filter = LEFT,RIGHT,FIXATION,SACCADE,BLINK,BUTTON,INPUT")
                .unwrap();
        match libeyelink_sys::eyecmd_printf(cmd.as_ptr()) {
            0 => (),
            _ => return Err(anyhow!("Unable to set select_parser_configuration.")),
        }
    }

    Ok(())
}

/// Run a 9-point eyelink calibration
fn run_calibration() {}

fn main() {
    pretty_env_logger::init();
    let opt = Opt::from_args();

    initialize_eyelink(&opt);

    dbg!(opt);
}
