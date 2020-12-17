//! Implementation for the two-stream foveated video encoding server.

extern crate ffmpeg_next as ffmpeg;

use std::cmp;
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
use x264::{Encoder, NalData, Picture};

use crate::{FvideoServerError, GazeSample};

/// Server/Encoder Struct
pub struct FvideoTwoStreamServer {
    _fovea: i32,
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
    timestamp: i64,
}

pub const CROP_WIDTH: u32 = 160;
pub const CROP_HEIGHT: u32 = 160;
pub const RESCALE_WIDTH: u32 = 1024;
pub const RESCALE_HEIGHT: u32 = 576;

impl FvideoTwoStreamServer {
    pub fn new(fovea: i32, video: PathBuf) -> Result<FvideoTwoStreamServer, FvideoServerError> {
        let video_in = File::open(video)?;
        let mut video_in = BufReader::new(video_in);

        // First, read dimensions/FPS from Y4M header.
        // This is done manually so that the header is already skipped once the server is
        // initialized.
        let mut hdr = String::new();
        video_in.read_line(&mut hdr).unwrap();
        let (width, height, fps) = crate::parse_y4m_header(&hdr)?;

        let frame_dur = Duration::from_secs_f64(1.0 / fps);

        let orig_par = crate::setup_x264_params(width, height, 24)?;
        let orig_pic = Picture::from_param(&orig_par)?;

        // foreground stream is cropped
        let mut fg_par = crate::setup_x264_params(CROP_WIDTH, CROP_HEIGHT, 24)?;
        let fg_pic = Picture::from_param(&fg_par)?;
        let fg_encoder = Encoder::open(&mut fg_par)
            .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        // background stream is scaled
        let mut bg_par = crate::setup_x264_params(RESCALE_WIDTH, RESCALE_HEIGHT, 32)?;
        let bg_pic = Picture::from_param(&bg_par)?;
        let bg_encoder = Encoder::open(&mut bg_par)
            .map_err(|s| FvideoServerError::EncoderError(s.to_string()))?;

        let scaler = Context::get(
            Pixel::YUV420P,
            width,
            height,
            Pixel::YUV420P,
            RESCALE_WIDTH,
            RESCALE_HEIGHT,
            Flags::FAST_BILINEAR,
        )?;

        let frame_time = Instant::now();

        Ok(FvideoTwoStreamServer {
            _fovea: fovea,
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
                let mut buf = self.orig_pic.as_mut_slice(plane).unwrap();
                self.video_in.read_exact(&mut buf)?;
            }
        }

        Ok(())
    }

    // TODO(lukehsiao): I don't like this return type. At some point we should pull this into a
    // trait or something to have a more clear interface.
    pub fn encode_frame(
        &mut self,
        gaze: GazeSample,
    ) -> Result<Vec<(Option<NalData>, NalData)>, FvideoServerError> {
        let time = Instant::now();
        self.read_frame()?;
        debug!("    read_frame: {:#?}", time.elapsed());

        //TODO(lukehsiao): Need to scale back up the gaze sample for for the original picture, or
        //only contain display coordinates and scale before use.
        // Scale gaze sample back up to original
        // Scale from display to video resolution
        // p_x *= self.bg_width as f32 / self.disp_width as f32;
        // p_y *= self.bg_height as f32 / self.disp_height as f32;
        //
        // let gaze = GazeSample {
        //     time: Instant::now(),
        //     p_x: p_x.round() as u32,
        //     p_y: p_y.round() as u32,
        //     m_x: (p_x / 16.0).round() as u32,
        //     m_y: (p_y / 16.0).round() as u32,
        // };

        // Crop section into fg_pic
        self.crop_x264_pic(&gaze, CROP_WIDTH, CROP_HEIGHT)?;

        // Rescale to bg_pic. This drops FPS from ~1500 to ~270 on panda. Using
        // fast_bilinear rather than bilinear gives about 800fps.
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
        self.fg_pic.set_timestamp(self.timestamp);
        self.timestamp += 1;

        let time = Instant::now();
        let mut nals = vec![];

        // TODO(lukehsiao): These is trying to encode both streams in sync. In reality, the whole
        // low quality stream could be sent beforehand, perhaps in lower FPS. Only the foreground
        // high quality stream needs to be high FPS.
        match (
            self.fg_encoder.encode(&self.fg_pic).unwrap(),
            self.bg_encoder.encode(&self.bg_pic).unwrap(),
        ) {
            (Some((fg_nal, _, _)), Some((bg_nal, _, _))) => {
                nals.push((Some(fg_nal), bg_nal));
            }
            (_, _) => {
                warn!("Didn't encode a nal?");
            }
        }

        while self.fg_encoder.delayed_frames() {
            todo!();
            // if let Some((nal, _, _)) = self.fg_encoder.encode(None).unwrap() {
            //     fg_nals.push(nal);
            // }
        }

        debug!("    x264.encode_frame: {:#?}", time.elapsed());

        Ok(nals)
    }

    /// Crop orig_pic centered around the gaze and place into fg_pic.
    fn crop_x264_pic(
        &mut self,
        gaze: &GazeSample,
        width: u32,
        height: u32,
    ) -> Result<(), FvideoServerError> {
        // Scale from disp coordinates to original video coordinates
        let p_y = gaze.d_y * self.height as f32;
        let p_x = gaze.d_x * self.width as f32;

        // Keep the "cropped" window contained in the frame.
        // Only allow multiples of 2 to maintain integer values after division
        let top: u32 = match cmp::max(p_y.round() as i32 - height as i32 / 2, 0) {
            n if n > 0 && n % 2 == 0 => n as u32,
            n if n > 0 && n % 2 != 0 => n as u32 + 1,
            _ => 0,
        };
        let left: u32 = match cmp::max(p_x.round() as i32 - width as i32 / 2, 0) {
            n if n > 0 && n % 2 == 0 => n as u32,
            n if n > 0 && n % 2 != 0 => n as u32 + 1,
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

                for _ in 0..(CROP_HEIGHT as f32 * csp_height[i]).round() as u32 {
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
        Ok(())
    }
}
