//! A binary for the user study experiments.
//!
//! ## Usage
//! ```
//! user_study 0.1.0
//! The user study experiment interface.
//!
//! USAGE:
//!     user_study [OPTIONS] <BASELINE> <VIDEO> --name <name>
//!
//! FLAGS:
//!     -h, --help       Prints help information
//!     -V, --version    Prints version information
//!
//! OPTIONS:
//!     -n, --name <name>        The full name of the participant
//!     -o, --output <output>    Where to save the foveated h264 bitstream and tracefile
//!
//! ARGS:
//!     <BASELINE>    The streaming proxy baseline video
//!     <VIDEO>       The uncompressed video to encode and display
//! ```

extern crate ffmpeg_next as ffmpeg;

use std::path::PathBuf;
use std::{fs, process};

use anyhow::Result;
use chrono::Utc;
use log::{debug, info, warn};
use structopt::clap::AppSettings;
use structopt::StructOpt;

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
    /// Defaults to output/%Y-%m-%d-%H-%M-%S/.
    #[structopt(short, long, parse(from_os_str))]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();

    let opt = Opt::from_args();
    debug!("{:?}", opt);

    let outdir = match &opt.output {
        None => [
            "output/",
            &Utc::now().format("%Y-%m-%d-%H-%M-%S").to_string(),
        ]
        .iter()
        .collect::<PathBuf>(),
        Some(p) => p.to_path_buf(),
    };
    info!("Storing logs at: {:?}", outdir);

    if let Err(e) = fs::create_dir_all(&outdir) {
        info!("{}", e);
    }

    // Catch SIGINT to allow early exit.
    ctrlc::set_handler(move || {
        debug!("Exiting from SIGINT");
        info!("Removing output directory...");
        if let Err(e) = fs::remove_dir_all(outdir.clone()) {
            warn!("{}", e);
        }
        process::exit(1)
    })
    .expect("Error setting Ctrl-C handler");

    println!("{}", opt.name);

    loop {}

    Ok(())
}
