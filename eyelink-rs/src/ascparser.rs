//! Functions for parsing SR Research's [ASC files][asc].
//!
//! [asc]: http://download.sr-support.com/dispdoc/page25.html

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::str::FromStr;

use lazy_static::lazy_static;
// use log::info;
use regex::Regex;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AscParserError {
    #[error("Cannot parse eye sample from: {self}")]
    UnrecognizedString(String),
    #[error(transparent)]
    ParseIntError(#[from] std::num::ParseIntError),
    #[error(transparent)]
    ParseFloatError(#[from] std::num::ParseFloatError),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

/// Structure of a single eye sample. Note that this assumes only a single eye
/// is used.
#[derive(Debug)]
pub struct EyeSample {
    time: u32, // time of the sample (ms)
    x: f32,    // x position
    y: f32,    // y position
    p: f32,    // pupil size
}

impl FromStr for EyeSample {
    type Err = AscParserError;

    fn from_str(s: &str) -> Result<EyeSample, AscParserError> {
        lazy_static! {
            static ref RE: Regex = Regex::new(
                r"(?x)
                ^\s*(?P<time>[0-9]+)
                \s+
                (?P<x>[0-9]+\.[0-9])
                \s+
                (?P<y>[0-9]+\.[0-9])
                \s+
                (?P<p>[0-9]+\.[0-9])
                \s+
                [\.]+
                "
            )
            .unwrap();
        }
        let caps = match RE.captures(s) {
            None => return Err(AscParserError::UnrecognizedString(s.to_string())),
            Some(caps) => caps,
        };

        Ok(EyeSample {
            time: caps["time"].parse()?,
            x: caps["x"].parse()?,
            y: caps["y"].parse()?,
            p: caps["p"].parse()?,
        })
    }
}

/// Parse an input ASC file into a vector of EyeSamples.
pub fn parse_asc(f: PathBuf) -> Result<Vec<EyeSample>, AscParserError> {
    let f = File::open(f)?;
    let buffered = BufReader::new(f);

    let samples: Vec<EyeSample> = buffered
        .lines()
        .filter_map(|s| match s {
            Ok(s) => s.parse::<EyeSample>().ok(),
            Err(_) => None,
        })
        .collect();

    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_asc() {
        let mut samples = parse_asc(PathBuf::from("../data/shibuya_trim.asc")).unwrap();
        assert_eq!(samples.len(), 17901);
        let sample = samples.pop().unwrap();
        assert_eq!(sample.time, 4071986);
        assert_eq!(sample.x, 168.5);
        assert_eq!(sample.y, 471.9);
        assert_eq!(sample.p, 898.0);
    }

    #[test]
    fn test_eyesample_from_str() {
        let sample: EyeSample = "4054086   980.4   556.0   606.0 ... ".parse().unwrap();
        assert_eq!(sample.time, 4054086);
        assert_eq!(sample.x, 980.4);
        assert_eq!(sample.y, 556.0);
        assert_eq!(sample.p, 606.0);

        if let Ok(_) = "MSG 4054085 GAZE_COORDS 0.00 0.00 1919.00 1079.00".parse::<EyeSample>() {
            panic!("Should have failed.");
        }

        if let Ok(_) =
            "EFIX R   4054093    4054330 238   980.4   556.8     572".parse::<EyeSample>()
        {
            panic!("Should have failed.");
        }
        if let Ok(_) = "4054086   980.4   556.0   606.0".parse::<EyeSample>() {
            panic!("Should have failed.");
        }
    }
}
