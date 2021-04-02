use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use std::{fs, thread};

use log::{info, warn};
use rand::prelude::*;
use serde::Deserialize;
use structopt::clap::arg_enum;
use termion::event::Key;
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::{clear, cursor};
use thiserror::Error;

// use crate::client::FvideoClient;
// use crate::twostreamserver::FvideoTwoStreamServer;
// use crate::{Dims, DisplayOptions, EyelinkOptions, FoveationAlg, GazeSource, EDF_FILE};

// TODO(lukehsiao): How do we get keyboard events when the videos are fullscreen?
// TODO(lukehsiao): How do we "interrupt" a currently playing video to change states?
// TODO(lukehsiao): How do we load configurations for each latency/video config? From a file?

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

#[derive(Debug, Deserialize)]
struct Quality {
    fg_size: u32,
    fg_crf: u32,
    bg_size: u32,
    bg_crf: u32,
}

#[derive(Debug, Deserialize)]
struct Delay {
    delay: u32,
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
    delays: Vec<u32>,
    name: String,
    baseline: PathBuf,
    video: PathBuf,
    output: Option<PathBuf>,
}

impl UserStudy {
    /// Create a new UserStudy state machine.
    fn new(
        name: &str,
        source: &Source,
        output: Option<&Path>,
        settings: HashMap<String, Video>,
    ) -> Self {
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
        let mut delays = vec![];
        let setting = settings.get(key).unwrap();
        for _ in 0..setting.attempts {
            for d in &setting.delays {
                delays.push(d.delay);
            }
        }
        // Shuffle to randomize order they are presented to the user
        let mut rng = rand::thread_rng();
        delays.shuffle(&mut rng);

        let state = StateData {
            start: Instant::now(),
            delays,
            name: name.to_string(),
            baseline: baseline.to_path_buf(),
            video: video.to_path_buf(),
            output: output.map(Path::to_path_buf),
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
            (State::Accept { quality: _ }, Event::Resume) => {
                // If we're all done
                if self.data.delays.is_empty() {
                    info!("All delays complete.");
                    UserStudy {
                        data: self.data,
                        state: State::Quit,
                    }
                } else {
                    info!("Paused and ready for the next delay.");
                    UserStudy {
                        data: self.data,
                        state: State::Pause { quality: 0 },
                    }
                }
            }
            (_, Event::Quit) => {
                info!("Quitting the user study.");
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
            (State::Calibrate, _) => {
                info!("Showing baseline.");
                UserStudy {
                    data: self.data,
                    state: State::Baseline,
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
            State::Init => {}
            State::Accept { quality: q } => {
                info!("Accepted quality: {}", q);
                if let Some(d) = self.data.delays.pop() {
                    info!("Log info for delay: {}", d);
                }
            }
            State::Pause { quality: _ } => {}
            State::Calibrate => {
                info!("Run Calibration.");
            }
            State::Baseline => play_video(&self.data.baseline)?,
            State::Video { quality: q } => {
                info!("Print quality: {}", q);
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

    let mut state = UserStudy::new(name, source, output, settings);

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
        state = state.next(event);

        // Run new state
        if let State::Quit = state.state {
            info!("User study is complete.");
            break;
        } else {
            state.run()?;
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
