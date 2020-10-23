//! A binary for measuring e2e latency of the fvideo stack.
//!
//! Meant to be used with the eyelink-latency hardware found here:
//! <https://github.com/lukehsiao/eyelink-latency>
extern crate ffmpeg_next as ffmpeg;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::str;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use log::{debug, info};
use serialport::prelude::{DataBits, FlowControl, Parity, StopBits};
use serialport::SerialPortSettings;
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
        default_value = "Eyelink",
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

    /// Path for serial connection to ASG.
    #[structopt(short, long, default_value = "/dev/ttyACM0", parse(from_os_str))]
    serial: PathBuf,

    /// Baud rate for ASG.
    #[structopt(short, long, default_value = "115200")]
    baud: u32,

    /// Path to append results of each run.
    ///
    /// Will create file if it does not exist.
    #[structopt(short, long, default_value = "results.csv", parse(from_os_str))]
    output: PathBuf,
}

const GO_CMD: &str = "g";

fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let mut logfile = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&opt.output)?;

    // Setup serial port connection
    let s = SerialPortSettings {
        baud_rate: opt.baud,
        data_bits: DataBits::Eight,
        flow_control: FlowControl::None,
        parity: Parity::None,
        stop_bits: StopBits::One,
        timeout: Duration::from_millis(100),
    };
    let mut port = serialport::open_with_settings(&opt.serial, &s)?;

    // Sleep to give arduino time to reboot.
    // This is needed since Arduino uses DTR line to trigger a reset.
    thread::sleep(Duration::from_secs(1));

    let gaze_source = opt.gaze_source;

    let mut client = FvideoClient::new(opt.width, opt.height, gaze_source, true, None);

    let (nal_tx, nal_rx) = mpsc::channel();
    let (gaze_tx, gaze_rx) = mpsc::channel();

    // Send first sample to kick off process
    gaze_tx.send(client.gaze_sample())?;

    let now = Instant::now();
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
    let mut triggered = false;
    for nal in nal_rx {
        // Trigger ASG movement
        if !triggered && now.elapsed() > Duration::from_secs(1) {
            port.write(GO_CMD.as_bytes())?;
            info!("Triggered!");
            triggered = true;
        }

        gaze_tx.send(client.gaze_sample())?;

        // TODO(lukehsiao): Where is the ~3-6ms discrepancy from?
        let time = Instant::now();
        client.display_frame(&nal);
        debug!("Total display_frame: {:#?}", time.elapsed());
    }

    t_enc.join().unwrap()?;

    // Read the measurement from the Arduino
    let mut serial_buf: Vec<u8> = vec![0; 32];
    port.read(serial_buf.as_mut_slice())?;
    let arduino_measurement = str::from_utf8(&serial_buf)?
        .split_ascii_whitespace()
        .next()
        .unwrap();

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
    writeln!(logfile, "{}", arduino_measurement)?;
    info!("e2e latency: {} us", arduino_measurement);

    Ok(())
}
