extern crate ffmpeg_next as ffmpeg;

use log::debug;
use x264::{Encoder, NalData, Picture};

use crate::{FvideoServerError, GazeSample};

const BLACK: u8 = 16;
const WHITE: u8 = 235;
pub const DIFF_THRESH: i32 = 100;
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
        let mut par = crate::setup_x264_params(width, height)?;
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
        let box_dim = width / 19;
        let buf = pic_white.as_mut_slice(0).unwrap();
        for c in 0..height {
            for r in 0..width {
                buf[(width * c + r) as usize] = if c > (height - box_dim) && r < (box_dim) {
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

    pub fn triggered(&self) -> bool {
        self.triggered
    }

    /// Read frame from dummy video which will include a white square at the
    /// bottom left when the gaze position has changed beyond a threshold.
    pub fn encode_frame(&mut self, gaze: GazeSample) -> Result<Vec<NalData>, FvideoServerError> {
        if self.triggered_buff >= LINGER_FRAMES {
            return Err(FvideoServerError::EncoderError("Finished.".to_string()));
        }
        if self.first_gaze.is_none() {
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

        let mut nals = vec![];
        if let Some((nal, _, _)) = self.encoder.encode(pic).unwrap() {
            nals.push(nal);
        }

        while self.encoder.delayed_frames() {
            if let Some((nal, _, _)) = self.encoder.encode(None).unwrap() {
                nals.push(nal);
            }
        }

        Ok(nals)
    }
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
    fn test_fill() {
        let mut buf = vec![0; 10];
        fill(&mut buf, 1);
        assert_eq!(buf, vec![1; 10]);
    }
}
