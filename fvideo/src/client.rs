//! Foveated video compression client.
//!
//! The client is responsible for gathering gaze data to send to the
//! server/encoder, and decoding/displaying the resulting frames.

extern crate ffmpeg_next as ffmpeg;

use std::collections::VecDeque;
use std::convert::TryInto;
use std::time::{Duration, Instant};
use std::{cmp, fmt, process};

use ffmpeg::filter::{self, graph::Graph};
use ffmpeg::util::format::pixel::Pixel;
use ffmpeg::util::frame::video::Video;
use ffmpeg::{codec, decoder, Packet};
use log::{debug, error, info};
use sdl2::event::{Event, EventType};
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, TextureCreator};
use sdl2::video::{Window, WindowContext};
use sdl2::{hint, EventPump};

use crate::{
    Coords, Dims, DisplayOptions, EyelinkOptions, FoveationAlg, GazeSample, GazeSource, EDF_FILE,
};
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
    fg: Dims,
    bg: Dims,
    _src: Dims,
    disp: Dims,
    total_bytes: u64,
    fg_bytes: u64,
    bg_bytes: u64,
    frame_idx: u64,
    gaze_source: GazeSource,
    gaze_samples: VecDeque<GazeSample>,
    eye_used: Option<EyeData>,
    eyelink_options: EyelinkOptions,
    triggered: bool,
    alpha_blend: Vec<u8>,
    bg_frame: Video,
    fg_frame: Video,
    seqno: u64,
    delay: Option<Duration>,
    filter: Graph,
    total_gaze: Coords,
    last_gaze: Coords,
    min_gaze: Coords,
    max_gaze: Coords,
}

impl fmt::Debug for FvideoClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FvideoClient")
            .field("alg", &self.alg)
            .field("fg", &self.fg)
            .field("bg", &self.bg)
            .field("disp", &self.disp)
            .field("total_bytes", &self.total_bytes)
            .field("fg_bytes", &self.fg_bytes)
            .field("bg_bytes", &self.bg_bytes)
            .field("frame_idx", &self.frame_idx)
            .field("gaze_source", &self.gaze_source)
            .field("eyelink_options", &self.eyelink_options)
            .field("triggered", &self.triggered)
            .field("seqno", &self.seqno)
            .field("total_gaze", &self.total_gaze)
            .field("last_gaze", &self.last_gaze)
            .field("min_gaze", &self.min_gaze)
            .field("max_gaze", &self.max_gaze)
            .finish()
    }
}

impl Drop for FvideoClient {
    fn drop(&mut self) {
        if self.gaze_source == GazeSource::Eyelink {
            if self.eyelink_options.record {
                if let Err(e) = eyelink::stop_recording(EDF_FILE) {
                    error!("Failed stopping recording: {}", e);
                    process::exit(1);
                }
            } else if let Err(e) = eyelink::stop_recording(None) {
                error!("Failed stopping recording: {}", e);
                process::exit(1);
            }

            eyelink_rs::close_eyelink_connection();
        }

        // Make sure to flush decoder.
        self.filter.get("in").unwrap().source().flush().unwrap();
        self.fg_decoder.flush();
        self.bg_decoder.flush();
    }
}

// TODO(lukehsiao): Switch to the builder pattern?
impl FvideoClient {
    pub fn new(
        alg: FoveationAlg,
        fovea: u32,
        src_dims: Dims,
        rescale_dims: Dims,
        display_options: DisplayOptions,
        gaze_source: GazeSource,
        eyelink_options: EyelinkOptions,
    ) -> FvideoClient {
        let mut eye_used = None;
        match gaze_source {
            GazeSource::Eyelink => {
                if let Err(e) = eyelink::initialize_eyelink(OpenMode::Real) {
                    error!("Failed Eyelink Initialization: {}", e);
                    process::exit(1);
                }

                if eyelink_options.calibrate {
                    if let Err(e) = eyelink::run_calibration() {
                        error!("Failed Eyelink Calibration: {}", e);
                        process::exit(1);
                    }
                } else {
                    info!("Skipping calibration.");
                }

                if eyelink_options.record {
                    info!("Recording eye-trace to {}.", EDF_FILE);
                    if let Err(e) = eyelink::start_recording(EDF_FILE) {
                        error!("Failed starting recording: {}", e);
                        process::exit(1);
                    }
                } else if let Err(e) = eyelink::start_recording(None) {
                    error!("Failed starting recording: {}", e);
                    process::exit(1);
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

        // Changes the algorithm used to upscale on display
        if !hint::set("SDL_RENDER_SCALE_QUALITY", "2") {
            error!("Unable to set SDL_RENDER_SCALE_QUALITY");
            panic!();
        }

        let (disp_width, disp_height) = {
            let disp_rect = video_subsystem.display_bounds(0).unwrap();
            (disp_rect.w as u32, disp_rect.h as u32)
        };

        let window = video_subsystem
            .window("fvideo.rs", disp_width, disp_height)
            .fullscreen_desktop()
            .build()
            .unwrap();

        let mut canvas = window
            .into_canvas()
            .accelerated()
            .target_texture()
            .build()
            .unwrap();

        // Clip the drawn area to the resolution of the source video.
        //
        // The goal is to avoid any scaling when displaying a video. Note that this means that it
        // will no longer make sense to do development on a display that has a lower resolution than
        // the source video itself (need a 4k display to work on 4k video).
        canvas.set_clip_rect(Rect::from_center(
            (disp_width as i32 / 2, disp_height as i32 / 2),
            src_dims.width,
            src_dims.height,
        ));

        event_pump.enable_event(EventType::MouseMotion);

        // 0 is immediate update
        // 1 synchronizes with vertical retrace
        // -1 for adaptive vsync
        video_subsystem.gl_set_swap_interval(0).unwrap();

        let texture_creator = canvas.texture_creator();

        let mut gaze_samples = VecDeque::new();
        gaze_samples.reserve(256);
        gaze_samples.push_back(GazeSample {
            time: Instant::now(),
            seqno: 0,
            d_width: disp_width,
            d_height: disp_height,
            d_x: disp_width / 2,
            d_y: disp_height / 2,
            p_x: src_dims.width / 2,
            p_y: src_dims.height / 2,
            m_x: src_dims.width / 2 / 16,
            m_y: src_dims.height / 2 / 16,
        });

        let fovea_size = match fovea {
            n if n * 16 > src_dims.height => src_dims.height,
            0 => panic!("Error"), // this is "no foveation"
            n => n * 16,
        };

        let (fg_width, fg_height, bg_width, bg_height) = match alg {
            FoveationAlg::TwoStream => {
                info!(
                    "fg res: {}x{}, bg_res: {}x{}",
                    fovea_size, fovea_size, rescale_dims.width, rescale_dims.height
                );

                (
                    fovea_size,
                    fovea_size,
                    rescale_dims.width,
                    rescale_dims.height,
                )
            }
            _ => (
                src_dims.width,
                src_dims.height,
                src_dims.width,
                src_dims.height,
            ),
        };

        let alpha_blend = compute_alpha(fg_width);

        let buffer_params = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:sar=1",
            bg_width, bg_height, "yuv420p", 1, 24
        );

        let filter = {
            let mut filter = filter::Graph::new();

            filter
                .add(
                    &filter::find("buffer").unwrap(),
                    "in",           // name
                    &buffer_params, // params
                )
                .unwrap();

            filter
                .add(
                    &filter::find("buffersink").unwrap(),
                    "out", // name
                    "",    // params
                )
                .unwrap();

            let mut inp = filter.get("in").unwrap();
            inp.set_pixel_format(Pixel::YUV420P);

            let mut out = filter.get("out").unwrap();
            out.set_pixel_format(Pixel::YUV420P);

            filter
                .output("in", 0)
                .unwrap()
                .input("out", 0)
                .unwrap()
                .parse(display_options.filter.as_str())
                .unwrap();

            filter.validate().unwrap();

            info!("{}", filter.dump());

            filter
        };

        FvideoClient {
            alg,
            fg_decoder,
            bg_decoder,
            texture_creator,
            canvas,
            event_pump,
            fg: Dims {
                width: fg_width,
                height: fg_height,
            },
            bg: Dims {
                width: bg_width,
                height: bg_height,
            },
            _src: src_dims,
            disp: Dims {
                width: disp_width,
                height: disp_height,
            },
            total_bytes: 0,
            fg_bytes: 0,
            bg_bytes: 0,
            frame_idx: 0,
            gaze_source,
            gaze_samples,
            eye_used,
            eyelink_options,
            triggered: false,
            alpha_blend,
            bg_frame: Video::empty(),
            fg_frame: Video::empty(),
            seqno: 0,
            delay: if display_options.delay > 0 {
                Some(Duration::from_millis(display_options.delay))
            } else {
                None
            },
            filter,
            total_gaze: Coords { x: 0, y: 0 },
            last_gaze: Coords {
                x: u64::from(src_dims.width) / 2,
                y: u64::from(src_dims.height) / 2,
            },
            min_gaze: Coords {
                x: u64::MAX,
                y: u64::MAX,
            },
            max_gaze: Coords {
                x: u64::MIN,
                y: u64::MIN,
            },
        }
    }

    /// Enable SDL event.
    pub fn enable_event(&mut self, event: EventType) {
        self.event_pump.enable_event(event);
    }

    /// Disable SDL event.
    pub fn disable_event(&mut self, event: EventType) {
        self.event_pump.disable_event(event);
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
                    let p_x = d_x * self.bg.width as f32 / self.disp.width as f32;
                    let p_y = d_y * self.bg.height as f32 / self.disp.height as f32;

                    let gaze = GazeSample {
                        time: Instant::now(),
                        seqno: self.seqno,
                        d_width: self.disp.width,
                        d_height: self.disp.height,
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
                        self.gaze_samples.pop_front();
                        self.triggered = true;
                        self.seqno += 1;
                        return curr_gaze_sample;
                    }
                    self.gaze_samples.push_back(gaze);
                    self.gaze_samples.pop_front();
                }
            }
        }
    }

    // Take a gaze position in display pixels and take it to video coordinates.
    //
    // This assumes that the client is displaying the video at source resolution (no scaling) in the
    // center of the screen with black bars elsewhere. It is essentially "clipping" the gaze
    // coordinates to only care about when the viewer is looking at the video itself.
    fn to_video_coords(&self, d_x: u32, d_y: u32) -> (u32, u32) {
        let clip_rect = self.canvas.clip_rect().unwrap();

        let p_x = {
            let mut tmp = d_x as i32 - clip_rect.x();
            tmp = cmp::min(tmp, clip_rect.width() as i32);
            tmp = cmp::max(tmp, 0);
            tmp
        };
        let p_y = {
            let mut tmp = d_y as i32 - clip_rect.y();
            tmp = cmp::min(tmp, clip_rect.height() as i32);
            tmp = cmp::max(tmp, 0);
            tmp
        };

        (p_x as u32, p_y as u32)
    }

    /// Return the latest KeyUp event, if one is available.
    pub fn keyboard_event(&mut self) -> Option<Keycode> {
        // If there are keyboard events, grab them
        for event in self.event_pump.poll_iter() {
            match event {
                Event::KeyUp {
                    keycode: Some(k), ..
                } => return Some(k),
                Event::KeyDown {
                    keycode: Some(k), ..
                } => return Some(k),
                _ => continue,
            }
        }
        None
    }

    /// Get the latest gaze sample, if one is available.
    pub fn gaze_sample(&mut self) -> GazeSample {
        let mut gaze = match self.gaze_source {
            GazeSource::Mouse => {
                // Grab mouse position using SDL2.
                if self.event_pump.poll_iter().last().is_some() {
                    let d_x = self.event_pump.mouse_state().x() as u32;
                    let d_y = self.event_pump.mouse_state().y() as u32;

                    let (p_x, p_y) = self.to_video_coords(d_x, d_y);

                    GazeSample {
                        time: Instant::now(),
                        seqno: self.seqno,
                        d_width: self.disp.width,
                        d_height: self.disp.height,
                        d_x,
                        d_y,
                        p_x,
                        p_y,
                        m_x: p_x / 16,
                        m_y: p_y / 16,
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
                        let (p_x, p_y) =
                            self.to_video_coords(d_x.round() as u32, d_y.round() as u32);

                        GazeSample {
                            time: Instant::now(),
                            seqno: self.seqno,
                            d_width: self.disp.width,
                            d_height: self.disp.height,
                            d_x: d_x.round() as u32,
                            d_y: d_y.round() as u32,
                            p_x,
                            p_y,
                            m_x: p_x / 16,
                            m_y: p_y / 16,
                        }
                    } else {
                        *self.gaze_samples.back().unwrap()
                    }
                } else {
                    *self.gaze_samples.back().unwrap()
                }
            }
        };
        gaze.time = Instant::now();
        gaze.seqno = self.seqno;
        self.seqno += 1;
        self.gaze_samples.push_back(gaze);

        // Allow artificial delay to determine when to release the next gaze sample
        if let Some(oldest_gaze) = match self.delay {
            Some(delay) => {
                if self.gaze_samples.front().unwrap().time.elapsed() >= delay {
                    self.gaze_samples.pop_front()
                } else {
                    None
                }
            }
            None => self.gaze_samples.pop_front(),
        } {
            // Update gaze stats
            self.min_gaze.x = cmp::min(self.min_gaze.x, u64::from(oldest_gaze.p_x));
            self.min_gaze.y = cmp::min(self.min_gaze.y, u64::from(oldest_gaze.p_y));
            self.max_gaze.x = cmp::max(self.max_gaze.x, u64::from(oldest_gaze.p_x));
            self.max_gaze.y = cmp::max(self.max_gaze.y, u64::from(oldest_gaze.p_y));

            self.total_gaze.x +=
                (self.last_gaze.x as i64 - i64::from(oldest_gaze.p_x)).saturating_abs() as u64;
            self.total_gaze.y +=
                (self.last_gaze.y as i64 - i64::from(oldest_gaze.p_y)).saturating_abs() as u64;

            self.last_gaze.x = u64::from(oldest_gaze.p_x);
            self.last_gaze.y = u64::from(oldest_gaze.p_y);
        }

        *self.gaze_samples.front().unwrap()
    }

    /// Utility function for immediately drawing a white square to the bottom
    /// left corner of the display. Useful for debugging timing.
    ///
    /// In particular, this is intended to be used with a photodiode like the
    /// one in <https://github.com/lukehsiao/eyelink-latency>.
    pub fn display_white(&mut self, dim: u32) {
        // YUV data for white
        let y = vec![235; dim as usize * dim as usize];
        let u = vec![128; dim as usize / 2 * dim as usize / 2];
        let v = vec![128; dim as usize / 2 * dim as usize / 2];

        let mut texture = self
            .texture_creator
            .create_texture_streaming(PixelFormatEnum::YV12, dim, dim)
            .unwrap();

        let mut rect = Rect::new(0, 0, dim, dim);
        let _ = texture.update_yuv(
            rect,
            y.as_slice(),
            dim as usize,
            u.as_slice(),
            dim as usize / 2,
            v.as_slice(),
            dim as usize / 2,
        );

        rect = Rect::new(0, (self.disp.height - dim).try_into().unwrap(), dim, dim);

        self.canvas.clear();
        let _ = self.canvas.copy(&texture, None, rect);
        self.canvas.present();

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
            .create_texture_streaming(PixelFormatEnum::YV12, self.bg.width, self.bg.height)
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

    fn display_twostream_frame(
        &mut self,
        fg_nal: Option<&(NalData, GazeSample)>,
        bg_nal: Option<&NalData>,
    ) {
        let time = Instant::now();

        // Quick return if no new data
        if let (None, None) = (fg_nal, bg_nal) {
            // FPS looks like it drops a lot, but in reality, we hit this fast path a LOT.
            // self.frame_idx += 1;
            return;
        }

        // Otherwise, we need to draw
        let mut fg_texture = self
            .texture_creator
            .create_texture_streaming(PixelFormatEnum::ABGR8888, self.fg.width, self.fg.height)
            .unwrap();
        fg_texture.set_blend_mode(BlendMode::Blend);

        if self.triggered {
            info!("    init texture: {:#?}", time.elapsed());
        } else {
            debug!("    init texture: {:#?}", time.elapsed());
        }

        let dec_time = Instant::now();
        // If there is a new bg frame, update it
        if let Some(bg) = bg_nal {
            let bg_packet = Packet::copy(bg.as_bytes());
            self.total_bytes += bg_packet.size() as u64;
            self.bg_bytes += bg_packet.size() as u64;
            match self.bg_decoder.decode(&bg_packet, &mut self.bg_frame) {
                Ok(true) => {
                    let mut filtered = Video::empty();
                    // Apply sharpening filter
                    self.filter
                        .get("in")
                        .unwrap()
                        .source()
                        .add(&self.bg_frame)
                        .unwrap();
                    match self.filter.get("out").unwrap().sink().frame(&mut filtered) {
                        Ok(_) => {
                            self.bg_frame = filtered;
                        }
                        Err(e) => {
                            error!("{}", e);
                            unimplemented!()
                        }
                    }
                }
                Ok(false) => unimplemented!(),
                Err(_) => {
                    error!("Error occured while decoding packet.");
                }
            }
        }

        // If there is a new fg frame, update it
        let mut c_y = 0;
        let mut c_x = 0;
        let bg_rect = Rect::new(0, 0, self.bg_frame.width(), self.bg_frame.height());
        if let Some((fg, gaze)) = fg_nal {
            let fg_packet = Packet::copy(fg.as_bytes());
            self.total_bytes += fg_packet.size() as u64;
            self.fg_bytes += fg_packet.size() as u64;
            match self.fg_decoder.decode(&fg_packet, &mut self.fg_frame) {
                Ok(true) => {
                    if self.triggered {
                        info!("    decode nal: {:?}", dec_time.elapsed());
                    } else {
                        debug!("    decode nal: {:?}", dec_time.elapsed());
                    }
                }
                Ok(false) => unimplemented!(),
                Err(_) => {
                    error!("Error occured while decoding packet.");
                }
            }
            c_y = gaze.p_y as i32 + bg_rect.y();
            c_x = gaze.p_x as i32 + bg_rect.x();
        }

        // Redraw
        let mut fg_frame_rgba = Video::empty();

        let time = Instant::now();
        let mut fg_rect = Rect::new(0, 0, self.fg_frame.width(), self.fg_frame.height());

        let mut converter = self.fg_frame.converter(Pixel::RGBA).unwrap();
        converter.run(&self.fg_frame, &mut fg_frame_rgba).unwrap();

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
            .create_texture_streaming(PixelFormatEnum::YV12, self.bg.width, self.bg.height)
            .unwrap();

        let _ = bg_texture.update_yuv(
            bg_rect,
            self.bg_frame.data(0),
            self.bg_frame.stride(0),
            self.bg_frame.data(1),
            self.bg_frame.stride(1),
            self.bg_frame.data(2),
            self.bg_frame.stride(2),
        );

        // position the fg square correctly on the canvas
        //
        // If there is an fg_nal, we assume there is an fg_gaze.
        let bg_rect = self.canvas.clip_rect().unwrap();

        fg_rect = Rect::from_center((c_x, c_y), fg_rect.width(), fg_rect.height());

        self.canvas.clear();
        // Stretches the bg_texture to fill the entire rendering target
        let _ = self.canvas.copy(&bg_texture, None, bg_rect);
        let _ = self.canvas.copy(&fg_texture, None, fg_rect);
        self.canvas.present();

        self.frame_idx += 1;
        if self.triggered {
            info!("    display new frame: {:?}", time.elapsed());
        } else {
            debug!("    display new frame: {:?}", time.elapsed());
        }
    }

    /// Decode and display the provided frame.
    pub fn display_frame<'a, T, U>(&mut self, fg_nal: T, bg_nal: U)
    where
        T: Into<Option<&'a (NalData, GazeSample)>>,
        U: Into<Option<&'a NalData>>,
    {
        match self.alg {
            FoveationAlg::TwoStream => self.display_twostream_frame(fg_nal.into(), bg_nal.into()),
            _ => self.display_onestream_frame(bg_nal.into().unwrap()),
        }
    }

    pub fn total_frames(&self) -> u64 {
        self.frame_idx
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn fg_bytes(&self) -> u64 {
        self.fg_bytes
    }

    pub fn bg_bytes(&self) -> u64 {
        self.bg_bytes
    }

    pub fn total_gaze(&self) -> Coords {
        self.total_gaze
    }

    pub fn min_gaze(&self) -> Coords {
        self.min_gaze
    }

    pub fn max_gaze(&self) -> Coords {
        self.max_gaze
    }
}

/// Compute the 2D Gaussian of alpha values for blending.
///
/// These constants right now are just tuned to what seems to look OK to me. See commit msg for
/// details.
fn compute_alpha(fg_width: u32) -> Vec<u8> {
    let mut alpha_blend: Vec<u8> = vec![];
    for j in 0..fg_width {
        for i in 0..fg_width {
            alpha_blend.push(cmp::min(
                255,
                (896.0
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
    alpha_blend
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_init_client() {
        // Skip this test on Github Actions
        if let Ok(_) = env::var("CI") {
            return;
        }

        let _client = FvideoClient::new(
            FoveationAlg::TwoStream,
            10,
            Dims {
                width: 3840,
                height: 2160,
            },
            Dims {
                width: 512,
                height: 512 * 9 / 16,
            },
            DisplayOptions {
                delay: 0,
                filter: "smartblur=lr=1.0:ls=-1.0".to_string(),
            },
            GazeSource::Mouse,
            EyelinkOptions {
                calibrate: false,
                record: false,
            },
        );
    }
}
