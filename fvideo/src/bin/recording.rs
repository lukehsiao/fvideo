/// A binary for performing calibration, and then recording eye tracking data
/// for the specified amount of time while a video is played.
use std::path::PathBuf;
use std::process;
use std::process::Command;

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
    skip_cal: bool,

    /// Run in debug mode if no Eyelink is connected.
    #[structopt(short, long)]
    debug: bool,

    /// The video to play with mpv
    #[structopt(name = "VIDEO", parse(from_os_str))]
    video: PathBuf,
}

// TODO(lukehsiao): "test.edf" works, but this breaks for unknown reasons for
// other filenames (like "recording.edf"). Not sure why.
const EDF_FILE: &str = "test.edf";

fn main() {
    pretty_env_logger::init();
    let opt = Opt::from_args();

    let mode = match opt.debug {
        true => eyelink_rs::OpenMode::Dummy,
        false => eyelink_rs::OpenMode::Real,
    };

    if let Err(e) = fvideo::initialize_eyelink(mode) {
        error!("Failed Eyelink Initialization: {}", e);
        process::exit(1);
    }

    if opt.skip_cal {
        info!("Skipping calibration.");
    } else if let Err(e) = fvideo::run_calibration() {
        error!("Failed Eyelink Calibration: {}", e);
        process::exit(1);
    }

    if let Err(e) = fvideo::start_recording(EDF_FILE) {
        error!("Failed starting recording: {}", e);
        process::exit(1);
    }

    // Play the video clip in mpv
    if let Err(e) = Command::new("mpv").arg("-fs").arg(&opt.video).status() {
        error!("Failed playing video: {}", e);
        process::exit(1);
    }

    if let Err(e) = fvideo::stop_recording(EDF_FILE) {
        error!("Failed stopping recording: {}", e);
        process::exit(1);
    }

    eyelink_rs::close_eyelink_connection();
}
