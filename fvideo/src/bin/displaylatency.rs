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
use serialport::prelude::{DataBits, FlowControl, Parity, StopBits};
use serialport::{ClearBuffer, SerialPortSettings};
use structopt::clap::AppSettings;
use structopt::StructOpt;

use fvideo::client::{Calibrate, FvideoClient, GazeSource, Record};
use fvideo::dummyserver::DIFF_THRESH;

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

    /// Width of dummy input.
    #[structopt(short, long, default_value = "1920")]
    width: u32,

    /// Height of dummy input.
    #[structopt(short, long, default_value = "1080")]
    height: u32,

    /// Path for serial connection to ASG.
    #[structopt(short, long, default_value = "/dev/ttyACM0", parse(from_os_str))]
    serial: PathBuf,

    /// Baud rate for ASG.
    #[structopt(short, long, default_value = "115200")]
    baud: u32,
}

const GO_CMD: &str = "g";

fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let mut port = match opt.gaze_source {
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

    let mut client = FvideoClient::new(
        opt.width,
        opt.height,
        opt.gaze_source,
        Calibrate::No,
        Record::No,
        None,
    );

    // Sleep to give arduino time to reboot.
    // This is needed since Arduino uses DTR line to trigger a reset.
    thread::sleep(Duration::from_secs(2));

    client.clear();

    // Trigger the ASG.
    client.gaze_sample();
    let mut e2e_time = Instant::now();
    if let Some(ref mut p) = port {
        p.clear(ClearBuffer::All)?;
        e2e_time = Instant::now();
        info!("Triggered Arduino!");
        p.write(GO_CMD.as_bytes())?;
        let time = Instant::now();
        client.triggered_gaze_sample(DIFF_THRESH);
        info!("Gaze update time: {:#?}", time.elapsed());
    }

    client.display_white(opt.height, opt.width / 19);

    info!("Rust e2e time: {:#?}", e2e_time.elapsed());

    // Read the measurement from the Arduino
    if let Some(ref mut p) = port {
        let mut serial_buf: Vec<u8> = vec![0; 32];
        if let Err(e) = p.read(serial_buf.as_mut_slice()) {
            error!("No response from Arduino. Was the screen asleep? If so, try again in a few seconds.");
            return Err(e.into());
        }

        let arduino_measurement = str::from_utf8(&serial_buf)?
            .split_ascii_whitespace()
            .next()
            .unwrap();
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

    Ok(())
}
