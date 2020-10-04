extern crate ffmpeg_next as ffmpeg;

use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

use anyhow::{anyhow, Result};
use log::{debug, info};
use structopt::clap::AppSettings;
use structopt::StructOpt;

// use eyelink_rs::eyelink;
use fvideo::client::{FvideoClient, GazeSource};
use fvideo::server::{FoveationAlg, FvideoServer};

/// Make sure the qp offset option is in a valid range.
fn parse_qo_max(src: &str) -> Result<f32> {
    let qo_max = f32::from_str(src)?;
    if qo_max < 0.0 || qo_max > 81.0 {
        Err(anyhow!("QO max offset not in valid range [0, 81]."))
    } else {
        Ok(qo_max)
    }
}

#[derive(StructOpt, Debug)]
#[structopt(
    about("A tool for foveated encoding an input Y4M and decoding/displaying the results."),
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// The parameter for the size of the foveal region (0 = disable foveation).
    ///
    /// The meaning of this value depends on the Foveation Algorithm.
    #[structopt(short, long, default_value = "0")]
    fovea: u32,

    /// The method used to calculate QP offsets for foveation.
    #[structopt(short, long, default_value = "Gaussian", possible_values = &FoveationAlg::variants(), case_insensitive=true)]
    alg: FoveationAlg,

    /// Source for gaze data.
    #[structopt(
        short,
        long,
        default_value = "Mouse",
        possible_values = &GazeSource::variants(),
        case_insensitive=true,
        requires_ifs(&[("tracefile", "trace"), ("TraceFile", "trace")])
    )]
    gaze_source: GazeSource,

    /// The maximum qp offset outside of the foveal region (only range 0 to 81 valid).
    #[structopt(short, long, default_value = "35.0", parse(try_from_str = parse_qo_max))]
    qo_max: f32,

    /// The video to encode and display.
    #[structopt(name = "VIDEO", parse(from_os_str))]
    video: PathBuf,

    /// The trace file to use, if a trace file is the gaze source.
    #[structopt(short, long, parse(from_os_str))]
    trace: Option<PathBuf>,

    /// Whether to run eyelink calibration or not.
    #[structopt(short, long)]
    skip_cal: bool,
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let mut server = FvideoServer::new(opt.fovea as i32, opt.alg, opt.qo_max, opt.video)?;

    let mut client = FvideoClient::new(
        server.width(),
        server.height(),
        opt.gaze_source,
        opt.skip_cal,
        opt.trace,
    );

    let now = Instant::now();
    loop {
        let current_gaze = client.gaze_sample();

        let time = Instant::now();
        let nals = match server.encode_frame(current_gaze) {
            Ok(n) => n,
            Err(_) => break,
        };
        debug!("encode_frame: {:?} ms", time.elapsed().as_millis());

        let time = Instant::now();
        for nal in nals {
            client.display_frame(nal);
        }
        debug!("display_frame: {:?} ms", time.elapsed().as_millis());
    }

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
