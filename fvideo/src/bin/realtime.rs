//! A binary for real-time foveated video encoding and display.
//!
//! # Usage
//! ```
//! realtime 0.1.0
//! A tool for foveated encoding an input Y4M and decoding/displaying the results.
//!
//! USAGE:
//!     realtime [FLAGS] [OPTIONS] <VIDEO>
//!
//! FLAGS:
//!     -h, --help
//!             Prints help information
//!
//!     -r, --record
//!             Whether to record an eye trace or not
//!
//!     -s, --skip-cal
//!             Whether to run eyelink calibration or not
//!
//!     -V, --version
//!             Prints version information
//!
//!
//! OPTIONS:
//!     -a, --alg <alg>
//!             The method used to calculate QP offsets for foveation [default: Gaussian]  [possible values:
//!             SquareStep, Gaussian, TwoStream]
//!     -f, --fovea <fovea>
//!             The parameter for the size of the foveal region (0 = disable foveation).
//!
//!             The meaning of this value depends on the Foveation Algorithm. [default: 0]
//!     -g, --gaze-source <gaze-source>
//!             Source for gaze data [default: Mouse]  [possible values: Mouse, Eyelink,
//!             TraceFile]
//!     -o, --output <output>
//!             Where to save the foveated h264 bitstream and tracefile.
//!
//!             Defaults to output/%Y-%m-%d-%H-%M-%S/.
//!     -q, --qo-max <qo-max>
//!             The maximum qp offset outside of the foveal region (only range 0 to 81 valid) [default: 35.0]
//!
//!     -t, --trace <trace>
//!             The trace file to use, if a trace file is the gaze source
//!
//!
//! ARGS:
//!     <VIDEO>
//!             The video to encode and display
//! ```
extern crate ffmpeg_next as ffmpeg;

use std::fs;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use anyhow::{anyhow, Result};
use chrono::Utc;
use log::{debug, info, warn};
use structopt::clap::AppSettings;
use structopt::StructOpt;

use fvideo::client::FvideoClient;
use fvideo::server::FvideoServer;
use fvideo::twostreamserver::FvideoTwoStreamServer;
use fvideo::{Calibrate, FoveationAlg, GazeSource, Record, EDF_FILE};

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
    name("realtime"),
    about("A tool for foveated encoding an input Y4M and decoding/displaying the results."),
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// The parameter for the size of the foveal region (0 = disable foveation).
    ///
    /// The meaning of this value depends on the Foveation Algorithm.
    /// TODO(lukehsiao): explain the differences.
    #[structopt(short, long, default_value = "1")]
    fovea: u32,

    /// The method used for foveation.
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

    /// Where to save the foveated h264 bitstream and tracefile.
    ///
    /// Defaults to output/%Y-%m-%d-%H-%M-%S/.
    #[structopt(short, long, parse(from_os_str))]
    output: Option<PathBuf>,

    /// Whether to run eyelink calibration or not.
    #[structopt(short, long)]
    skip_cal: bool,

    /// Whether to record an eye trace or not.
    #[structopt(short, long)]
    record: bool,
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let gaze_source = opt.gaze_source;

    let (width, height, _) = fvideo::get_video_metadata(&opt.video)?;

    let mut client = FvideoClient::new(
        opt.alg,
        opt.fovea,
        width,
        height,
        gaze_source,
        if opt.skip_cal {
            Calibrate::No
        } else {
            Calibrate::Yes
        },
        if opt.record { Record::Yes } else { Record::No },
        opt.trace.clone(),
    );

    let outdir = match &opt.output {
        None => [
            "output/",
            &Utc::now().format("%Y-%m-%d-%H-%M-%S").to_string(),
        ]
        .iter()
        .collect::<PathBuf>(),
        Some(p) => p.to_path_buf(),
    };
    if let Err(e) = fs::create_dir_all(&outdir) {
        info!("{}", e);
    }

    let outfile: PathBuf = [&outdir, &PathBuf::from("video.h264")].iter().collect();
    let mut outfile = BufWriter::new(fs::File::create(outfile)?);

    let (nal_tx, nal_rx) = mpsc::channel();
    let (gaze_tx, gaze_rx) = mpsc::channel();

    let now = Instant::now();

    gaze_tx.send(client.gaze_sample())?;

    // Create server thread
    let alg_clone = opt.alg;
    let t_enc = match opt.alg {
        FoveationAlg::TwoStream => {
            thread::spawn(move || -> Result<()> {
                let mut server = FvideoTwoStreamServer::new(opt.fovea, opt.video.clone())?;

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
            })
        }
        _ => {
            thread::spawn(move || -> Result<()> {
                let mut server =
                    FvideoServer::new(opt.fovea as i32, opt.alg, opt.qo_max, opt.video.clone())?;

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
            })
        }
    };

    // Continuously display until channel is closed.
    match alg_clone {
        FoveationAlg::TwoStream => {
            for nal in nal_rx {
                gaze_tx.send(client.gaze_sample())?;

                // TODO(lukehsiao): Where is the ~3-6ms discrepancy from?
                let time = Instant::now();
                client.display_frame(nal.0.as_ref().unwrap(), &nal.1);
                debug!("Total display_frame: {:#?}", time.elapsed());

                // Also save to file
                outfile.write_all(nal.0.as_ref().unwrap().as_bytes())?;
            }
        }
        _ => {
            for nal in nal_rx {
                gaze_tx.send(client.gaze_sample())?;

                // TODO(lukehsiao): Where is the ~3-6ms discrepancy from?
                let time = Instant::now();
                client.display_frame(None, &nal.1);
                debug!("Total display_frame: {:#?}", time.elapsed());

                // Also save to file
                outfile.write_all(nal.1.as_bytes())?;
            }
        }
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
    info!("Total Encoded Size: {} bytes\n", total_bytes);

    // TODO(lukehsiao): This is kind of hack-y. Should probably have the client
    // do this.
    if let GazeSource::Eyelink = gaze_source {
        let edf_dest: PathBuf = [&outdir, &PathBuf::from("eyetrace.edf")].iter().collect();
        if let Err(e) = fs::rename(EDF_FILE, edf_dest) {
            warn!("{}", e);
        }
    }

    Ok(())
}
