use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use log::{info, warn};
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

#[derive(Error, Debug)]
pub enum UserStudyError {
    #[error("Unable to play `{0}` with mpv.")]
    MpvError(String),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

// The set of possible user study states.
#[derive(Debug, PartialEq)]
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
    delays: HashMap<u64, u32>,
    name: String,
    baseline: PathBuf,
    video: PathBuf,
    output: Option<PathBuf>,
}

impl UserStudy {
    /// Create a new UserStudy state machine.
    fn new(name: &str, baseline: &Path, video: &Path, output: Option<&Path>) -> Self {
        // The artificial delays we will use.
        let mut delays = HashMap::new();
        delays.insert(0, 0);
        delays.insert(19, 0);
        delays.insert(38, 0);
        delays.insert(57, 0);
        delays.insert(76, 0);

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
            (_, Event::Quit) => {
                info!("Quitting the user study.");
                UserStudy {
                    data: self.data,
                    state: State::Quit,
                }
            }
            (_, Event::Pause) => {
                info!("Pausing the user study.");
                UserStudy {
                    data: self.data,
                    state: State::Pause,
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
    fn run(&self) -> Result<(), UserStudyError> {
        match self.state {
            State::Init => {}
            State::Accept { quality: q } => {
                info!("Accepted quality: {}", q);
            }
            State::Pause => {}
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
pub fn run(
    name: &str,
    baseline: &Path,
    video: &Path,
    output: Option<&Path>,
) -> Result<(), UserStudyError> {
    let mut state = UserStudy::new(name, baseline, video, output);

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
