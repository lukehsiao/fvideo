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
use log::{debug, error, info};
use serialport::prelude::{DataBits, FlowControl, Parity, StopBits};
use serialport::{ClearBuffer, SerialPortSettings};
use structopt::clap::AppSettings;
use structopt::StructOpt;

// use eyelink_rs::eyelink;
use fvideo::client::{Calibrate, FvideoClient, GazeSource, Record};
use fvideo::dummyserver::{FvideoDummyServer, DIFF_THRESH};

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

    let gaze_source = opt.gaze_source;

    let mut port = match gaze_source {
        GazeSource::Eyelink => {
            // Setup serial port connection
            let s = SerialPortSettings {
                baud_rate: opt.baud,
                data_bits: DataBits::Eight,
                flow_control: FlowControl::None,
                parity: Parity::None,
                stop_bits: StopBits::One,
                timeout: Duration::from_millis(100),
            };
            Some(serialport::open_with_settings(&opt.serial, &s)?)
        }
        _ => None,
    };

    // Sleep to give arduino time to reboot.
    // This is needed since Arduino uses DTR line to trigger a reset.
    thread::sleep(Duration::from_secs(2));

    let mut client = FvideoClient::new(
        opt.width,
        opt.height,
        gaze_source,
        Calibrate::No,
        Record::No,
        None,
    );

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

            for nal in nals {
                nal_tx.send(nal)?;
            }
            debug!("Total encode_frame: {:#?}", time.elapsed());
        }
        Ok(())
    });

    // Continuously display until channel is closed.
    let mut e2e_time = Instant::now();
    let mut triggered = false;
    let mut gaze = client.gaze_sample();
    for nal in nal_rx {
        gaze_tx.send(gaze)?;
        debug!("Sent gaze.");

        let time = Instant::now();
        // TODO(lukehsiao): Where is the ~3-6ms discrepancy from?
        client.display_frame(&nal);
        debug!("Total display_frame: {:#?}", time.elapsed());

        gaze = client.gaze_sample();

        // After a delay, trigger the ASG.
        if let Some(ref mut p) = port {
            // Trigger ASG movement
            if !triggered && now.elapsed() > Duration::from_millis(1500) {
                p.clear(ClearBuffer::All)?;
                e2e_time = Instant::now();
                info!("Triggered Arduino!");
                p.write(GO_CMD.as_bytes())?;
                triggered = true;
                let time = Instant::now();
                gaze = client.triggered_gaze_sample(DIFF_THRESH);
                info!("Gaze update time: {:#?}", time.elapsed());
            }
        }
    }

    t_enc.join().unwrap()?;

    // Read the measurement from the Arduino
    if let Some(ref mut p) = port {
        let mut serial_buf: Vec<u8> = vec![0; 32];
        info!("Rust e2e time: {:#?}", e2e_time.elapsed());
        if let Err(e) = p.read(serial_buf.as_mut_slice()) {
            error!("No response from Arduino. Was the screen asleep? If so, try again in a few seconds.");
            return Err(e.into());
        }
        info!("Read response: {:#?}", e2e_time.elapsed());

        let arduino_measurement = str::from_utf8(&serial_buf)?
            .split_ascii_whitespace()
            .next()
            .unwrap();
        writeln!(logfile, "{}", arduino_measurement)?;
        let arduino_micros = match arduino_measurement.parse::<u64>() {
            Ok(p) => p,
            Err(e) => {
                error!(
                    "Unable to parse arduino measurement: {}",
                    arduino_measurement
                );
                return Err(e.into());
            }
        };
        info!("e2e latency: {:#?}", Duration::from_micros(arduino_micros));
    }

    let elapsed = now.elapsed().as_secs_f64();

    let frame_index = client.total_frames();
    let total_bytes = client.total_bytes();
    info!(
        "FPS: {}/{:.2} = {:.1}",
        frame_index,
        elapsed,
        frame_index as f64 / elapsed
    );
    info!("Total Encoded Size: {} bytes", total_bytes);

    Ok(())
}
