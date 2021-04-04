//! A binary for the user study experiments.

extern crate ffmpeg_next as ffmpeg;

use std::path::PathBuf;
use std::{fs, process};

use anyhow::Result;
use log::{debug, info, warn};
use structopt::clap::AppSettings;
use structopt::StructOpt;

use fvideo::user_study::{self, Source};

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

    /// Source for gaze data.
    #[structopt(
        name = "SOURCE",
        possible_values = &Source::variants(),
        case_insensitive=true,
    )]
    source: Source,

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

    user_study::run(&opt.name, &opt.source, opt.output.as_deref())?;

    info!("User study complete.");

    Ok(())
}
