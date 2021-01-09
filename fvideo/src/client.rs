//! Struct for the video client.
//!
//! The client is responsible for gathering gaze data to send to the
//! server/encoder, and decoding/displaying the resulting frames.

extern crate ffmpeg_next as ffmpeg;

use std::cmp;
use std::collections::VecDeque;
use std::convert::TryInto;
use std::path::PathBuf;
use std::process;
use std::time::{Duration, Instant};

use ffmpeg::util::format::pixel::Pixel;
use ffmpeg::util::frame::video::Video;
use ffmpeg::{codec, decoder, Packet};
use log::{debug, error, info};
use sdl2::event::EventType;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, TextureCreator};
use sdl2::video::{Window, WindowContext};
use sdl2::EventPump;

use crate::twostreamserver::{RESCALE_HEIGHT, RESCALE_WIDTH};
use crate::{Calibrate, FoveationAlg, GazeSample, GazeSource, Record, EDF_FILE};
use eyelink_rs::ascparser::{self, EyeSample};
use eyelink_rs::libeyelink_sys::MISSING_DATA;
use eyelink_rs::{self, eyelink, EyeData, OpenMode};
use x264::NalData;

pub struct FvideoClient {
    alg: FoveationAlg,
    fg_decoder: decoder::Video,
    bg_decoder: decoder::Video,
    texture_creator: TextureCreator<WindowContext>,
    canvas: Canvas<Window>,
    event_pump: EventPump,
    fg_width: u32,
    fg_height: u32,
    bg_width: u32,
    bg_height: u32,
    src_width: u32,
    src_height: u32,
    disp_width: u32,
    disp_height: u32,
    total_bytes: u64,
    frame_idx: u64,
    gaze_source: GazeSource,
    gaze_samples: VecDeque<GazeSample>,
    last_gaze_sample: GazeSample,
    eye_used: Option<EyeData>,
    trace_samples: Option<VecDeque<EyeSample>>,
    record: Record,
    triggered: bool,
    alpha_blend: Vec<u8>,
    bg_frame: Video,
    seqno: u64,
    delay: Option<Duration>,
}

impl Drop for FvideoClient {
    fn drop(&mut self) {
        if self.gaze_source == GazeSource::Eyelink {
            match self.record {
                Record::Yes => {
                    if let Err(e) = eyelink::stop_recording(EDF_FILE) {
                        error!("Failed stopping recording: {}", e);
                        process::exit(1);
                    }
                }
                Record::No => {
                    if let Err(e) = eyelink::stop_recording(None) {
                        error!("Failed stopping recording: {}", e);
                        process::exit(1);
                    }
                }
            }

            eyelink_rs::close_eyelink_connection();
        }

        // Make sure to flush decoder.
        self.fg_decoder.flush();
        self.bg_decoder.flush();
    }
}

// TODO(lukehsiao): Switch to the builder pattern?
impl FvideoClient {
    pub fn new<T: Into<Option<PathBuf>>>(
        alg: FoveationAlg,
        fovea: u32,
        width: u32,
        height: u32,
        delay: u64,
        gaze_source: GazeSource,
        cal: Calibrate,
        record: Record,
        trace: T,
    ) -> FvideoClient {
        let mut eye_used = None;
        let mut trace_samples = None;
        match gaze_source {
            GazeSource::Eyelink => {
                if let Err(e) = eyelink::initialize_eyelink(OpenMode::Real) {
                    error!("Failed Eyelink Initialization: {}", e);
                    process::exit(1);
                }

                match cal {
                    Calibrate::Yes => {
                        if let Err(e) = eyelink::run_calibration() {
                            error!("Failed Eyelink Calibration: {}", e);
                            process::exit(1);
                        }
                    }
                    Calibrate::No => {
                        info!("Skipping calibration.");
                    }
                }

                match record {
                    Record::Yes => {
                        info!("Recording eye-trace to {}.", EDF_FILE);
                        if let Err(e) = eyelink::start_recording(EDF_FILE) {
                            error!("Failed starting recording: {}", e);
                            process::exit(1);
                        }
                    }
                    Record::No => {
                        if let Err(e) = eyelink::start_recording(None) {
                            error!("Failed starting recording: {}", e);
                            process::exit(1);
                        }
                    }
                }

                if let Err(e) = eyelink_rs::eyelink_wait_for_block_start(100, 1, 0) {
                    error!("No link samples received: {}", e);
                    process::exit(1);
                }

                eye_used = match eyelink_rs::eyelink_eye_available() {
                    Ok(e) => {
                        debug!("Eye data from: {:?}", e);
                        Some(e)
                    }
                    Err(e) => {
                        error!("No eye data available: {}", e);
                        process::exit(1);
                    }
                };

                // Flush and only look at the most recent button press
                if let Err(e) = eyelink_rs::eyelink_flush_keybuttons(0) {
                    error!("Unable to flush buttons: {}", e);
                    process::exit(1);
                }
            }
            GazeSource::Mouse => (),
            GazeSource::TraceFile => {
                let trace = trace.into().expect("Missing trace file path.");
                trace_samples = match ascparser::parse_asc(trace) {
                    Err(e) => {
                        error!("Unable to parse ASC file: {}", e);
                        process::exit(1);
                    }
                    Ok(s) => Some(VecDeque::from(s)),
                };
            }
        }

        let fg_decoder = decoder::new()
            .open_as(decoder::find(codec::Id::H264))
            .unwrap()
            .video()
            .unwrap();

        let bg_decoder = decoder::new()
            .open_as(decoder::find(codec::Id::H264))
            .unwrap()
            .video()
            .unwrap();

        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();
        let mut event_pump = sdl_context.event_pump().unwrap();

        let (disp_width, disp_height) = {
            let disp_rect = video_subsystem.display_bounds(0).unwrap();
            (disp_rect.w as u32, disp_rect.h as u32)
        };

        let window = video_subsystem
            .window("fvideo.rs", disp_width, disp_height)
            .fullscreen_desktop()
            .build()
            .unwrap();

        let canvas = window
            .into_canvas()
            .accelerated()
            .target_texture()
            .build()
            .unwrap();

        event_pump.enable_event(EventType::MouseMotion);
        event_pump.pump_events();

        // 0 is immediate update
        // 1 synchronizes with vertical retrace
        // -1 for adaptive vsync
        video_subsystem.gl_set_swap_interval(0).unwrap();

        let texture_creator = canvas.texture_creator();

        let last_gaze_sample = GazeSample {
            time: Instant::now(),
            seqno: 0,
            d_width: disp_width,
            d_height: disp_height,
            d_x: disp_width / 2,
            d_y: disp_height / 2,
            p_x: width / 2,
            p_y: height / 2,
            m_x: width / 2 / 16,
            m_y: height / 2 / 16,
        };
        let mut gaze_samples = VecDeque::new();
        gaze_samples.reserve(256);
        gaze_samples.push_back(GazeSample {
            time: Instant::now(),
            seqno: 0,
            d_width: disp_width,
            d_height: disp_height,
            d_x: disp_width / 2,
            d_y: disp_height / 2,
            p_x: width / 2,
            p_y: height / 2,
            m_x: width / 2 / 16,
            m_y: height / 2 / 16,
        });

        let fovea_size = match fovea {
            n if n * 16 > height => height,
            0 => panic!("Error"), // this is "no foveation"
            n => n * 16,
        };

        let (fg_width, fg_height, bg_width, bg_height) = match alg {
            FoveationAlg::TwoStream => (fovea_size, fovea_size, RESCALE_WIDTH, RESCALE_HEIGHT),
            _ => (width, height, width, height),
        };

        // Set the alpha values based on a circular 2D Gaussian. These constants right now
        // are just tuned to what seems to look OK to me. See commit msg for details.
        let mut alpha_blend: Vec<u8> = vec![];
        for j in 0..fg_width {
            for i in 0..fg_width {
                alpha_blend.push(cmp::min(
                    255,
                    (768.0
                        * (-1.0
                            * (((i as i32 - (fg_width / 2) as i32).pow(2)
                                + (j as i32 - (fg_width / 2) as i32).pow(2))
                                as f32
                                / (2.0 * (fg_width as f32 / 5.0).powi(2))))
                        .exp())
                    .round() as u8,
                ));
            }
        }

        FvideoClient {
            alg,
            fg_decoder,
            bg_decoder,
            texture_creator,
            canvas,
            event_pump,
            fg_width,
            fg_height,
            bg_width,
            bg_height,
            src_width: width,
            src_height: height,
            disp_width,
            disp_height,
            total_bytes: 0,
            frame_idx: 0,
            gaze_source,
            gaze_samples,
            last_gaze_sample,
            eye_used,
            trace_samples,
            record,
            triggered: false,
            alpha_blend,
            bg_frame: Video::empty(),
            seqno: 0,
            delay: if delay > 0 {
                Some(Duration::from_millis(delay))
            } else {
                None
            },
        }
    }

    /// Repeatedly checks the latest gaze sample until a threshold is exceeded.
    pub fn triggered_gaze_sample(&mut self, thresh: i32) -> GazeSample {
        loop {
            if let Ok(evt) = eyelink_rs::eyelink_newest_float_sample() {
                let idx = match self.eye_used.as_ref() {
                    Some(EyeData::Left) => 0,
                    Some(EyeData::Right) => 1,
                    Some(EyeData::Binocular) => 0, // if both eyes used, still just use left
                    None => {
                        error!("No eye data was found.");
                        process::exit(1);
                    }
                };

                let d_x = evt.gx[idx];
                let d_y = evt.gy[idx];

                let pa = evt.pa[idx];

                // Make sure pupil is present
                if d_x as i32 != MISSING_DATA && d_y as i32 != MISSING_DATA && pa > 0.0 {
                    // Scale from display to video resolution
                    let p_x = d_x * self.bg_width as f32 / self.disp_width as f32;
                    let p_y = d_y * self.bg_height as f32 / self.disp_height as f32;

                    let gaze = GazeSample {
                        time: Instant::now(),
                        seqno: self.seqno,
                        d_width: self.disp_width,
                        d_height: self.disp_height,
                        d_x: d_x as u32,
                        d_y: d_y as u32,
                        p_x: p_x.round() as u32,
                        p_y: p_y.round() as u32,
                        m_x: (p_x / 16.0).round() as u32,
                        m_y: (p_y / 16.0).round() as u32,
                    };

                    let curr_gaze_sample = *self.gaze_samples.front().unwrap();
                    if (gaze.p_x as i32 - curr_gaze_sample.p_x as i32).abs() > thresh
                        || (gaze.p_y as i32 - curr_gaze_sample.p_y as i32).abs() > thresh
                    {
                        self.gaze_samples.push_back(gaze);
                        self.last_gaze_sample = self.gaze_samples.pop_front().unwrap();
                        self.triggered = true;
                        self.seqno += 1;
                        return curr_gaze_sample;
                    }
                    self.gaze_samples.push_back(gaze);
                    self.last_gaze_sample = self.gaze_samples.pop_front().unwrap();
                }
            }
        }
    }

    /// Get the latest gaze sample, if one is available.
    pub fn gaze_sample(&mut self) -> GazeSample {
        let mut gaze = match self.gaze_source {
            GazeSource::Mouse => {
                // Grab mouse position using SDL2.
                if self.event_pump.poll_iter().last().is_some() {
                    let d_x = self.event_pump.mouse_state().x() as u32;
                    let d_y = self.event_pump.mouse_state().y() as u32;

                    // Scale from display to video resolution
                    let p_x = d_x as f32 * self.bg_width as f32 / self.disp_width as f32;
                    let p_y = d_y as f32 * self.bg_height as f32 / self.disp_height as f32;

                    GazeSample {
                        time: Instant::now(),
                        seqno: self.seqno,
                        d_width: self.disp_width,
                        d_height: self.disp_height,
                        d_x,
                        d_y,
                        p_x: p_x.round() as u32,
                        p_y: p_y.round() as u32,
                        m_x: (p_x / 16.0).round() as u32,
                        m_y: (p_y / 16.0).round() as u32,
                    }
                } else {
                    *self.gaze_samples.back().unwrap()
                }
            }
            GazeSource::Eyelink => {
                if let Ok(evt) = eyelink_rs::eyelink_newest_float_sample() {
                    let idx = match self.eye_used.as_ref() {
                        Some(EyeData::Left) => 0,
                        Some(EyeData::Right) => 1,
                        Some(EyeData::Binocular) => 0, // if both eyes used, still just use left
                        None => {
                            error!("No eye data was found.");
                            process::exit(1);
                        }
                    };

                    let d_x = evt.gx[idx];
                    let d_y = evt.gy[idx];

                    let pa = evt.pa[idx];

                    // Make sure pupil is present
                    if d_x as i32 != MISSING_DATA && d_y as i32 != MISSING_DATA && pa > 0.0 {
                        // Scale from display to video resolution
                        let p_x = d_x * self.bg_width as f32 / self.disp_width as f32;
                        let p_y = d_y * self.bg_height as f32 / self.disp_height as f32;

                        GazeSample {
                            time: Instant::now(),
                            seqno: self.seqno,
                            d_width: self.disp_width,
                            d_height: self.disp_height,
                            d_x: d_x.round() as u32,
                            d_y: d_y.round() as u32,
                            p_x: p_x.round() as u32,
                            p_y: p_y.round() as u32,
                            m_x: (p_x / 16.0).round() as u32,
                            m_y: (p_y / 16.0).round() as u32,
                        }
                    } else {
                        *self.gaze_samples.back().unwrap()
                    }
                } else {
                    *self.gaze_samples.back().unwrap()
                }
            }
            GazeSource::TraceFile => {
                info!("{:?}", self.trace_samples.as_ref().unwrap().front());
                todo!();
                // TODO(lukehsiao): how to determine what the right trace sample
                // to use is? How to properly "align" this with the video?

                // while comsuming all the front samples that are old
                // Grab the sample for this time, which is now at the front.
                //
                // for sample in self.trace_samples.unwrap() {
                //     self.trace_samples.pop_front();
                //     self.curr_gaze_sample = GazeSample {
                //         time: Instant::now(),
                //         p_x: p_x.round() as u32,
                //         p_y: p_y.round() as u32,
                //         m_x: (p_x / 16.0).round() as u32,
                //         m_y: (p_y / 16.0).round() as u32,
                //     }
                // }
            }
        };
        gaze.time = Instant::now();
        gaze.seqno = self.seqno;
        self.seqno += 1;
        self.gaze_samples.push_back(gaze);

        // Allow artificial delay to determine when to release the next gaze sample
        match self.delay {
            Some(delay) => {
                if self.gaze_samples.front().unwrap().time.elapsed() >= delay {
                    self.last_gaze_sample = self.gaze_samples.pop_front().unwrap();
                }
            }
            None => self.last_gaze_sample = self.gaze_samples.pop_front().unwrap(),
        }

        *self.gaze_samples.front().unwrap()
    }

    /// Utility function for immediately drawing a white square to the bottom
    /// left corner of the display. Useful for debugging timing.
    ///
    /// In particular, this is intended to be used with a photodiode like the
    /// one in <https://github.com/lukehsiao/eyelink-latency>.
    pub fn display_white(&mut self, height: u32, dim: u32) {
        self.canvas.set_draw_color(Color::WHITE);
        match self
            .canvas
            .fill_rect(Rect::new(0, (height - dim).try_into().unwrap(), dim, dim))
        {
            Ok(_) => {
                self.canvas.present();
            }
            Err(e) => {
                error!("Failed drawing rectangle: {}.", e);
            }
        }

        self.frame_idx += 1;
    }

    /// Utility function for clearing a screen with all black.
    pub fn clear(&mut self) {
        self.canvas.set_draw_color(Color::BLACK);
        self.canvas.clear();
        self.canvas.present();
        self.frame_idx += 1;
    }

    fn display_onestream_frame(&mut self, nal: &NalData) {
        let time = Instant::now();

        let mut texture = self
            .texture_creator
            .create_texture_streaming(PixelFormatEnum::YV12, self.bg_width, self.bg_height)
            .unwrap();

        if self.triggered {
            info!("    init texture: {:#?}", time.elapsed());
        } else {
            debug!("    init texture: {:#?}", time.elapsed());
        }

        let dec_time = Instant::now();
        let packet = Packet::copy(nal.as_bytes());
        self.total_bytes += packet.size() as u64;
        let mut frame = Video::empty();
        match self.bg_decoder.decode(&packet, &mut frame) {
            Ok(true) => {
                if self.triggered {
                    info!("    decode nal: {:?}", dec_time.elapsed());
                } else {
                    debug!("    decode nal: {:?}", dec_time.elapsed());
                }

                let time = Instant::now();
                let rect = Rect::new(0, 0, frame.width(), frame.height());
                let _ = texture.update_yuv(
                    rect,
                    frame.data(0),
                    frame.stride(0),
                    frame.data(1),
                    frame.stride(1),
                    frame.data(2),
                    frame.stride(2),
                );
                let _ = self.canvas.copy(&texture, None, None);
                self.canvas.present();

                self.frame_idx += 1;
                if self.triggered {
                    info!("    display new frame: {:?}", time.elapsed());
                } else {
                    debug!("    display new frame: {:?}", time.elapsed());
                }
            }
            Ok(false) => (),
            Err(_) => {
                error!("Error occured while decoding packet.");
            }
        }
    }

    fn display_twostream_frame(&mut self, fg_nal: &NalData, bg_nal: Option<&NalData>) {
        let time = Instant::now();

        let mut fg_texture = self
            .texture_creator
            .create_texture_streaming(PixelFormatEnum::ABGR8888, self.fg_width, self.fg_height)
            .unwrap();
        fg_texture.set_blend_mode(BlendMode::Blend);

        if self.triggered {
            info!("    init texture: {:#?}", time.elapsed());
        } else {
            debug!("    init texture: {:#?}", time.elapsed());
        }

        let dec_time = Instant::now();
        let fg_packet = Packet::copy(fg_nal.as_bytes());
        self.total_bytes += fg_packet.size() as u64;
        let mut fg_frame = Video::empty();
        let mut fg_frame_rgba = Video::empty();

        // If there is a new bg frame, update it
        if let Some(bg) = bg_nal {
            let bg_packet = Packet::copy(bg.as_bytes());
            self.total_bytes += bg_packet.size() as u64;
            match self.bg_decoder.decode(&bg_packet, &mut self.bg_frame) {
                Ok(true) => (),
                Ok(false) => unimplemented!(),
                Err(_) => {
                    error!("Error occured while decoding packet.");
                }
            }
        }

        match self.fg_decoder.decode(&fg_packet, &mut fg_frame) {
            Ok(true) => {
                if self.triggered {
                    info!("    decode nal: {:?}", dec_time.elapsed());
                } else {
                    debug!("    decode nal: {:?}", dec_time.elapsed());
                }

                let time = Instant::now();
                let fg_rect = Rect::new(0, 0, fg_frame.width(), fg_frame.height());

                let mut converter = fg_frame.converter(Pixel::RGBA).unwrap();
                converter.run(&fg_frame, &mut fg_frame_rgba).unwrap();

                // Manipulate the alpha channel to give blend at the edges
                let height = fg_frame_rgba.height();
                let width = fg_frame_rgba.stride(0);
                let rgba_data = fg_frame_rgba.data_mut(0);

                let mut alpha_iter = self.alpha_blend.iter();
                for j in 0..height {
                    for i in (0..width).step_by(4) {
                        rgba_data[(width * j as usize) + i + 3] = *alpha_iter.next().unwrap();
                    }
                }

                let _ = fg_texture.update(fg_rect, fg_frame_rgba.data(0), fg_frame_rgba.stride(0));

                let mut bg_texture = self
                    .texture_creator
                    .create_texture_streaming(PixelFormatEnum::YV12, self.bg_width, self.bg_height)
                    .unwrap();

                let bg_rect = Rect::new(0, 0, self.bg_frame.width(), self.bg_frame.height());
                let _ = bg_texture.update_yuv(
                    bg_rect,
                    self.bg_frame.data(0),
                    self.bg_frame.stride(0),
                    self.bg_frame.data(1),
                    self.bg_frame.stride(1),
                    self.bg_frame.data(2),
                    self.bg_frame.stride(2),
                );

                // Scale fg square to match the bg scaling.
                let c_x = self.last_gaze_sample.d_x as i32;
                let c_y = self.last_gaze_sample.d_y as i32;
                let scaled_fg_rect = Rect::from_center(
                    (c_x, c_y),
                    fg_rect.width() * self.disp_width / self.src_width,
                    fg_rect.height() * self.disp_height / self.src_height,
                );

                self.canvas.clear();
                let _ = self.canvas.copy(&bg_texture, None, None);
                let _ = self.canvas.copy(&fg_texture, None, scaled_fg_rect);
                self.canvas.present();

                self.frame_idx += 1;
                if self.triggered {
                    info!("    display new frame: {:?}", time.elapsed());
                } else {
                    debug!("    display new frame: {:?}", time.elapsed());
                }
            }
            Ok(false) => (),
            Err(_) => {
                error!("Error occured while decoding packet.");
            }
        }
    }

    /// Decode and display the provided frame.
    pub fn display_frame<'a, T>(&mut self, fg_nal: T, bg_nal: T)
    where
        T: Into<Option<&'a NalData>>,
    {
        match self.alg {
            FoveationAlg::TwoStream => {
                self.display_twostream_frame(fg_nal.into().unwrap(), bg_nal.into())
            }
            _ => self.display_onestream_frame(bg_nal.into().unwrap()),
        }
    }

    pub fn total_frames(&self) -> u64 {
        self.frame_idx
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
}
