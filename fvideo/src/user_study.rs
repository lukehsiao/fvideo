use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use log::{debug, info, warn};
use rand::prelude::*;
use sdl2::event::EventType;
use sdl2::keyboard::Keycode;
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
};

// - [ ] TODO(lukehsiao): What exactly do we log?
// - [x] TODO(lukehsiao): How do we get keyboard events when the videos are fullscreen?
// - [x] TODO(lukehsiao): How do we "interrupt" a currently playing video to change states?
// - [x] TODO(lukehsiao): How do we load configurations for each latency/video config? From a file?

#[derive(Debug)]
pub enum ServerCmd {
    Start,
    Stop,
}

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
    #[error("{0} is not a valid quality setting.")]
    InvalidQuality(u32),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    TomlError(#[from] toml::de::Error),
    #[error(transparent)]
    FvideoServerError(#[from] crate::FvideoServerError),
    #[error(transparent)]
    SendNalError(#[from] flume::SendError<EncodedFrames>),
    #[error(transparent)]
    SendGazeError(#[from] flume::SendError<GazeSample>),
    #[error(transparent)]
    SendCmdError(#[from] flume::SendError<ServerCmd>),
}

// The set of possible user study states.
#[derive(Debug, PartialEq, Clone, Copy)]
enum State {
    Video { quality: u32 },
    Accept { quality: u32 },
    Pause,
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
    None,
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
    key: String,
    output: Option<PathBuf>,
    log: File,
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
            Source::PierSeaside => PathBuf::from(
                "/home/lukehsiao/Videos/Netflix_PierSeaside_3840x2160_60fps_yuv420p.y4m",
            ),
            Source::ToddlerFountain => PathBuf::from(
                "/home/lukehsiao/Videos/Netflix_ToddlerFountain_3840x2160_60fps_yuv420p.y4m",
            ),
            Source::SquareTimelapse => PathBuf::from(
                "/home/lukehsiao/Videos/Netflix_SquareAndTimelapse_3840x2160_60fps_yuv420p.y4m",
            ),
            Source::Barscene => {
                PathBuf::from("/home/lukehsiao/Videos/Netflix_BarScene_3840x2160_60fps_yuv420p.y4m")
            }
            Source::Rollercoaster => PathBuf::from(
                "/home/lukehsiao/Videos/Netflix_RollerCoaster_3840x2160_60fps_yuv420p.y4m",
            ),
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

        // Open the logfile
        let log = match OpenOptions::new().append(true).open("data/user_study.csv") {
            Ok(file) => file,
            Err(_) => {
                let mut file = OpenOptions::new()
                    .append(true)
                    .create_new(true)
                    .open("data/user_study.csv")
                    .unwrap();

                // Write the header for the CSV if it is new
                writeln!(
                    file,
                    "timestamp,name,alg,fovea,bg_width,bg_crf,fg_crf,delay_ms,gaze_source,video,total_gaze,min_gaze,max_gaze,frames,filesize_bytes",
                ).unwrap();

                file
            }
        };

        // Initialize with the highest quality settings (q0).
        let state = StateData {
            start: Instant::now(),
            delays,
            name: name.to_string(),
            baseline,
            video,
            key: key.to_string(),
            output: output.map(Path::to_path_buf),
            log,
        };

        // Init state machine
        UserStudy {
            data: Box::new(state),
            state: State::Init,
        }
    }

    /// Handle state transition logic.
    fn next(&mut self, event: Event) {
        match (&self.state, event) {
            (_, Event::Quit) => {
                info!("Quitting.");
                self.state = State::Quit;
            }
            (_, Event::Calibrate) => {
                info!("Re-calibrating.");
                self.state = State::Calibrate;
            }
            (_, Event::Baseline) => {
                info!("Showing baseline.");
                self.state = State::Baseline;
            }
            (_, Event::Video { quality: q }) => {
                info!("Showing quality {}.", q);
                self.state = State::Video { quality: q };
            }
            (State::Video { quality: _ }, Event::Pause) => {
                info!("Pausing the user study.");
                self.state = State::Pause;
            }
            (State::Pause, Event::Resume) => {
                info!("Resuming the user study.");
                self.state = State::Video { quality: 0 };
            }
            (State::Video { quality: q }, Event::Accept) => {
                info!("Choosing this quality setting.");
                self.state = State::Accept { quality: *q };
            }
            (_, Event::None) => (),
            (s, e) => {
                warn!("Undefined transition: {:?} and {:?}", s, e);
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
                    // TODO(lukehsiao): actually log stuff
                    info!("Log info for delay: {}", d.delay);
                }

                // Also state transition immediately after accept.
                if self.data.delays.is_empty() {
                    info!("All delays complete.");
                    self.state = State::Quit;
                } else {
                    info!("Paused and ready for the next delay.");
                    self.state = State::Pause;
                }
            }
            State::Pause => {}
            State::Calibrate => {
                // Create a new client
                let (width, height, _) =
                    crate::get_video_metadata(&self.data.video).expect("Unable to open video");
                let filter = "smartblur=lr=1.0:ls=-1.0";
                let bg_width = 512;

                // These settings don't really matter. We will make a new client for each quality.
                let _ = FvideoClient::new(
                    FoveationAlg::TwoStream,
                    30,
                    Dims { width, height },
                    Dims {
                        width: bg_width,
                        height: bg_width * 9 / 16,
                    },
                    DisplayOptions {
                        delay: 0,
                        filter: filter.to_string(),
                    },
                    GazeSource::Eyelink,
                    EyelinkOptions {
                        calibrate: true,
                        record: false,
                    },
                );

                // Also state transition afterwords to pause
                self.state = State::Pause;
            }
            State::Baseline => {
                play_video(&self.data.baseline)?;
                self.state = State::Pause;
            }
            State::Video { quality: q } => {
                info!("Playing video quality: {}", q);
                // Create a new client
                let (width, height, _) =
                    crate::get_video_metadata(&self.data.video).expect("Unable to open video");
                let delay = self.data.delays.last().unwrap().delay;
                let filter = "smartblur=lr=1.0:ls=-1.0";
                let (fovea, fg_crf, bg_width, bg_crf) = match q {
                    0 => {
                        let delay = self.data.delays.last().unwrap().q0;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    1 => {
                        let delay = self.data.delays.last().unwrap().q1;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    2 => {
                        let delay = self.data.delays.last().unwrap().q2;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    3 => {
                        let delay = self.data.delays.last().unwrap().q3;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    4 => {
                        let delay = self.data.delays.last().unwrap().q4;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    5 => {
                        let delay = self.data.delays.last().unwrap().q5;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    6 => {
                        let delay = self.data.delays.last().unwrap().q6;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    7 => {
                        let delay = self.data.delays.last().unwrap().q7;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    8 => {
                        let delay = self.data.delays.last().unwrap().q8;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    9 => {
                        let delay = self.data.delays.last().unwrap().q9;
                        (delay.fg_size, delay.fg_crf, delay.bg_size, delay.bg_crf)
                    }
                    _ => return Err(UserStudyError::InvalidQuality(q)),
                };
                let mut client = FvideoClient::new(
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

                client.disable_event(EventType::MouseMotion);
                client.disable_event(EventType::MouseButtonDown);
                client.disable_event(EventType::MouseButtonUp);
                client.enable_event(EventType::KeyUp);

                // Reinitalize a new server
                let (nal_tx, nal_rx) = flume::bounded(16);
                let (gaze_tx, gaze_rx) = flume::bounded(16);
                let (cmd_tx, cmd_rx) = flume::bounded(5);

                let video_clone = self.data.video.clone();
                let server = thread::spawn(move || -> Result<(), UserStudyError> {
                    // Wait for start command before starting the server
                    if let Ok(ServerCmd::Start) = cmd_rx.recv() {
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
                            let nals = match server.encode_frame(current_gaze) {
                                Ok(n) => n,
                                Err(_) => break,
                            };
                            nal_tx.send(nals)?;

                            // Terminate if signaled
                            if let Ok(ServerCmd::Stop) = cmd_rx.try_recv() {
                                break;
                            }
                        }
                    }
                    Ok(())
                });

                // Send first to pipeline encode/decode, otherwise it would be in serial.
                client.gaze_sample(); // Prime with one real gaze sample
                gaze_tx.send(client.gaze_sample())?;

                // Start the server
                cmd_tx.send(ServerCmd::Start)?;

                for nal in nal_rx {
                    // Send first to pipeline encode/decode, otherwise it would be in serial.
                    if gaze_tx.send(client.gaze_sample()).is_err() {
                        break;
                    }

                    client.display_frame(nal.0.as_ref(), nal.1.as_ref());

                    // Check for keyboard event, and force state transition if necessary
                    if let Some(key) = client.keyboard_event() {
                        match key {
                            Keycode::Escape => {
                                self.next(Event::Quit);
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::P => {
                                self.next(Event::Pause);
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::C => {
                                self.next(Event::Calibrate);
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::B => {
                                self.next(Event::Baseline);
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Return => {
                                // Need to log this here while the client exists
                                writeln!(
                                    self.data.log,
                                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                                    Utc::now().to_rfc3339(),
                                    self.data.name,
                                    FoveationAlg::TwoStream,
                                    fovea,
                                    bg_width,
                                    bg_crf,
                                    fg_crf,
                                    self.data.delays.last().unwrap().delay,
                                    GazeSource::Eyelink,
                                    self.data.key,
                                    client.total_gaze(),
                                    client.min_gaze(),
                                    client.max_gaze(),
                                    client.total_frames(),
                                    client.total_bytes()
                                )?;
                                self.next(Event::Accept);
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num0 => {
                                self.next(Event::Video { quality: 0 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num1 => {
                                self.next(Event::Video { quality: 1 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num2 => {
                                self.next(Event::Video { quality: 2 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num3 => {
                                self.next(Event::Video { quality: 3 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num4 => {
                                self.next(Event::Video { quality: 4 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num5 => {
                                self.next(Event::Video { quality: 5 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num6 => {
                                self.next(Event::Video { quality: 6 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num7 => {
                                self.next(Event::Video { quality: 7 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num8 => {
                                self.next(Event::Video { quality: 8 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            Keycode::Num9 => {
                                self.next(Event::Video { quality: 9 });
                                cmd_tx.send(ServerCmd::Stop)?;
                            }
                            _ => (),
                        }
                    }
                }

                server.join().unwrap()?;
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
                    _ => Event::None,
                },
                Ok(_) => Event::None,
                Err(e) => return Err(UserStudyError::IoError(e)),
            },
            None => {
                // So we're not just burning cycles busy spinning
                thread::sleep(Duration::from_millis(50));
                Event::None
            }
        };

        // Transition State
        let prev_state = user_study.state;
        user_study.next(event);
        if prev_state != user_study.state {
            debug!("Moving from: {:?} to {:?}", prev_state, user_study.state);
        }

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
