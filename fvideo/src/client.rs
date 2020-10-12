//! Struct for the video client.
//!
//! The client is responsible for gathering gaze data to send to the
//! server/encoder, and decoding/displaying the resulting frames.

extern crate ffmpeg_next as ffmpeg;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;
use std::{num, process};

use ffmpeg::util::frame::video::Video;
use ffmpeg::{codec, decoder, Packet};
use log::{debug, error, info};
use sdl2::event::EventType;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{Canvas, TextureCreator};
use sdl2::video::{Window, WindowContext};
use sdl2::EventPump;
use structopt::clap::arg_enum;
use thiserror::Error;

use crate::GazeSample;
use eyelink_rs::ascparser::{self, EyeSample};
use eyelink_rs::libeyelink_sys::MISSING_DATA;
use eyelink_rs::{self, eyelink, EyeData, OpenMode};
use x264::NalData;

// TODO(lukehsiao): "test.edf" works, but this breaks for unknown reasons for
// other filenames (like "recording.edf"). Not sure why.
const EDF_FILE: &str = "test.edf";

arg_enum! {
    #[derive(Copy, Clone, PartialEq, Debug)]
    pub enum GazeSource {
        Mouse,
        Eyelink,
        TraceFile,
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

pub struct FvideoClient {
    decoder: decoder::Video,
    frame: Video,
    texture_creator: TextureCreator<WindowContext>,
    canvas: Canvas<Window>,
    event_pump: EventPump,
    vid_width: u32,
    vid_height: u32,
    disp_width: u32,
    disp_height: u32,
    total_bytes: u64,
    frame_idx: u64,
    gaze_source: GazeSource,
    last_gaze_sample: GazeSample,
    eye_used: Option<EyeData>,
    trace_samples: Option<VecDeque<EyeSample>>,
}

impl Drop for FvideoClient {
    fn drop(&mut self) {
        if self.gaze_source == GazeSource::Eyelink {
            if let Err(e) = eyelink::stop_recording(EDF_FILE) {
                error!("Failed stopping recording: {}", e);
                process::exit(1);
            }

            eyelink_rs::close_eyelink_connection();
        }

        // Make sure to flush decoder.
        self.decoder.flush();
    }
}

impl FvideoClient {
    pub fn new(
        vid_width: u32,
        vid_height: u32,
        gaze_source: GazeSource,
        skip_cal: bool,
        trace: Option<PathBuf>,
    ) -> FvideoClient {
        let mut eye_used = None;
        let mut trace_samples = None;
        match gaze_source {
            GazeSource::Eyelink => {
                if let Err(e) = eyelink::initialize_eyelink(OpenMode::Real) {
                    error!("Failed Eyelink Initialization: {}", e);
                    process::exit(1);
                }

                if skip_cal {
                    info!("Skipping calibration.");
                } else if let Err(e) = eyelink::run_calibration() {
                    error!("Failed Eyelink Calibration: {}", e);
                    process::exit(1);
                }

                if let Err(e) = eyelink::start_recording(EDF_FILE) {
                    error!("Failed starting recording: {}", e);
                    process::exit(1);
                }

                if let Err(e) = eyelink_rs::eyelink_wait_for_block_start(100, 1, 0) {
                    error!("No link samples received: {}", e);
                    process::exit(1);
                }

                eye_used = match eyelink_rs::eyelink_eye_available() {
                    Ok(e) => {
                        info!("Eye data from: {:?}", e);
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
                let trace = trace.expect("Missing trace file path.");
                trace_samples = match ascparser::parse_asc(trace) {
                    Err(e) => {
                        error!("Unable to parse ASC file: {}", e);
                        process::exit(1);
                    }
                    Ok(s) => Some(VecDeque::from(s)),
                };
            }
        }
        let decoder = decoder::new()
            .open_as(decoder::find(codec::Id::H264))
            .unwrap()
            .video()
            .unwrap();

        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();
        let mut event_pump = sdl_context.event_pump().unwrap();

        let window = video_subsystem
            .window("fvideo.rs", vid_width, vid_height)
            .fullscreen_desktop()
            // .position_centered()
            .build()
            .unwrap();

        let canvas = window
            .into_canvas()
            .accelerated()
            // .present_vsync()
            .target_texture()
            .build()
            .unwrap();

        let (disp_width, disp_height) = {
            let disp_rect = video_subsystem.display_bounds(0).unwrap();
            (disp_rect.w as u32, disp_rect.h as u32)
        };

        event_pump.enable_event(EventType::MouseMotion);
        event_pump.pump_events();

        // 0 is immediate update
        video_subsystem.gl_set_swap_interval(0).unwrap();

        let texture_creator = canvas.texture_creator();

        let last_gaze_sample = GazeSample {
            time: Instant::now(),
            p_x: vid_width / 2,
            p_y: vid_height / 2,
            m_x: vid_width / 2 / 16,
            m_y: vid_height / 2 / 16,
        };

        FvideoClient {
            decoder,
            frame: Video::empty(),
            texture_creator,
            canvas,
            event_pump,
            vid_width,
            vid_height,
            disp_width,
            disp_height,
            total_bytes: 0,
            frame_idx: 0,
            gaze_source,
            last_gaze_sample,
            eye_used,
            trace_samples,
        }
    }

    /// Get the latest gaze sample, if one is available.
    ///
    /// Note: This currently uses mouse position as a substitute for Eyelink data.
    pub fn gaze_sample(&mut self) -> GazeSample {
        match self.gaze_source {
            GazeSource::Mouse => {
                // Grab mouse position using SDL2.
                if self.event_pump.poll_iter().last().is_some() {
                    let mut p_x = self.event_pump.mouse_state().x() as u32;
                    let mut p_y = self.event_pump.mouse_state().y() as u32;

                    // Scale from display to video resolution
                    p_x = (p_x as f64 * (self.vid_width as f64 / self.disp_width as f64)) as u32;
                    p_y = (p_y as f64 * (self.vid_height as f64 / self.disp_height as f64)) as u32;

                    self.last_gaze_sample = GazeSample {
                        time: Instant::now(),
                        p_x,
                        p_y,
                        m_x: p_x / 16,
                        m_y: p_y / 16,
                    };
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

                    let mut p_x = evt.gx[idx];
                    let mut p_y = evt.gy[idx];

                    let pa = evt.pa[idx];

                    // Make sure pupil is present
                    if p_x as i32 != MISSING_DATA && p_y as i32 != MISSING_DATA && pa > 0.0 {
                        // Scale from display to video resolution
                        p_x *= self.vid_width as f32 / self.disp_width as f32;
                        p_y *= self.vid_height as f32 / self.disp_height as f32;

                        self.last_gaze_sample = GazeSample {
                            time: Instant::now(),
                            p_x: p_x.round() as u32,
                            p_y: p_y.round() as u32,
                            m_x: (p_x / 16.0).round() as u32,
                            m_y: (p_y / 16.0).round() as u32,
                        };
                    }
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
                //     self.last_gaze_sample = GazeSample {
                //         time: Instant::now(),
                //         p_x: p_x.round() as u32,
                //         p_y: p_y.round() as u32,
                //         m_x: (p_x / 16.0).round() as u32,
                //         m_y: (p_y / 16.0).round() as u32,
                //     }
                // }
            }
        }

        self.last_gaze_sample
    }

    /// Decode and display the provided frame.
    pub fn display_frame(&mut self, nal: &NalData) {
        let time = Instant::now();
        let mut texture = self
            .texture_creator
            .create_texture_streaming(
                PixelFormatEnum::YV12,
                self.vid_width as u32,
                self.vid_height as u32,
            )
            .unwrap();
        debug!("    init texture: {:#?}", time.elapsed());

        let time = Instant::now();
        let packet = Packet::copy(nal.as_bytes());
        self.total_bytes += packet.size() as u64;
        match self.decoder.decode(&packet, &mut self.frame) {
            Ok(true) => {
                debug!("    decode nal: {:?}", time.elapsed());
                let time = Instant::now();
                let rect = Rect::new(0, 0, self.frame.width(), self.frame.height());
                let _ = texture.update_yuv(
                    rect,
                    self.frame.data(0),
                    self.frame.stride(0),
                    self.frame.data(1),
                    self.frame.stride(1),
                    self.frame.data(2),
                    self.frame.stride(2),
                );

                // TODO(lukehsiao): Is this copy slow?
                let _ = self.canvas.copy(&texture, None, None);
                self.canvas.present();

                self.frame_idx += 1;
                debug!("    display new frame: {:?}", time.elapsed());
            }
            Ok(false) => (),
            Err(_) => {
                error!("Error occured while decoding packet.");
            }
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
