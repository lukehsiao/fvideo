//! A binary for measuring e2e latency of the fvideo stack.
//!
//! Meant to be used with the eyelink-latency hardware found here:
//! <https://github.com/lukehsiao/eyelink-latency>
extern crate ffmpeg_next as ffmpeg;

use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use anyhow::Result;
use log::{debug, info};
use structopt::clap::AppSettings;
use structopt::StructOpt;

// use eyelink_rs::eyelink;
use fvideo::client::{FvideoClient, GazeSource};
use fvideo::server::FvideoDummyServer;

#[derive(StructOpt, Debug)]
#[structopt(
    about("Measure e2e latency of the fvideo stack."),
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// Source for gaze data.
    #[structopt(
        short,
        long,
        default_value = "Mouse",
        possible_values = &GazeSource::variants(),
        case_insensitive=true,
    )]
    gaze_source: GazeSource,

    /// Width of dummy input.
    #[structopt(short, long, default_value = "3840")]
    width: u32,

    /// Height of dummy input.
    #[structopt(short, long, default_value = "2160")]
    height: u32,
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let gaze_source = opt.gaze_source;

    let mut client = FvideoClient::new(opt.width, opt.height, gaze_source, true, None);

    let (nal_tx, nal_rx) = mpsc::channel();
    let (gaze_tx, gaze_rx) = mpsc::channel();

    let now = Instant::now();

    gaze_tx.send(client.gaze_sample())?;

    // Create encoder thread
    let t_enc = thread::spawn(move || -> Result<()> {
        let mut server = FvideoDummyServer::new(opt.width, opt.height)?;

        for current_gaze in gaze_rx {
            // Only look at latest available gaze sample
            let time = Instant::now();
            let nals = match server.encode_frame(current_gaze) {
                Ok(n) => n,
                Err(_) => break,
            };
            debug!("Total encode_frame: {:#?}", time.elapsed());

            for nal in nals {
                nal_tx.send(nal)?;
            }
        }
        Ok(())
    });

    // Continuously display until channel is closed.
    for nal in nal_rx {
        gaze_tx.send(client.gaze_sample())?;

        // TODO(lukehsiao): Where is the ~3-6ms discrepancy from?
        let time = Instant::now();
        client.display_frame(&nal);
        debug!("Total display_frame: {:#?}", time.elapsed());
    }

    t_enc.join().unwrap()?;

    let elapsed = now.elapsed();

    let frame_index = client.total_frames();
    let total_bytes = client.total_bytes();
    info!(
        "FPS: {}/{} = {}",
        frame_index,
        elapsed.as_secs_f64(),
        frame_index as f64 / elapsed.as_secs_f64()
    );
    info!("Total Encoded Size: {} bytes", total_bytes);

    Ok(())
}
