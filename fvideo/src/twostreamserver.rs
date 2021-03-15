//! Foveated video encoding server using two streams.

extern crate ffmpeg_next as ffmpeg;

use std::convert::TryInto;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::ptr;
use std::time::{Duration, Instant};

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::sys as ffmpeg_sys_next;
use log::{debug, warn};
use x264::{Encoder, Picture};

use crate::{Dims, EncodedFrames, FvideoServerError, GazeSample};

/// Server/Encoder Struct
pub struct FvideoTwoStreamServer {
    fovea: u32,
    video_in: BufReader<File>,
    bg_pic: Picture,
    fg_pic: Picture,
    orig_pic: Picture,
    bg_encoder: Encoder,
    fg_encoder: Encoder,
    scaler: Context,
    width: u32,
    height: u32,
    frame_dur: Duration,
    frame_time: Instant,
    frame_cnt: u32,
    last_frame_time: Duration,
    last_gaze_sample: GazeSample,
    timestamp: i64,
}

const DIFF_THRESH: i32 = 10;

impl FvideoTwoStreamServer {
    pub fn new(
        fovea: u32,
        rescale: Dims,
        fg_crf: f32,
        bg_crf: f32,
        video: PathBuf,
    ) -> Result<FvideoTwoStreamServer, FvideoServerError> {
        let video_in = File::open(video)?;
        let mut video_in = BufReader::new(video_in);

        // First, read dimensions/FPS from Y4M header.
        // This is done manually so that the header is already skipped once the server is
        // initialized.
        let mut hdr = String::new();
        video_in.read_line(&mut hdr).unwrap();
        let (width, height, fps) = crate::parse_y4m_header(&hdr)?;

        let fovea_size = match fovea {
            n if n * 16 > height => height,
            0 => {
                return Err(FvideoServerError::TwoStreamError(
                    "TwoStream requires fovea to be non-zero.".to_string(),
                ))
            }
            n => n * 16,
        };

        let frame_dur = Duration::from_secs_f64(1.0 / fps);

        let orig_par = crate::setup_x264_params_crf(width, height, fg_crf)?;
        let orig_pic = Picture::from_param(&orig_par)?;

        // foreground stream is cropped
        let mut fg_par = crate::setup_x264_params_crf(fovea_size, fovea_size, fg_crf)?;
        let fg_pic = Picture::from_param(&fg_par)?;
        let fg_encoder = Encoder::open(&mut fg_par)
            .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        // background stream is scaled
        let mut bg_par = crate::setup_x264_params_bg_crf(rescale.width, rescale.height, bg_crf)?;
        let bg_pic = Picture::from_param(&bg_par)?;
        let bg_encoder = Encoder::open(&mut bg_par)
            .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        let scaler = Context::get(
            Pixel::YUV420P,
            width,
            height,
            Pixel::YUV420P,
            rescale.width,
            rescale.height,
            Flags::BILINEAR,
        )?;

        let frame_time = Instant::now();

        let last_gaze_sample = GazeSample {
            time: Instant::now(),
            seqno: 0,
            d_width: 0,
            d_height: 0,
            d_x: 0,
            d_y: 0,
            p_x: 0,
            p_y: 0,
            m_x: 0,
            m_y: 0,
        };

        Ok(FvideoTwoStreamServer {
            fovea: fovea_size,
            video_in,
            bg_pic,
            fg_pic,
            orig_pic,
            bg_encoder,
            fg_encoder,
            scaler,
            width,
            height,
            frame_dur,
            frame_time,
            frame_cnt: 0,
            last_frame_time: frame_time.elapsed(),
            last_gaze_sample,
            timestamp: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    fn read_frame(&mut self) -> Result<bool, FvideoServerError> {
        // Advance source frame based on frame time.
        if self.frame_cnt == 0
            || (self.frame_time.elapsed() - self.last_frame_time >= self.frame_dur)
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
                let mut buf = self.orig_pic.as_mut_slice(plane).unwrap();
                self.video_in.read_exact(&mut buf)?;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // TODO(lukehsiao): I don't like this return type. At some point we should pull this into a
    // trait or something to have a more clear interface.
    pub fn encode_frame(
        &mut self,
        mut gaze: GazeSample,
    ) -> Result<EncodedFrames, FvideoServerError> {
        let time = Instant::now();
        let advanced = self.read_frame()?;

        let gaze_changed: bool = {
            if ((self.last_gaze_sample.p_x as i32 - gaze.p_x as i32).abs() > DIFF_THRESH)
                || ((self.last_gaze_sample.p_y as i32 - gaze.p_y as i32).abs() > DIFF_THRESH)
            {
                self.last_gaze_sample = gaze;
                true
            } else {
                false
            }
        };

        debug!("    read_frame: {:#?}", time.elapsed());

        let time = Instant::now();

        let mut bg_nal = None;
        let mut fg_nal = None;
        if advanced {
            // Rescale to bg_pic.
            unsafe {
                ffmpeg_sys_next::sws_scale(
                    self.scaler.as_mut_ptr(),
                    self.orig_pic.pic.img.plane.as_ptr() as *const *const _,
                    self.orig_pic.pic.img.i_stride.as_ptr(),
                    0,
                    self.height.try_into()?,
                    self.bg_pic.pic.img.plane.as_ptr(),
                    self.bg_pic.pic.img.i_stride.as_ptr(),
                );
            }

            self.bg_pic.set_timestamp(self.timestamp);

            match self.bg_encoder.encode(&self.bg_pic).unwrap() {
                Some((bg, _, _)) => {
                    bg_nal = Some(bg);
                }
                _ => warn!("Didn't encode a nal?"),
            }
        }

        if advanced || gaze_changed {
            // Crop section into fg_pic
            self.crop_x264_pic(&mut gaze, self.fovea, self.fovea);

            self.fg_pic.set_timestamp(self.timestamp);
            self.timestamp += 1;

            // TODO(lukehsiao): These is trying to encode both streams in sync. In reality, the
            // whole low quality stream could be sent beforehand, or in lower FPS. Only the
            // foreground high quality stream needs to be high FPS.
            match self.fg_encoder.encode(&self.fg_pic).unwrap() {
                Some((fg, _, _)) => {
                    fg_nal = Some((fg, gaze));
                }
                _ => {
                    warn!("Didn't encode a nal?");
                }
            }
        }

        debug!("    x264.encode_frame: {:#?}", time.elapsed());

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
            let mut offset: f32 =
                self.orig_pic.pic.img.i_stride[i] as f32 * top as f32 * csp_height[i];
            offset += left as f32 * csp_width[i];

            // grab the offset ptrs
            // Copy data into fg_pic
            unsafe {
                offset_plane[i] = self.orig_pic.pic.img.plane[i].offset(offset.round() as isize);

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
                    src_ptr = src_ptr.offset(self.orig_pic.pic.img.i_stride[i].try_into().unwrap());
                    dst_ptr = dst_ptr.offset(self.fg_pic.pic.img.i_stride[i].try_into().unwrap());
                }
            }
        }
    }
}
