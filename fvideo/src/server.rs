//! Struct for the foveated video encoding server.
//!
//! The server is responsible for encoding frames using the latest gaze data
//! available from the client.

extern crate ffmpeg_next as ffmpeg;

use std::convert::TryInto;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::num;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use lazy_static::lazy_static;
use log::{debug, error, info};
use regex::Regex;
use structopt::clap::arg_enum;
use thiserror::Error;
use x264::{Encoder, NalData, Param, Picture};

use crate::GazeSample;

arg_enum! {
    #[derive(Copy, Clone, Debug)]
    pub enum FoveationAlg {
        SquareStep,
        Gaussian
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
    #[error("Encoder Error: {self}")]
    EncoderError(String),
}

/// Server/Encoder Struct
pub struct FvideoServer {
    fovea: i32,
    alg: FoveationAlg,
    qo_max: f32,
    video_in: BufReader<File>,
    pic: Picture,
    encoder: Encoder,
    width: u32,
    height: u32,
    mb_x: u32,
    mb_y: u32,
    frame_dur: Duration,
    frame_time: Instant,
    frame_cnt: u32,
    last_frame_time: Duration,
    qp_offsets: Vec<f32>,
    hdr: String,
    timestamp: i64,
}

impl FvideoServer {
    pub fn new(
        fovea: i32,
        alg: FoveationAlg,
        qo_max: f32,
        video: PathBuf,
    ) -> Result<FvideoServer, FvideoServerError> {
        let video_in = File::open(video)?;
        let mut video_in = BufReader::new(video_in);

        // First, read dimensions/FPS from Y4M header.
        let mut hdr = String::new();
        video_in.read_line(&mut hdr).unwrap();
        let (width, height, fps) = parse_y4m_header(&hdr)?;

        let frame_dur = Duration::from_secs_f64(1.0 / fps);

        let mut par = setup_x264_params(width, height)?;
        let pic = Picture::from_param(&par)?;
        let encoder =
            Encoder::open(&mut par).map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        // The frame dimensions in terms of macroblocks
        let mb_x = width / 16;
        let mb_y = height / 16;
        let qp_offsets = vec![0.0; (mb_x * mb_y).try_into()?];

        let frame_time = Instant::now();

        Ok(FvideoServer {
            fovea,
            alg,
            qo_max,
            video_in,
            pic,
            encoder,
            width,
            height,
            mb_x,
            mb_y,
            frame_dur,
            frame_time,
            frame_cnt: 0,
            last_frame_time: frame_time.elapsed(),
            qp_offsets,
            hdr: String::new(),
            timestamp: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    fn read_frame(&mut self) -> Result<(), FvideoServerError> {
        // Advance source frame based on frame time.
        if (self.frame_time.elapsed() - self.last_frame_time >= self.frame_dur)
            || (self.frame_time.elapsed().as_millis() / self.frame_dur.as_millis()
                > self.frame_cnt.into())
        {
            debug!(
                "Frame {} gap: {:#?}",
                self.frame_cnt,
                self.frame_time.elapsed() - self.last_frame_time
            );
            self.last_frame_time = self.frame_time.elapsed();
            // Skip header data of the frame
            self.video_in.read_line(&mut self.hdr)?;
            self.frame_cnt += 1;

            // Read the input YUV frame
            for plane in 0..3 {
                let mut buf = self.pic.as_mut_slice(plane).unwrap();
                self.video_in.read_exact(&mut buf)?;
            }
        }

        Ok(())
    }

    pub fn encode_frame(&mut self, gaze: GazeSample) -> Result<Vec<NalData>, FvideoServerError> {
        let time = Instant::now();
        self.read_frame()?;
        debug!("    read_frame: {:#?}", time.elapsed());

        // Prepare QP offsets and encode

        // TODO(lukehsiao): 5x5px white square where mouse cursor is.
        // Note that 235 = white for luma
        // Also note that trying to iterate over the whole image here was too slow.
        // const SQ_WIDTH: usize = 4;
        // let luma = pic.as_mut_slice(0).unwrap();
        // for x in 0..SQ_WIDTH {
        //     for y in 0..SQ_WIDTH {
        //         luma[cmp::min(WIDTH, (WIDTH * (p_y + y)) + (p_x + x))] = 0xEB;
        //     }
        // }

        if self.fovea > 0 && self.timestamp > 0 {
            // Calculate offsets based on Foveation Alg
            match self.alg {
                FoveationAlg::Gaussian => {
                    for j in 0..self.mb_y {
                        for i in 0..self.mb_x {
                            // Below is the 2d gaussian used by Illahi et al.
                            self.qp_offsets[((self.mb_x * j) + i) as usize] = self.qo_max
                                - (self.qo_max
                                    * (-1.0
                                        * (((i as i32 - gaze.m_x as i32).pow(2)
                                            + (j as i32 - gaze.m_y as i32).pow(2))
                                            as f32
                                            / (2.0
                                                * (self.mb_x as f32 / self.fovea as f32)
                                                    .powi(2))))
                                    .exp());
                        }
                    }
                }
                FoveationAlg::SquareStep => {
                    for j in 0..self.mb_y {
                        for i in 0..self.mb_x {
                            // Keeps (2(dim) - 1)^2 macroblocks in HQ
                            self.qp_offsets[((self.mb_x * j) + i) as usize] =
                                if (gaze.m_x as i32 - i as i32).abs() < self.fovea
                                    && (gaze.m_y as i32 - j as i32).abs() < self.fovea
                                {
                                    0.0
                                } else {
                                    self.qo_max
                                };
                        }
                    }
                }
            }

            self.pic.pic.prop.quant_offsets = self.qp_offsets.as_mut_ptr();
        }

        self.pic.set_timestamp(self.timestamp);
        self.timestamp += 1;

        let time = Instant::now();
        let mut nals = vec![];
        if let Some((nal, _, _)) = self.encoder.encode(&self.pic).unwrap() {
            nals.push(nal);
        }

        while self.encoder.delayed_frames() {
            if let Some((nal, _, _)) = self.encoder.encode(None).unwrap() {
                nals.push(nal);
            }
        }
        debug!("    x264.encode_frame: {:#?}", time.elapsed());

        Ok(nals)
    }
}

const BLACK: u8 = 16;
const WHITE: u8 = 235;
const BOX_DIM: u32 = 200;
const DIFF_THRESH: i32 = 200;
const LINGER_FRAMES: i64 = 1;

/// Dummy server struct used for e2e latency measurements
pub struct FvideoDummyServer {
    pic_black: Picture,
    pic_white: Picture,
    encoder: Encoder,
    _width: u32,
    _height: u32,
    timestamp: i64,
    triggered_buff: i64,
    triggered: bool,
    first_gaze: Option<GazeSample>,
}

impl FvideoDummyServer {
    /// Used to create a dummy video server for measuring e2e latency.
    pub fn new(width: u32, height: u32) -> Result<FvideoDummyServer, FvideoServerError> {
        let mut par = setup_x264_params(width, height)?;
        let mut pic_black = Picture::from_param(&par)?;
        let mut pic_white = Picture::from_param(&par)?;
        let encoder =
            Encoder::open(&mut par).map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        // init black
        let mut buf = pic_black.as_mut_slice(0).unwrap();
        fill(&mut buf, BLACK);
        for plane in 1..3 {
            let mut buf = pic_black.as_mut_slice(plane).unwrap();
            fill(&mut buf, 128);
        }

        // init white
        // But, only a small portion in the bottom left of the frame. Otherwise
        // a whole screen of white adds a lot of latency.
        let buf = pic_white.as_mut_slice(0).unwrap();
        for c in 0..height {
            for r in 0..width {
                buf[(width * c + r) as usize] = if c > (height - BOX_DIM) && r < (BOX_DIM) {
                    WHITE
                } else {
                    BLACK
                };
            }
        }
        for plane in 1..3 {
            let mut buf = pic_white.as_mut_slice(plane).unwrap();
            fill(&mut buf, 128);
        }

        Ok(FvideoDummyServer {
            pic_black,
            pic_white,
            encoder,
            _width: width,
            _height: height,
            timestamp: 0,
            triggered_buff: 0,
            triggered: false,
            first_gaze: None,
        })
    }

    /// Read frame from dummy video which will include a white square at the
    /// bottom left when the gaze position has changed beyond a threshold.
    pub fn encode_frame(&mut self, gaze: GazeSample) -> Result<Vec<NalData>, FvideoServerError> {
        if self.triggered_buff >= LINGER_FRAMES {
            return Err(FvideoServerError::EncoderError("Finished.".to_string()));
        }
        if let None = self.first_gaze {
            self.first_gaze = Some(gaze);
        }

        if !self.triggered
            && ((gaze.p_x as i32 - self.first_gaze.unwrap().p_x as i32).abs() > DIFF_THRESH
                || (gaze.p_y as i32 - self.first_gaze.unwrap().p_y as i32).abs() > DIFF_THRESH)
        {
            self.triggered = true;
            debug!("Server changing white!");
        }

        let pic = match self.triggered {
            true => {
                self.pic_white.set_timestamp(self.timestamp);
                &self.pic_white
            }
            false => {
                self.pic_black.set_timestamp(self.timestamp);
                &self.pic_black
            }
        };

        self.timestamp += 1;
        if self.triggered {
            self.triggered_buff += 1;
        }

        let time = Instant::now();
        let mut nals = vec![];
        if let Some((nal, _, _)) = self.encoder.encode(pic).unwrap() {
            nals.push(nal);
        }

        while self.encoder.delayed_frames() {
            if let Some((nal, _, _)) = self.encoder.encode(None).unwrap() {
                nals.push(nal);
            }
        }
        debug!("    x264.encode_frame: {:#?}", time.elapsed());

        Ok(nals)
    }
}

fn setup_x264_params(width: u32, height: u32) -> Result<Param, FvideoServerError> {
    let mut par = Param::default_preset("superfast", "zerolatency")
        .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

    // TODO(lukehsiao): this is hacky, and shoould probably be cleaned up.
    par = par.set_x264_defaults();
    par = par.set_dimension(width as i32, height as i32);
    par = par.set_min_keyint(i32::MAX);
    par = par.set_no_scenecut();

    Ok(par)
}

/// Return the width, height, and framerate of the input Y4M.
///
/// See https://wiki.multimedia.cx/index.php/YUV4MPEG2 for details.
pub fn get_video_metadata(video: &PathBuf) -> Result<(u32, u32, f64), FvideoServerError> {
    let video_in = File::open(video)?;
    let mut video_in = BufReader::new(video_in);

    // First, read dimensions/FPS from Y4M header.
    let mut hdr = String::new();
    video_in.read_line(&mut hdr).unwrap();
    let (width, height, fps) = parse_y4m_header(&hdr)?;
    Ok((width, height, fps))
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

    let fps = match &caps["frame"] {
        "30:1" => 30.0,
        "25:1" => 25.0,
        "24:1" => 24.0,
        "30000:1001" => 29.97,
        "24000:1001" => 23.976,
        _ => return Err(FvideoServerError::ParseY4MError(src.to_string())),
    };

    Ok((width, height, fps))
}

fn fill(slice: &mut [u8], value: u8) {
    if let Some((last, elems)) = slice.split_last_mut() {
        for el in elems {
            el.clone_from(&value);
        }

        *last = value
    }
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
    }

    #[test]
    fn test_fill() {
        let mut buf = vec![0; 10];
        fill(&mut buf, 1);
        assert_eq!(buf, vec![1; 10]);
    }
}
