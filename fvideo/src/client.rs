//! Struct for the video client.
//!
//! The client is responsible for gathering gaze data to send to the
//! server/encoder, and decoding/displaying the resulting frames.

extern crate ffmpeg_next as ffmpeg;

use std::num;
use std::time::Instant;

use ffmpeg::util::frame::video::Video;
use ffmpeg::{codec, decoder, Packet};
use log::error;
use sdl2::event::EventType;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{Canvas, TextureCreator};
use sdl2::video::{Window, WindowContext};
use sdl2::EventPump;
use structopt::clap::arg_enum;
use thiserror::Error;
use x264::NalData;

use crate::GazeSample;

arg_enum! {
    #[derive(Debug)]
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
}

impl Drop for FvideoClient {
    fn drop(&mut self) {
        // Make sure to flush decoder.
        self.decoder.flush();
    }
}

impl FvideoClient {
    pub fn new(vid_width: u32, vid_height: u32, gaze_source: GazeSource) -> FvideoClient {
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
            GazeSource::Eyelink => todo!(),
            GazeSource::TraceFile => todo!(),
        }

        self.last_gaze_sample
    }

    /// Decode and display the provided frame.
    pub fn display_frame(&mut self, nal: NalData) {
        let mut texture = self
            .texture_creator
            .create_texture_streaming(
                PixelFormatEnum::YV12,
                self.vid_width as u32,
                self.vid_height as u32,
            )
            .unwrap();

        let packet = Packet::copy(nal.as_bytes());
        self.total_bytes += packet.size() as u64;
        match self.decoder.decode(&packet, &mut self.frame) {
            Ok(true) => {
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

                self.canvas.clear();
                // TODO(lukehsiao): Is this copy slow?
                let _ = self.canvas.copy(&texture, None, None);
                self.canvas.present();

                self.frame_idx += 1;
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
