use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use thiserror::Error;

// use crate::client::FvideoClient;
// use crate::twostreamserver::FvideoTwoStreamServer;
// use crate::{Dims, DisplayOptions, EyelinkOptions, FoveationAlg, GazeSource, EDF_FILE};

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
    Pause { quality: u32 },
    Init,
    Calibrate,
    Baseline,
    Quit,
}

// Events that can cause state transitions
#[derive(Debug)]
enum Event {
    Enter,
    One,
    Two,
    Quit,
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
    delays: HashMap<u64, u32>,
    name: String,
    baseline: PathBuf,
    video: PathBuf,
    output: Option<PathBuf>,
}

impl UserStudy {
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

    fn next(self, event: Event) -> Self {
        match (&self.state, event) {
            (State::Init, Event::None) => {
                println!("Init, None");
                self
            }
            (_, _) => self,
        }
    }

    fn run(&self) {
        match self.state {
            State::Init => println!("Init state action."),
            _ => print!("x"),
        }
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

    loop {
        state = state.next(Event::None);
        state.run();
        break;
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
