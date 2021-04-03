use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use flume::{Receiver, Sender};
use log::{debug, info, warn};
use rand::prelude::*;
use serde::Deserialize;
use structopt::clap::arg_enum;
use termion::event::Key;
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::{clear, cursor};
use thiserror::Error;

use crate::client::FvideoClient;
use crate::twostreamserver::FvideoTwoStreamServer;
use crate::{
    Dims, DisplayOptions, EncodedFrames, EyelinkOptions, FoveationAlg, GazeSample, GazeSource,
    ServerCmd,
};

// - [ ] TODO(lukehsiao): How do we get keyboard events when the videos are fullscreen?
// - [ ] TODO(lukehsiao): How do we "interrupt" a currently playing video to change states?
// - [x] TODO(lukehsiao): How do we load configurations for each latency/video config? From a file?

arg_enum! {
    #[derive(Copy, Clone, Debug, PartialEq)]
    pub enum Source {
        PierSeaside,
        Barscene,
        SquareTimelapse,
        Rollercoaster,
        ToddlerFountain,
    }
}

#[derive(Debug, Deserialize, Copy, Clone)]
struct Quality {
    fg_size: u32,
    fg_crf: f32,
    bg_size: u32,
    bg_crf: f32,
}

#[derive(Debug, Deserialize, Copy, Clone)]
struct Delay {
    delay: u64,
    q1: Quality,
    q2: Quality,
    q3: Quality,
    q4: Quality,
    q5: Quality,
    q6: Quality,
    q7: Quality,
    q8: Quality,
    q9: Quality,
    q0: Quality,
}

#[derive(Debug, Deserialize)]
struct Video {
    attempts: u32,
    delays: Vec<Delay>,
}

#[derive(Error, Debug)]
pub enum UserStudyError {
    #[error("Unable to play `{0}` with mpv.")]
    MpvError(String),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    TomlError(#[from] toml::de::Error),
    #[error(transparent)]
    FvideoServerError(#[from] crate::FvideoServerError),
    #[error(transparent)]
    SendError(#[from] flume::SendError<EncodedFrames>),
}

// The set of possible user study states.
#[derive(Debug, PartialEq)]
enum State {
    Video { quality: u32 },
    Accept { quality: u32 },
    Pause { quality: u32 },
    Init,
    Calibrate,
    Baseline,
    Quit,
}

// Events that can cause state transitions
#[derive(Debug, PartialEq)]
enum Event {
    Accept,
    Quit,
    Baseline,
    Calibrate,
    Pause,
    Resume,
    Video { quality: u32 },
}

#[derive(Debug)]
struct UserStudy {
    data: Box<StateData>,
    state: State,
}

// Data available to all states
#[derive(Debug)]
struct StateData {
    start: Instant,
    delays: Vec<Delay>,
    name: String,
    baseline: PathBuf,
    video: PathBuf,
    output: Option<PathBuf>,
    client: FvideoClient,
    server: JoinHandle<Result<(), UserStudyError>>,
    nal_rx: Receiver<EncodedFrames>,
    gaze_tx: Sender<GazeSample>,
    cmd_tx: Sender<ServerCmd>,
}

impl UserStudy {
    /// Create a new UserStudy state machine.
    fn new(
        name: &str,
        source: &Source,
        output: Option<&Path>,
        settings: HashMap<String, Video>,
    ) -> Self {
        // TODO(lukehsiao): these should be configured, and not hardcoded.
        let baseline = match source {
            Source::PierSeaside => PathBuf::from("data/pierseaside.h264"),
            Source::ToddlerFountain => PathBuf::from("data/toddlerfountain.h264"),
            Source::SquareTimelapse => PathBuf::from("data/square_timelapse.h264"),
            Source::Barscene => PathBuf::from("data/barscene.h264"),
            Source::Rollercoaster => PathBuf::from("data/rollercoaster.h264"),
        };

        let video = match source {
            Source::PierSeaside => {
                PathBuf::from("~/Videos/Netflix_PierSeaside_3840x2160_60fps_yuv420p.y4m")
            }
            Source::ToddlerFountain => {
                PathBuf::from("~/Videos/Netflix_ToddlerFountain_3840x2160_60fps_yuv420p.y4m")
            }
            Source::SquareTimelapse => {
                PathBuf::from("~/Videos/Netflix_SquareAndTimelapse_3840x2160_60fps_yuv420p.y4m")
            }
            Source::Barscene => {
                PathBuf::from("~/Videos/Netflix_BarScene_3840x2160_60fps_yuv420p.y4m")
            }
            Source::Rollercoaster => {
                PathBuf::from("~/Videos/Netflix_RollerCoaster_3840x2160_60fps_yuv420p.y4m")
            }
        };

        let key = match source {
            Source::PierSeaside => "pier_seaside",
            Source::ToddlerFountain => "toddler_fountain",
            Source::SquareTimelapse => "square_timelapse",
            Source::Barscene => "barscene",
            Source::Rollercoaster => "rollercoaster",
        };

        // Queue up N attempts of each artificual delay we'll use.
        let mut delays: Vec<Delay> = vec![];
        let setting = settings.get(key).unwrap();
        for _ in 0..setting.attempts {
            for delay in &setting.delays {
                delays.push(*delay);
            }
        }
        // Shuffle to randomize order they are presented to the user
        let mut rng = rand::thread_rng();
        delays.shuffle(&mut rng);

        let (width, height, _) =
            crate::get_video_metadata(&video).expect("Unable to get video metadata.");

        // Initialize with the highest quality settings (q0).
        let fovea = delays.last().unwrap().q0.fg_size;
        let bg_width = delays.last().unwrap().q0.bg_size;
        let delay = delays.last().unwrap().delay;
        let filter = "smartblur=lr=1.0:ls=-1.0";
        let client = FvideoClient::new(
            FoveationAlg::TwoStream,
            fovea,
            Dims { width, height },
            Dims {
                width: bg_width,
                height: bg_width * 9 / 16,
            },
            DisplayOptions {
                delay,
                filter: filter.to_string(),
            },
            GazeSource::Eyelink,
            EyelinkOptions {
                calibrate: false,
                record: false,
            },
        );

        // Communication channels between client and server
        let (nal_tx, nal_rx) = flume::bounded(16);
        let (gaze_tx, gaze_rx) = flume::bounded(16);
        let (cmd_tx, _cmd_rx) = flume::bounded(5);

        let fg_crf = delays.last().unwrap().q0.fg_crf;
        let bg_crf = delays.last().unwrap().q0.bg_crf;
        let video_clone = video.clone();
        let server_hnd = thread::spawn(move || -> Result<(), UserStudyError> {
            let mut server = FvideoTwoStreamServer::new(
                fovea,
                Dims {
                    width: bg_width,
                    height: bg_width * 9 / 16,
                },
                fg_crf,
                bg_crf,
                video_clone,
            )?;
            for current_gaze in gaze_rx {
                // Only look at latest available gaze sample
                let time = Instant::now();
                let nals = match server.encode_frame(current_gaze) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                debug!("Total encode_frame: {:#?}", time.elapsed());

                nal_tx.send(nals)?;
            }
            Ok(())
        });

        let state = StateData {
            start: Instant::now(),
            delays,
            name: name.to_string(),
            baseline: baseline.to_path_buf(),
            video: video.to_path_buf(),
            output: output.map(Path::to_path_buf),
            client,
            server: server_hnd,
            nal_rx,
            gaze_tx,
            cmd_tx,
        };

        // Init state machine
        UserStudy {
            data: Box::new(state),
            state: State::Init,
        }
    }

    /// Handle state transition logic.
    fn next(self, event: Event) -> Self {
        match (&self.state, event) {
            (_, Event::Quit) => {
                info!("Quitting.");
                UserStudy {
                    data: self.data,
                    state: State::Quit,
                }
            }
            (_, Event::Calibrate) => {
                info!("Re-calibrating.");
                UserStudy {
                    data: self.data,
                    state: State::Calibrate,
                }
            }
            (_, Event::Baseline) => {
                info!("Showing baseline.");
                UserStudy {
                    data: self.data,
                    state: State::Baseline,
                }
            }
            (_, Event::Video { quality: q }) => {
                info!("Showing quality {}.", q);
                UserStudy {
                    data: self.data,
                    state: State::Video { quality: q },
                }
            }
            (State::Video { quality: q }, Event::Pause) => {
                info!("Pausing the user study.");
                UserStudy {
                    data: self.data,
                    state: State::Pause { quality: *q },
                }
            }
            (State::Pause { quality: q }, Event::Resume) => {
                info!("Resuming the user study.");
                UserStudy {
                    data: self.data,
                    state: State::Video { quality: *q },
                }
            }
            (State::Video { quality: q }, Event::Accept) => {
                info!("Choosing this quality setting.");
                UserStudy {
                    data: self.data,
                    state: State::Accept { quality: *q },
                }
            }
            (s, e) => {
                warn!("Undefined transition: {:?} and {:?}", s, e);
                self
            }
        }
    }

    /// Handle state actions.
    fn run(&mut self) -> Result<(), UserStudyError> {
        match self.state {
            State::Init => {
                info!("Moving to calibration first.");
                self.state = State::Calibrate;
            }
            State::Accept { quality: q } => {
                info!("Accepted quality: {}", q);
                if let Some(d) = self.data.delays.pop() {
                    info!("Log info for delay: {}", d.delay);
                }

                // Also state transition immediately after accept.
                if self.data.delays.is_empty() {
                    info!("All delays complete.");
                    self.state = State::Quit;
                } else {
                    info!("Paused and ready for the next delay.");
                    self.state = State::Pause { quality: 0 };
                }
            }
            State::Pause { quality: _ } => {}
            State::Calibrate => {
                info!("Run Calibration.");
            }
            State::Baseline => play_video(&self.data.baseline)?,
            State::Video { quality: q } => {
                info!("Playing video quality: {}", q);
                // for nal in nal_rx {
                //     // Send first to pipeline encode/decode, otherwise it would be in serial.
                //     gaze_tx.send(client.gaze_sample())?;
                //
                //     // TODO(lukehsiao): Where is the ~3-6ms discrepancy from?
                //     let time = Instant::now();
                //     client.display_frame(nal.0.as_ref(), nal.1.as_ref());
                //     debug!("Total display_frame: {:#?}", time.elapsed());
                //
                //     // Also save both streams to file
                //     // TODO(lukehsiao): this would probably be more useful if it was actually the
                //     // overlayed video. But for now, at least we can see both streams directly.
                //     if let Some(bg_nal) = nal.1 {
                //         outfile.write_all(bg_nal.as_bytes())?;
                //     }
                //     if let Some((fg_nal, _)) = nal.0 {
                //         fgfile.as_mut().unwrap().write_all(fg_nal.as_bytes())?;
                //     }
                // }
            }
            State::Quit => {}
        }
        Ok(())
    }
}

/// Main state machine loop
pub fn run(name: &str, source: &Source, output: Option<&Path>) -> Result<(), UserStudyError> {
    let config_file = fs::read_to_string("data/settings.toml")?;
    let settings: HashMap<String, Video> = toml::from_str(&config_file)?;

    let mut user_study = UserStudy::new(name, source, output, settings);

    // Enter raw mode to get keypresses immediately.
    //
    // This breaks the normal print macros.
    let mut stdout = io::stdout().into_raw_mode().unwrap();
    let mut stdin = termion::async_stdin().keys();

    info!("Starting state machine loop.");
    loop {
        // Read possible events
        let event = match stdin.next() {
            Some(r) => match r {
                Ok(Key::Esc) | Ok(Key::Ctrl('c')) => {
                    write!(stdout, "{}{}", clear::All, cursor::Goto(1, 1))?;
                    stdout.lock().flush()?;
                    Event::Quit
                }
                Ok(Key::Char(c)) => match c {
                    '0'..='9' => Event::Video {
                        quality: c.to_digit(10).unwrap(),
                    },
                    '\n' => Event::Accept,
                    'p' => Event::Pause,
                    'c' => Event::Calibrate,
                    'b' => Event::Baseline,
                    'r' => Event::Resume,
                    _ => continue,
                },
                Ok(_) => continue,
                Err(e) => return Err(UserStudyError::IoError(e)),
            },
            None => {
                // So we're not just burning cycles busy spinning
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };

        // Transition State
        debug!("Moving from: {:?}", user_study.state);
        user_study = user_study.next(event);
        debug!("Moving into: {:?}", user_study.state);

        // Run new state
        if let State::Quit = user_study.state {
            info!("User study is complete.");
            break;
        } else {
            user_study.run()?;
        }
    }

    Ok(())
}

/// Play a video with `mpv -fs <VIDEO>`.
fn play_video(baseline: &Path) -> Result<(), UserStudyError> {
    let status = Command::new("mpv").arg("-fs").arg(baseline).status()?;
    if !status.success() {
        Err(UserStudyError::MpvError(baseline.display().to_string()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        ()
    }
}
