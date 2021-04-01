//! A binary for the user study experiments.

extern crate ffmpeg_next as ffmpeg;

use std::path::PathBuf;
use std::{fs, process};

use anyhow::Result;
use log::{debug, info, warn};
use structopt::clap::AppSettings;
use structopt::StructOpt;

use fvideo::user_study;

#[derive(StructOpt, Debug)]
#[structopt(
    name("user_study"),
    about("The user study experiment interface."),
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// The full name of the participant.
    #[structopt(short, long)]
    name: String,

    /// The streaming proxy baseline video.
    ///
    /// Will be played using `mpv`, which must be present on the PATH.
    #[structopt(name = "BASELINE", parse(from_os_str))]
    baseline: PathBuf,

    /// The uncompressed video to encode and display.
    #[structopt(name = "VIDEO", parse(from_os_str))]
    video: PathBuf,

    /// Where to save the foveated h264 bitstream and tracefile.
    ///
    /// No output is saved unless this is specified.
    #[structopt(short, long, parse(from_os_str))]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();

    let opt = Opt::from_args();
    debug!("{:?}", opt);

    if let Some(outdir) = &opt.output {
        info!("Storing logs at: {:?}", outdir);
        if let Err(e) = fs::create_dir_all(outdir) {
            info!("{}", e);
        }
    } else {
        warn!("Not storing any logs.");
    }

    // Catch SIGINT to allow early exit.
    ctrlc::set_handler(|| {
        debug!("Exiting from SIGINT");
        process::exit(1)
    })
    .expect("Error setting Ctrl-C handler");

    user_study::run(
        &opt.name,
        &opt.baseline,
        &opt.video,
        opt.output.as_ref().map(|p| p.as_path()),
    )?;

    info!("User study complete.");

    Ok(())
}
