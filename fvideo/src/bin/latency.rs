//! A binary for measuring e2e latency of the full fvideo stack.
//!
//! Meant to be used with the eyelink-latency hardware found here:
//! <https://github.com/lukehsiao/eyelink-latency>
extern crate ffmpeg_next as ffmpeg;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{process, str, thread};

use anyhow::Result;
use log::{debug, error, info, warn};
use serialport::{ClearBuffer, DataBits, FlowControl, Parity, StopBits};
use structopt::clap::AppSettings;
use structopt::StructOpt;

// use eyelink_rs::eyelink;
use fvideo::client::FvideoClient;
use fvideo::dummyserver::{FvideoDummyServer, FvideoDummyTwoStreamServer, DIFF_THRESH};
use fvideo::{Dims, DisplayOptions, EyelinkOptions, FoveationAlg, GazeSource};

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

    /// The parameter for the size of the foveal region (0 = disable foveation).
    ///
    /// The meaning of this value depends on the Foveation Algorithm.
    /// TODO(lukehsiao): explain the differences.
    #[structopt(short, long, default_value = "30")]
    fovea: u32,

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

    /// Baud rate for ASG.
    #[structopt(short, long, default_value = "115200")]
    baud: u32,

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

    /// How many times to run the experiment
    #[structopt(short, long, default_value = "1")]
    trials: u32,
}

const GO_CMD: &str = "g";

fn main() -> Result<()> {
    pretty_env_logger::init_timed();
    ffmpeg::init().unwrap();
    // Catch SIGINT to allow early exit.
    ctrlc::set_handler(|| {
        debug!("Exiting from SIGINT");
        process::exit(1)
    })
    .expect("Error setting Ctrl-C handler");

    let opt = Opt::from_args();

    let gaze_source = opt.gaze_source;

    let mut port = match gaze_source {
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

            Some(p)
        }
        _ => None,
    };

    let mut client = FvideoClient::new(
        opt.alg,
        opt.fovea,
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
            filter: opt.filter.clone(),
        },
        gaze_source,
        EyelinkOptions {
            calibrate: false,
            record: false,
        },
    );

    // Create encoder thread
    let alg = opt.alg;
    let width = opt.width;
    let height = opt.height;
    let bg_width = opt.bg_width;
    let fovea = opt.fovea;

    println!("e2e_us");
    let mut count = opt.trials;
    while count > 0 {
        let (nal_tx, nal_rx) = mpsc::channel();
        let (gaze_tx, gaze_rx) = mpsc::channel();

        // Send first sample to kick off process
        gaze_tx.send(client.gaze_sample())?;

        let now = Instant::now();

        let t_enc = match alg {
            FoveationAlg::TwoStream => {
                thread::spawn(move || -> Result<()> {
                    let mut server = FvideoDummyTwoStreamServer::new(
                        Dims { width, height },
                        Dims {
                            width: bg_width,
                            height: bg_width * 9 / 16,
                        },
                        fovea,
                    )?;

                    for current_gaze in gaze_rx {
                        // Only look at latest available gaze sample
                        let time = Instant::now();
                        let nals = match server.encode_frame(current_gaze) {
                            Ok(n) => n,
                            Err(_) => break,
                        };

                        nal_tx.send(nals)?;

                        if server.triggered() {
                            info!("Total encode_frame: {:#?}", time.elapsed());
                        } else {
                            debug!("Total encode_frame: {:#?}", time.elapsed());
                        }
                    }
                    Ok(())
                })
            }
            _ => {
                thread::spawn(move || -> Result<()> {
                    let mut server = FvideoDummyServer::new(width, height)?;

                    for current_gaze in gaze_rx {
                        // Only look at latest available gaze sample
                        let time = Instant::now();
                        let nals = match server.encode_frame(current_gaze) {
                            Ok(n) => n,
                            Err(_) => break,
                        };

                        nal_tx.send(nals)?;

                        if server.triggered() {
                            info!("Total encode_frame: {:#?}", time.elapsed());
                        } else {
                            debug!("Total encode_frame: {:#?}", time.elapsed());
                        }
                    }
                    Ok(())
                })
            }
        };

        // Continuously display until channel is closed.
        let mut triggered = false;
        match alg {
            FoveationAlg::TwoStream => {
                for nal in nal_rx {
                    let mut gaze = client.gaze_sample();
                    // After a delay, trigger the ASG.
                    if let Some(ref mut p) = port {
                        // Trigger ASG movement
                        if !triggered && now.elapsed() > Duration::from_millis(1500) {
                            p.clear(ClearBuffer::All)?;
                            info!("Triggered Arduino!");
                            p.write_all(GO_CMD.as_bytes())?;
                            triggered = true;
                            let time = Instant::now();
                            gaze = Some(client.triggered_gaze_sample(DIFF_THRESH));
                            info!("Gaze update time: {:#?}", time.elapsed());
                        }
                    }
                    gaze_tx.send(gaze)?;
                    debug!("Sent gaze.");

                    let time = Instant::now();
                    client.display_frame(nal.0.as_ref(), nal.1.as_ref());
                    if triggered {
                        info!("Total display_frame: {:#?}", time.elapsed());
                    } else {
                        debug!("Total display_frame: {:#?}", time.elapsed());
                    }
                }
            }
            _ => {
                for nal in nal_rx {
                    let mut gaze = client.gaze_sample();

                    // After a delay, trigger the ASG and send the updated gaze sample immediately
                    if let Some(ref mut p) = port {
                        // Trigger ASG movement
                        if !triggered && now.elapsed() > Duration::from_millis(1500) {
                            p.clear(ClearBuffer::All)?;
                            info!("Triggered Arduino!");
                            p.write_all(GO_CMD.as_bytes())?;
                            triggered = true;
                            let time = Instant::now();
                            gaze = Some(client.triggered_gaze_sample(DIFF_THRESH));
                            info!("Gaze update time: {:#?}", time.elapsed());
                        }
                    }
                    gaze_tx.send(gaze)?;
                    debug!("Sent gaze.");

                    let time = Instant::now();
                    client.display_frame(None, nal.1.as_ref());
                    if triggered {
                        info!("Total display_frame: {:#?}", time.elapsed());
                    } else {
                        debug!("Total display_frame: {:#?}", time.elapsed());
                    }
                }
            }
        }

        t_enc.join().unwrap()?;

        // Read the measurement from the Arduino
        if let Some(ref mut p) = port {
            let mut serial_buf: Vec<u8> = vec![0; 32];
            if let Err(e) = p.read(serial_buf.as_mut_slice()) {
                error!("No response from Arduino. Was the screen asleep? If so, try again in a few seconds.");
                return Err(e.into());
            }

            let arduino_measurement = str::from_utf8(&serial_buf)?
                .trim_matches(char::from(0))
                .trim();

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
            println!("{}", arduino_measurement);
            info!("e2e latency: {:#?}", Duration::from_micros(arduino_micros));
            count -= 1;
        } else {
            warn!("e2e latency unavailable w/o ASG.");
            count -= 1;
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

        client.clear();
        thread::sleep(Duration::from_millis(150));
    }

    Ok(())
}
