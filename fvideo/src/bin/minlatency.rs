//! A binary for measuring just the minimum possible display latency.
//!
//! Meant to be used with the eyelink-latency hardware found here:
//! <https://github.com/lukehsiao/eyelink-latency>
extern crate ffmpeg_next as ffmpeg;

use std::io::Write;
use std::path::PathBuf;
use std::str;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use log::{error, info};
use serialport::{ClearBuffer, DataBits, FlowControl, Parity, StopBits};
use structopt::clap::AppSettings;
use structopt::StructOpt;

use fvideo::client::FvideoClient;
use fvideo::dummyserver::DIFF_THRESH;
use fvideo::{Dims, DisplayOptions, EyelinkOptions, FoveationAlg, GazeSource};

#[derive(StructOpt, Debug)]
#[structopt(
    about("Measure minimum motion-to-photon latency."),
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// Source for gaze data.
    #[structopt(
        short,
        long,
        default_value = "Eyelink",
        possible_values = &GazeSource::variants(),
        case_insensitive=true,
    )]
    gaze_source: GazeSource,

    /// The method used for foveation.
    #[structopt(short, long, default_value = "Gaussian", possible_values = &FoveationAlg::variants(), case_insensitive=true)]
    alg: FoveationAlg,

    /// Width of dummy input.
    #[structopt(short, long, default_value = "3840")]
    width: u32,

    /// Height of dummy input.
    #[structopt(short, long, default_value = "2160")]
    height: u32,

    /// Path for serial connection to ASG.
    #[structopt(short, long, default_value = "/dev/ttyACM0", parse(from_os_str))]
    serial: PathBuf,

    /// Width to rescale the background video stream.
    ///
    /// Both width and height must be a multiple of 16 (the size of a macroblock). Height will
    /// automatically be calculated to keep a 16:9 ratio. Only used by the TwoStream foveation
    /// algorithm.
    #[structopt(short, long, default_value = "512")]
    bg_width: u32,

    /// FFmpeg-style filter to apply to the decoded bg frames.
    #[structopt(short, long, default_value = "smartblur=lr=1.0:ls=-1.0")]
    filter: String,

    /// Baud rate for ASG.
    #[structopt(short, long, default_value = "115200")]
    baud: u32,

    /// How many times to run the experiment
    #[structopt(short, long, default_value = "1")]
    trials: u32,
}

const GO_CMD: &str = "g";

fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let mut port = match opt.gaze_source {
        GazeSource::Eyelink => {
            // Setup serial port connection
            let p = serialport::new(opt.serial.to_str().unwrap(), opt.baud)
                .data_bits(DataBits::Eight)
                .flow_control(FlowControl::None)
                .parity(Parity::None)
                .stop_bits(StopBits::One)
                .timeout(Duration::from_millis(100))
                .open()?;

            // Sleep to give arduino time to reboot.
            // This is needed since Arduino uses DTR line to trigger a reset.
            thread::sleep(Duration::from_secs(3));

            p.clear(ClearBuffer::All)?;

            Some(p)
        }
        _ => None,
    };

    let mut client = FvideoClient::new(
        opt.alg,
        1,
        Dims {
            width: opt.width,
            height: opt.height,
        },
        Dims {
            width: opt.bg_width,
            height: opt.bg_width * 9 / 16,
        },
        DisplayOptions {
            delay: 0,
            filter: opt.filter,
        },
        opt.gaze_source,
        EyelinkOptions {
            calibrate: false,
            record: false,
        },
    );
    client.gaze_sample();

    // Toggle a couple times to get these in cache.
    for _ in 0..3 {
        client.clear();
        thread::sleep(Duration::from_millis(100));
        client.display_white(opt.height, opt.width / 19);
        thread::sleep(Duration::from_millis(100));
    }

    println!("e2e_us");
    for _ in 0..opt.trials {
        client.clear();
        thread::sleep(Duration::from_millis(100));

        // Trigger the ASG.
        let mut e2e_time = Instant::now();
        if let Some(ref mut p) = port {
            e2e_time = Instant::now();
            info!("Triggered Arduino!");
            p.write_all(GO_CMD.as_bytes())?;
            let time = Instant::now();
            client.triggered_gaze_sample(DIFF_THRESH);
            info!("Gaze update time: {:#?}", time.elapsed());
        }

        let now = Instant::now();
        client.display_white(opt.height, opt.width / 19);
        info!("rust draw time: {:#?}", now.elapsed());

        // Read the measurement from the Arduino
        if let Some(ref mut p) = port {
            let mut serial_buf: Vec<u8> = vec![0; 32];
            if let Err(e) = p.read(serial_buf.as_mut_slice()) {
                error!("No response from Arduino. Was the screen asleep? If so, try again in a few seconds.");
                return Err(e.into());
            }

            info!("Rust e2e time: {:#?}", e2e_time.elapsed());
            let arduino_measurement = str::from_utf8(&serial_buf)?
                .split_ascii_whitespace()
                .next()
                .unwrap();
            info!("arduino latency: {}µs", arduino_measurement);
            println!("{}", arduino_measurement);
        }
    }

    Ok(())
}
