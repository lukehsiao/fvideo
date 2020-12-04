//! Struct for the foveated video encoding server.
//!
//! The server is responsible for encoding frames using the latest gaze data available from the
//! client.

extern crate ffmpeg_next as ffmpeg;

use std::cmp;
use std::convert::TryInto;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{num, ptr};

use lazy_static::lazy_static;
use log::{debug, error};
use regex::Regex;
use structopt::clap::arg_enum;
use thiserror::Error;
use x264::{Encoder, NalData, Param, Picture};

use crate::GazeSample;

arg_enum! {
    #[derive(Copy, Clone, Debug)]
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
    #[error("Encoder Error: {self}")]
    EncoderError(String),
}

/// Server/Encoder Struct
pub struct FvideoServer {
    fovea: i32,
    alg: FoveationAlg,
    qo_max: f32,
    video_in: BufReader<File>,
    bg_pic: Option<Picture>,
    fg_pic: Picture,
    orig_pic: Picture,
    bg_encoder: Option<Encoder>,
    fg_encoder: Encoder,
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

pub const CROP_WIDTH: u32 = 1920;
pub const CROP_HEIGHT: u32 = 1080;

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

        let orig_par = setup_x264_params(width, height)?;
        let orig_pic = Picture::from_param(&orig_par)?;

        // Only init 2nd stream if it is necessary
        let (fg_pic, fg_encoder, bg_pic, bg_encoder) = match alg {
            FoveationAlg::TwoStream => {
                // foreground stream is cropped
                let mut fg_par = setup_x264_params(CROP_WIDTH, CROP_HEIGHT)?;
                let fg_pic = Picture::from_param(&fg_par)?;
                let fg_encoder = Encoder::open(&mut fg_par)
                    .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

                // TODO: this will also need to be lower resolution than orig
                let mut bg_par = setup_x264_params(width, height)?;
                let bg_pic = Picture::from_param(&bg_par)?;
                let bg_encoder = Encoder::open(&mut bg_par)
                    .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

                (fg_pic, fg_encoder, Some(bg_pic), Some(bg_encoder))
            }
            _ => {
                // Otherwise, there is just a single stream of orig resolution
                let mut fg_par = setup_x264_params(width, height)?;
                let fg_pic = Picture::from_param(&fg_par)?;
                let fg_encoder = Encoder::open(&mut fg_par)
                    .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

                (fg_pic, fg_encoder, None, None)
            }
        };

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
            bg_pic,
            fg_pic,
            orig_pic,
            bg_encoder,
            fg_encoder,
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
                let mut buf = self.orig_pic.as_mut_slice(plane).unwrap();
                self.video_in.read_exact(&mut buf)?;
            }
        }

        Ok(())
    }

    pub fn encode_frame(&mut self, gaze: GazeSample) -> Result<Vec<NalData>, FvideoServerError> {
        let time = Instant::now();
        self.read_frame()?;
        debug!("    read_frame: {:#?}", time.elapsed());

        // TODO(lukehsiao): 5x5px white square where mouse cursor is.
        // Note that 235 = white for luma. Also note that trying to iterate over the whole image
        // here was too slow.
        //
        // const SQ_WIDTH: usize = 4;
        // let luma = pic.as_mut_slice(0).unwrap();
        // for x in 0..SQ_WIDTH {
        //     for y in 0..SQ_WIDTH {
        //         luma[cmp::min(WIDTH, (WIDTH * (p_y + y)) + (p_x + x))] = 0xEB;
        //     }
        // }

        // Prepare foveation algorithm preprocessing (e.g., computing QP offsets).
        //
        // Wait for timestamp > 0 so that the first (and only) I-frame is sent untouched.
        if self.fovea > 0 && self.timestamp >= 0 {
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
                    self.fg_pic.pic.prop.quant_offsets = self.qp_offsets.as_mut_ptr();
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
                    self.fg_pic.pic.prop.quant_offsets = self.qp_offsets.as_mut_ptr();
                }
                FoveationAlg::TwoStream => {
                    crop_x264_pic(
                        &self.orig_pic,
                        &gaze,
                        CROP_WIDTH,
                        CROP_HEIGHT,
                        &mut self.fg_pic,
                    )?;

                    // Crop frame to only the relevant (480 x 272) area
                    // self.pic.img.i_stride = [480, 240, 240, 0];
                    // self.pic.plane_size = [480 * 272, 240 * 136, 240 * 136];
                }
            }
        }

        self.fg_pic.set_timestamp(self.timestamp);
        self.timestamp += 1;

        let time = Instant::now();
        let mut nals = vec![];
        if let Some((nal, _, _)) = self.fg_encoder.encode(&self.fg_pic).unwrap() {
            nals.push(nal);
        }

        while self.fg_encoder.delayed_frames() {
            if let Some((nal, _, _)) = self.fg_encoder.encode(None).unwrap() {
                nals.push(nal);
            }
        }
        debug!("    x264.encode_frame: {:#?}", time.elapsed());

        Ok(nals)
    }
}

pub(crate) fn setup_x264_params(width: u32, height: u32) -> Result<Param, FvideoServerError> {
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
/// See <https://wiki.multimedia.cx/index.php/YUV4MPEG2> for details.
pub fn get_video_metadata(video: &PathBuf) -> Result<(u32, u32, f64), FvideoServerError> {
    let video_in = File::open(video)?;
    let mut video_in = BufReader::new(video_in);

    // First, read dimensions/FPS from Y4M header.
    let mut hdr = String::new();
    video_in.read_line(&mut hdr).unwrap();
    let (width, height, fps) = parse_y4m_header(&hdr)?;
    Ok((width, height, fps))
}

/// Crop orig_pic centered around the gaze and place into fg_pic.
fn crop_x264_pic(
    src: &Picture,
    gaze: &GazeSample,
    width: u32,
    height: u32,
    dst: &mut Picture,
) -> Result<(), FvideoServerError> {
    // output->img.width  = h->dims[2];
    // output->img.height = h->dims[3];
    // [> shift the plane pointers down 'top' rows and right 'left' columns. <]
    // for( int i = 0; i < output->img.planes; i++ )
    // {
    //     intptr_t offset = output->img.stride[i] * h->dims[1] * h->csp->height[i];
    //     offset += h->dims[0] * h->csp->width[i] * x264_cli_csp_depth_factor( output->img.csp );
    //     output->img.plane[i] += offset;
    // }

    // Keep the "cropped" window contained in the frame
    //
    // Only allow multiples of 2 to maintain integer values after division
    dbg!(&gaze);
    let top: u32 = match cmp::max(gaze.p_y as i32 - height as i32 / 2, 0) {
        n if n > 0 && n % 2 == 0 => n as u32,
        n if n > 0 && n % 2 != 0 => n as u32 + 1,
        _ => 0,
    };

    let left: u32 = match cmp::max(gaze.p_x as i32 - width as i32 / 2, 0) {
        n if n > 0 && n % 2 == 0 => n as u32,
        n if n > 0 && n % 2 != 0 => n as u32 + 1,
        _ => 0,
    };

    // TODO(lukehsiao): hard-coded color space values for now.
    let csp_height = vec![1.0, 0.5, 0.5];
    let csp_width = vec![1.0, 0.5, 0.5];

    let mut offset_plane: [*mut u8; 4] = [ptr::null_mut(); 4];

    // Shift the plane pointers down 'top' rows and right 'left' columns
    for i in 0..3 {
        let mut offset: isize =
            (src.pic.img.i_stride[i] as f32 * top as f32 * csp_height[i]) as isize;
        offset += (left as f32 * csp_width[i] * src.pic.img.i_csp as f32) as isize;

        // grab the offset ptrs
        // Copy data into fg_pic
        unsafe {
            offset_plane[i] = src.pic.img.plane[i].offset(offset);

            // Manually copying over. Is this too slow?
            let mut src_ptr: *mut u8 = offset_plane[i];
            let mut dst_ptr: *mut u8 = dst.pic.img.plane[i];

            for _ in 0..(CROP_HEIGHT as f32 * csp_height[i]) as u32 {
                ptr::copy_nonoverlapping(src_ptr, dst_ptr, dst.pic.img.i_stride[i] as usize);

                // Advance a full row
                src_ptr = src_ptr.offset(src.pic.img.i_stride[i] as isize);
                dst_ptr = dst_ptr.offset(dst.pic.img.i_stride[i] as isize);
            }
        }
    }

    // dbg!(src);
    // dbg!(dst);
    // dbg!(left);
    // dbg!(top);
    // dbg!(gaze);
    // dbg!(offset_plane);
    // todo!();
    Ok(())
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
}
