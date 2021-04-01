// #![forbid(unsafe_code)]
// #![forbid(warnings)]
extern crate ffmpeg_next as ffmpeg;

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Instant;
use std::{io, num};

use lazy_static::lazy_static;
use regex::Regex;
use structopt::clap::arg_enum;
use thiserror::Error;
use x264::{NalData, Param};

pub mod client;
pub mod dummyserver;
pub mod server;
pub mod twostreamserver;
pub mod user_study;

pub type EncodedFrames = (Option<(NalData, GazeSample)>, Option<NalData>);

/// Tracking gaze statistics
#[derive(Copy, Clone, Debug)]
pub struct Coords {
    pub x: u64,
    pub y: u64,
}

/// Source video dimensions for initializing a client.
#[derive(Copy, Clone, Debug)]
pub struct Dims {
    pub width: u32,
    pub height: u32,
}

/// A gaze sample.
#[derive(Copy, Clone, Debug)]
pub struct GazeSample {
    pub time: Instant, // time of the sample
    pub seqno: u64,    // sequence number (will wrap around)
    pub d_width: u32,  // display width in px
    pub d_height: u32, // display height in px
    pub d_x: u32,      // x position in disp px
    pub d_y: u32,      // y position in disp px
    pub p_x: u32,      // x position in video px
    pub p_y: u32,      // y position in video px
    pub m_x: u32,      // x position in macroblock
    pub m_y: u32,      // y position in macroblock
}

arg_enum! {
    #[derive(Copy, Clone, Debug, PartialEq)]
    pub enum FoveationAlg {
        SquareStep,
        Gaussian,
        TwoStream,
    }
}

#[derive(Error, Debug)]
pub enum FvideoServerError {
    #[error(transparent)]
    ParseIntError(#[from] num::ParseIntError),
    #[error(transparent)]
    ParseFloatError(#[from] num::ParseFloatError),
    #[error(transparent)]
    TryFromIntError(#[from] num::TryFromIntError),
    #[error("Invalid Y4M Header: {self}")]
    ParseY4MError(String),
    #[error(transparent)]
    IoError(#[from] io::Error),
    #[error(transparent)]
    X264Error(#[from] x264::X264Error),
    #[error(transparent)]
    FfmpegError(#[from] ffmpeg::Error),
    #[error("Encoder Error: {self}")]
    EncoderError(String),
    #[error("Invalid Foveation Algorithm: {self}")]
    InvalidAlgError(String),
    #[error("TwoStream Error: {self}")]
    TwoStreamError(String),
}

// TODO(lukehsiao): "test.edf" works, but this breaks for unknown reasons for
// other filenames (like "recording.edf"). Not sure why.
pub const EDF_FILE: &str = "test.edf";

arg_enum! {
    #[derive(Copy, Clone, PartialEq, Debug)]
    pub enum GazeSource {
        Mouse,
        Eyelink,
    }
}

#[derive(Error, Debug)]
pub enum FvideoClientError {
    #[error(transparent)]
    ParseIntError(#[from] num::ParseIntError),
    #[error(transparent)]
    ParseFloatError(#[from] num::ParseFloatError),
    #[error(transparent)]
    EyelinkError(#[from] eyelink_rs::EyelinkError),
}

/// Collection of options for interacting with the Eyelink.
#[derive(Copy, Clone, Debug)]
pub struct EyelinkOptions {
    pub calibrate: bool,
    pub record: bool,
}

/// Collection of options for modifying the display of the video.
#[derive(Debug)]
pub struct DisplayOptions {
    pub delay: u64,
    pub filter: String,
}

/// Parse the width, height, and frame rate from the Y4M header.
///
/// See https://wiki.multimedia.cx/index.php/YUV4MPEG2 for details.
fn parse_y4m_header(src: &str) -> Result<(u32, u32, f64), FvideoServerError> {
    lazy_static! {
        static ref RE: Regex = Regex::new(
            r"(?x)
            ^YUV4MPEG2\s
            W(?P<width>[0-9]+)\s
            H(?P<height>[0-9]+)\s
            F(?P<frame>[0-9:]+).*
        "
        )
        .unwrap();
    }

    let caps = match RE.captures(src) {
        None => return Err(FvideoServerError::ParseY4MError(src.to_string())),
        Some(caps) => caps,
    };

    let width = caps["width"].parse()?;
    let height = caps["height"].parse()?;

    let ratio: Vec<&str> = caps["frame"].split(':').collect();

    let fps = ratio[0].parse::<f64>()? / ratio[1].parse::<f64>()?;

    Ok((width, height, fps))
}

fn setup_x264_params_bg(width: u32, height: u32, qp: i32) -> Result<Param, FvideoServerError> {
    let mut par = Param::default_preset("faster", "zerolatency")
        .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

    par = par.set_dimension(width as i32, height as i32);
    par = par.set_min_keyint(i32::MAX);
    par = par.set_no_scenecut();
    par = par.set_qp(qp);

    Ok(par)
}

fn setup_x264_params(width: u32, height: u32, qp: i32) -> Result<Param, FvideoServerError> {
    let mut par = Param::default_preset("superfast", "zerolatency")
        .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

    // TODO(lukehsiao): this is hacky, and should probably be cleaned up.
    par = par.set_x264_defaults();
    par = par.set_dimension(width as i32, height as i32);
    par = par.set_min_keyint(i32::MAX);
    par = par.set_no_scenecut();
    par = par.set_qp(qp);

    Ok(par)
}

fn setup_x264_params_bg_crf(width: u32, height: u32, crf: f32) -> Result<Param, FvideoServerError> {
    let mut par = Param::default_preset("faster", "zerolatency")
        .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

    par = par.set_dimension(width as i32, height as i32);
    // par = par.set_min_keyint(i32::MAX);
    // par = par.set_no_scenecut();
    par = par.set_crf(crf);

    Ok(par)
}

fn setup_x264_params_crf(width: u32, height: u32, crf: f32) -> Result<Param, FvideoServerError> {
    let mut par = Param::default_preset("veryfast", "zerolatency")
        .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

    // TODO(lukehsiao): this is hacky, and should probably be cleaned up.
    par = par.set_x264_defaults();
    par = par.set_dimension(width as i32, height as i32);
    // par = par.set_min_keyint(i32::MAX);
    // par = par.set_no_scenecut();
    par = par.set_crf(crf);

    Ok(par)
}

/// Return the width, height, and framerate of the input Y4M.
///
/// See <https://wiki.multimedia.cx/index.php/YUV4MPEG2> for details.
pub fn get_video_metadata(video: &Path) -> Result<(u32, u32, f64), FvideoServerError> {
    let video_in = File::open(video)?;
    let mut video_in = BufReader::new(video_in);

    // First, read dimensions/FPS from Y4M header.
    let mut hdr = String::new();
    video_in.read_line(&mut hdr).unwrap();
    let (width, height, fps) = crate::parse_y4m_header(&hdr)?;
    Ok((width, height, fps))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_y4m_header() {
        let hdr = "YUV4MPEG2 W3840 H2160 F24:1 Ip A0:0 C420jpeg\n";

        let (width, height, fps) = parse_y4m_header(&hdr).unwrap();
        assert_eq!(width, 3840);
        assert_eq!(height, 2160);
        assert_eq!(fps, 24.0);

        let hdr = "YUV4MPEG2 W1920 H1080 F60:1 Ip A0:0 C420jpeg\n";

        let (width, height, fps) = parse_y4m_header(&hdr).unwrap();
        assert_eq!(width, 1920);
        assert_eq!(height, 1080);
        assert_eq!(fps, 60.0);

        let hdr = "YUV4MPEG2 W1920 H1080 F24000:1001 Ip A0:0 C420jpeg\n";

        let (width, height, fps) = parse_y4m_header(&hdr).unwrap();
        assert_eq!(width, 1920);
        assert_eq!(height, 1080);
        assert_eq!(fps, 24000.0 / 1001.0);
    }
}
