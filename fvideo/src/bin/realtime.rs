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

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;
use std::{fs, process, thread};

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
    if !(0.0..=81.0).contains(&qo_max) {
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

    /// Amount of artificial latency to add (ms).
    #[structopt(short, long, default_value = "0")]
    delay: u64,
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();

    // Catch SIGINT to allow early exit.
    ctrlc::set_handler(|| {
        debug!("Exiting from SIGINT");
        process::exit(1)
    })
    .expect("Error setting Ctrl-C handler");

    let opt = Opt::from_args();

    let gaze_source = opt.gaze_source;

    let (width, height, _) = fvideo::get_video_metadata(&opt.video)?;

    let mut client = FvideoClient::new(
        opt.alg,
        opt.fovea,
        width,
        height,
        opt.delay,
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

    let cfg_dest: PathBuf = [&outdir, &PathBuf::from("config.csv")].iter().collect();
    let mut cfg_dest = BufWriter::new(fs::File::create(cfg_dest)?);
    writeln!(
        cfg_dest,
        "alg,fovea,qo_max,gaze_source,video,frames,elapsed_time,fps,filesize_bytes",
    )?;
    write!(
        cfg_dest,
        "{},{},{},{},{},",
        opt.alg,
        opt.fovea,
        opt.qo_max,
        opt.gaze_source,
        opt.video.display()
    )?;

    let outfile: PathBuf = [&outdir, &PathBuf::from("video.h264")].iter().collect();
    let mut outfile = BufWriter::new(fs::File::create(outfile)?);

    let mut fgfile = match opt.alg {
        FoveationAlg::TwoStream => {
            let tmp: PathBuf = [&outdir, &PathBuf::from("foreground.h264")]
                .iter()
                .collect();
            Some(BufWriter::new(fs::File::create(tmp)?))
        }
        _ => None,
    };

    let (nal_tx, nal_rx) = flume::bounded(16);
    let (gaze_tx, gaze_rx) = flume::bounded(16);

    let now = Instant::now();

    // Prime with real gaze samples
    client.gaze_sample();
    gaze_tx.send(client.gaze_sample())?;

    // Create server thread
    let record = opt.record;
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
                // Send first to pipeline encode/decode, otherwise it would be in serial.
                gaze_tx.send(client.gaze_sample())?;

                // TODO(lukehsiao): Where is the ~3-6ms discrepancy from?
                let time = Instant::now();
                client.display_frame(nal.0.as_ref(), nal.1.as_ref());
                debug!("Total display_frame: {:#?}", time.elapsed());

                // Also save both streams to file
                // TODO(lukehsiao): this would probably be more useful if it was
                // actually the overlayed video. But for now, at least we can
                // see both streams directly.
                if let Some(bg_nal) = nal.1 {
                    outfile.write_all(bg_nal.as_bytes())?;
                }
                if let Some(fg_nal) = nal.0 {
                    fgfile.as_mut().unwrap().write_all(fg_nal.as_bytes())?;
                }
            }
        }
        _ => {
            for nal in nal_rx {
                // Send first to pipeline encode/decode, otherwise it would be in serial.
                gaze_tx.send(client.gaze_sample())?;

                let time = Instant::now();
                client.display_frame(None, nal.1.as_ref());
                debug!("Total display_frame: {:#?}", time.elapsed());

                // Also save to file
                outfile.write_all(nal.1.as_ref().unwrap().as_bytes())?;
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

    write!(
        cfg_dest,
        "{},{},{},{}",
        frame_index,
        elapsed.as_secs_f32(),
        frame_index as f32 / elapsed.as_secs_f32(),
        total_bytes,
    )?;

    // TODO(lukehsiao): This is kind of hack-y. Should probably have the client
    // do this.
    if GazeSource::Eyelink == gaze_source && record {
        let edf_dest: PathBuf = [&outdir, &PathBuf::from("eyetrace.edf")].iter().collect();
        if let Err(e) = fs::rename(EDF_FILE, edf_dest) {
            warn!("{}", e);
        }
    }

    Ok(())
}
