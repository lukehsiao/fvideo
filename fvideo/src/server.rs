//! Struct for the foveated video encoding server.
//!
//! The server is responsible for encoding frames using the latest gaze data available from the
//! client.

extern crate ffmpeg_next as ffmpeg;

use std::convert::TryInto;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use log::debug;
use x264::{Encoder, NalData, Picture};

use crate::{FoveationAlg, FvideoServerError, GazeSample};

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
    frame_dur: Duration,
    frame_time: Instant,
    frame_cnt: u32,
    last_frame_time: Duration,
    timestamp: i64,
}

impl FvideoServer {
    pub fn new(
        fovea: i32,
        alg: FoveationAlg,
        qo_max: f32,
        video: PathBuf,
    ) -> Result<FvideoServer, FvideoServerError> {
        // Validate alg
        if let FoveationAlg::TwoStream = alg {
            return Err(FvideoServerError::InvalidAlgError(
                "This server only supports a single stream.".into(),
            ));
        }

        let video_in = File::open(video)?;
        let mut video_in = BufReader::new(video_in);

        // First, read dimensions/FPS from Y4M header.
        // This is done manually so that the header is already skipped once the server is
        // initialized.
        let mut hdr = String::new();
        video_in.read_line(&mut hdr).unwrap();
        let (width, height, fps) = crate::parse_y4m_header(&hdr)?;

        let frame_dur = Duration::from_secs_f64(1.0 / fps);

        let mut par = crate::setup_x264_params(width, height, 24)?;
        let pic = Picture::from_param(&par)?;
        let encoder =
            Encoder::open(&mut par).map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

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
            frame_dur,
            frame_time,
            frame_cnt: 0,
            last_frame_time: frame_time.elapsed(),
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
            self.video_in.read_line(&mut String::new())?;
            self.frame_cnt += 1;

            // Read the input YUV frame
            for plane in 0..3 {
                let mut buf = self.pic.as_mut_slice(plane).unwrap();
                self.video_in.read_exact(&mut buf)?;
            }
        }

        Ok(())
    }

    pub fn encode_frame(
        &mut self,
        gaze: GazeSample,
    ) -> Result<Vec<(Option<NalData>, NalData)>, FvideoServerError> {
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
        if self.fovea > 0 && self.timestamp > 0 {
            // Calculate offsets based on Foveation Alg
            match self.alg {
                FoveationAlg::Gaussian => {
                    let (mb_x, mb_y) = (self.width / 16, self.height / 16);
                    let mut qp_offsets: Vec<f32> = vec![0.0; (mb_x * mb_y).try_into()?];
                    for j in 0..mb_y {
                        for i in 0..mb_x {
                            // Below is the 2d gaussian used by Illahi et al.
                            qp_offsets[((mb_x * j) + i) as usize] = self.qo_max
                                - (self.qo_max
                                    * (-1.0
                                        * (((i as i32 - gaze.m_x as i32).pow(2)
                                            + (j as i32 - gaze.m_y as i32).pow(2))
                                            as f32
                                            / (2.0 * (mb_x as f32 / self.fovea as f32).powi(2))))
                                    .exp());
                        }
                    }
                    self.pic.pic.prop.quant_offsets = qp_offsets.as_mut_ptr();
                }
                FoveationAlg::SquareStep => {
                    let (mb_x, mb_y) = (self.width / 16, self.height / 16);
                    let mut qp_offsets: Vec<f32> = vec![0.0; (mb_x * mb_y).try_into()?];
                    for j in 0..mb_y {
                        for i in 0..mb_x {
                            // Keeps (2(dim) - 1)^2 macroblocks in HQ
                            qp_offsets[((mb_x * j) + i) as usize] =
                                if (gaze.m_x as i32 - i as i32).abs() < self.fovea
                                    && (gaze.m_y as i32 - j as i32).abs() < self.fovea
                                {
                                    0.0
                                } else {
                                    self.qo_max
                                };
                        }
                    }
                    self.pic.pic.prop.quant_offsets = qp_offsets.as_mut_ptr();
                }
                FoveationAlg::TwoStream => {
                    unimplemented!()
                }
            }
        }

        self.pic.set_timestamp(self.timestamp);
        self.timestamp += 1;

        let time = Instant::now();
        let mut nals = vec![];

        if let Some((nal, _, _)) = self.encoder.encode(&self.pic).unwrap() {
            nals.push((None, nal));
        }
        while self.encoder.delayed_frames() {
            if let Some((nal, _, _)) = self.encoder.encode(None).unwrap() {
                nals.push((None, nal));
            }
        }

        debug!("    x264.encode_frame: {:#?}", time.elapsed());

        Ok(nals)
    }
}
