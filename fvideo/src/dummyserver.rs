//! Dummy foveated video encoding server.
//!
//! Unlike the real servers, the dummy server generates a black video, and then a frame with a white
//! square in the bottom left once the gaze is triggered.
extern crate ffmpeg_next as ffmpeg;

use std::convert::TryInto;
use std::ptr;

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::sys as ffmpeg_sys_next;
use log::{debug, info, warn};
use x264::{Encoder, Picture};

use crate::{Dims, EncodedFrames, FvideoServerError, GazeSample};

const BLACK: u8 = 16;
const WHITE: u8 = 235;
pub const DIFF_THRESH: i32 = 50;
const LINGER_FRAMES: i64 = 1;

/// Dummy server used for single-stream e2e latency measurements
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
        let mut par = crate::setup_x264_params(width, height, 24)?;
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

    pub fn reset_trigger(&mut self) {
        self.triggered = false;
        self.triggered_buff = 0;
    }

    /// Read frame from dummy video which will include a white square at the
    /// bottom left when the gaze position has changed beyond a threshold.
    pub fn encode_frame(&mut self, gaze: GazeSample) -> Result<EncodedFrames, FvideoServerError> {
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

        let fg_nal = None;
        let mut bg_nal = None;
        if let Some((nal, _, _)) = self.encoder.encode(pic).unwrap() {
            bg_nal = Some(nal);
        }

        while self.encoder.delayed_frames() {
            todo!();
        }

        Ok((fg_nal, bg_nal))
    }
}

/// Dummy server used for single-stream e2e latency measurements
pub struct FvideoDummyTwoStreamServer {
    fovea: u32,
    pic_black: Picture,
    pic_white: Picture,
    bg_pic: Picture,
    fg_pic: Picture,
    bg_encoder: Encoder,
    fg_encoder: Encoder,
    scaler: Context,
    width: u32,
    height: u32,
    timestamp: i64,
    triggered_buff: i64,
    triggered: bool,
    first_gaze: Option<GazeSample>,
}

impl FvideoDummyTwoStreamServer {
    /// Used to create a dummy video server for measuring e2e latency.
    pub fn new(
        src_dims: Dims,
        rescale_dims: Dims,
        fovea: u32,
    ) -> Result<FvideoDummyTwoStreamServer, FvideoServerError> {
        let fovea_size = match fovea {
            n if n * 16 > src_dims.height => src_dims.height,
            0 => {
                return Err(FvideoServerError::TwoStreamError(
                    "TwoStream requires fovea to be non-zero.".to_string(),
                ))
            }
            n => n * 16,
        };
        let par = crate::setup_x264_params(src_dims.width, src_dims.height, 24)?;
        let mut pic_black = Picture::from_param(&par)?;
        let mut pic_white = Picture::from_param(&par)?;

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
        let box_dim = src_dims.width / 19;
        let buf = pic_white.as_mut_slice(0).unwrap();
        for c in 0..src_dims.height {
            for r in 0..src_dims.width {
                buf[(src_dims.width * c + r) as usize] =
                    if c > (src_dims.height - box_dim) && r < (box_dim) {
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

        // foreground stream is cropped
        let mut fg_par = crate::setup_x264_params(fovea_size, fovea_size, 24)?;
        let fg_pic = Picture::from_param(&fg_par)?;
        let fg_encoder = Encoder::open(&mut fg_par)
            .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        // background stream is scaled
        let mut bg_par = crate::setup_x264_params_bg(rescale_dims.width, rescale_dims.height, 33)?;
        let bg_pic = Picture::from_param(&bg_par)?;
        let bg_encoder = Encoder::open(&mut bg_par)
            .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        let scaler = Context::get(
            Pixel::YUV420P,
            src_dims.width,
            src_dims.height,
            Pixel::YUV420P,
            rescale_dims.width,
            rescale_dims.height,
            Flags::FAST_BILINEAR,
        )?;

        Ok(FvideoDummyTwoStreamServer {
            fovea: fovea_size,
            pic_black,
            pic_white,
            bg_pic,
            fg_pic,
            bg_encoder,
            fg_encoder,
            scaler,
            width: src_dims.width,
            height: src_dims.height,
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
    pub fn encode_frame(
        &mut self,
        mut gaze: GazeSample,
    ) -> Result<EncodedFrames, FvideoServerError> {
        if self.triggered_buff >= LINGER_FRAMES {
            info!("Finished.");
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

            // Change gaze to bottom left
            let box_dim = self.width / 19;
            gaze.p_x = box_dim / 2;
            gaze.p_y = self.height - (box_dim / 2);
            info!("Server changing white!");
        }

        self.timestamp += 1;
        if self.triggered {
            self.triggered_buff += 1;
        }

        // Crop section into fg_pic
        self.crop_x264_pic(&mut gaze, self.fovea, self.fovea);

        // Rescale to bg_pic. This drops FPS from ~1500 to ~270 on panda. Using
        // fast_bilinear rather than bilinear gives about 800fps.
        unsafe {
            if self.triggered {
                ffmpeg_sys_next::sws_scale(
                    self.scaler.as_mut_ptr(),
                    self.pic_white.pic.img.plane.as_ptr() as *const *const _,
                    self.pic_white.pic.img.i_stride.as_ptr(),
                    0,
                    self.height.try_into()?,
                    self.bg_pic.pic.img.plane.as_ptr(),
                    self.bg_pic.pic.img.i_stride.as_ptr(),
                );
            } else {
                ffmpeg_sys_next::sws_scale(
                    self.scaler.as_mut_ptr(),
                    self.pic_black.pic.img.plane.as_ptr() as *const *const _,
                    self.pic_black.pic.img.i_stride.as_ptr(),
                    0,
                    self.height.try_into()?,
                    self.bg_pic.pic.img.plane.as_ptr(),
                    self.bg_pic.pic.img.i_stride.as_ptr(),
                );
            }
        }

        self.bg_pic.set_timestamp(self.timestamp);
        self.fg_pic.set_timestamp(self.timestamp);
        self.timestamp += 1;

        let mut fg_nal = None;
        let mut bg_nal = None;
        match (
            self.fg_encoder.encode(&self.fg_pic).unwrap(),
            self.bg_encoder.encode(&self.bg_pic).unwrap(),
        ) {
            (Some((fg, _, _)), Some((bg, _, _))) => {
                fg_nal = Some((fg, gaze));
                bg_nal = Some(bg);
            }
            (_, _) => {
                warn!("Didn't encode a nal?");
            }
        }

        while self.fg_encoder.delayed_frames() {
            todo!();
        }

        Ok((fg_nal, bg_nal))
    }

    /// Crop orig_pic centered around the gaze and place into fg_pic.
    fn crop_x264_pic(&mut self, gaze: &mut GazeSample, width: u32, height: u32) {
        let p_y = gaze.p_y as i32;
        let p_x = gaze.p_x as i32;

        // TODO(lukehsiao): This is unsafe in particular in that right now I allow the copies to
        // reach into random data off the edges of the picture. This garbage data is essentially
        // hidden when it is displayed, but it could be better to be safer about this.

        // Keep the "cropped" window contained in the frame.
        // Only allow multiples of 2 to maintain integer values after division
        let top: i32 = match p_y - height as i32 / 2 {
            n if n % 2 == 0 => n,
            n if n % 2 != 0 => {
                gaze.p_y += 1;
                n + 1
            }
            _ => 0,
        };
        let left: i32 = match p_x - width as i32 / 2 {
            n if n % 2 == 0 => n,
            n if n % 2 != 0 => {
                gaze.p_x += 1;
                n + 1
            }
            _ => 0,
        };

        // TODO(lukehsiao): hard-coded color space values for now.
        let csp_height = [1.0, 0.5, 0.5];
        let csp_width = [1.0, 0.5, 0.5];

        let mut offset_plane: [*mut u8; 4] = [ptr::null_mut(); 4];

        // Shift the plane pointers down 'top' rows and right 'left' columns
        for i in 0..3 {
            let mut offset: f32 = match self.triggered {
                false => self.pic_black.pic.img.i_stride[i] as f32 * top as f32 * csp_height[i],
                true => self.pic_white.pic.img.i_stride[i] as f32 * top as f32 * csp_height[i],
            };
            offset += left as f32 * csp_width[i];

            // grab the offset ptrs
            // Copy data into fg_pic
            unsafe {
                offset_plane[i] = match self.triggered {
                    false => self.pic_black.pic.img.plane[i].offset(offset.round() as isize),
                    true => self.pic_white.pic.img.plane[i].offset(offset.round() as isize),
                };

                // Manually copying over. Is this too slow?
                let mut src_ptr: *mut u8 = offset_plane[i];
                let mut dst_ptr: *mut u8 = self.fg_pic.pic.img.plane[i];

                for _ in 0..(self.fovea as f32 * csp_height[i]).round() as u32 {
                    ptr::copy_nonoverlapping(
                        src_ptr,
                        dst_ptr,
                        self.fg_pic.pic.img.i_stride[i].try_into().unwrap(),
                    );

                    // Advance a full row
                    src_ptr = src_ptr.offset(match self.triggered {
                        false => self.pic_black.pic.img.i_stride[i].try_into().unwrap(),
                        true => self.pic_white.pic.img.i_stride[i].try_into().unwrap(),
                    });
                    dst_ptr = dst_ptr.offset(self.fg_pic.pic.img.i_stride[i].try_into().unwrap());
                }
            }
        }
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
