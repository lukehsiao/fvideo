extern crate ffmpeg_next as ffmpeg;

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use ffmpeg::util::frame::video::Video;
use ffmpeg::{codec, decoder, Packet};
use lazy_static::lazy_static;
use log::{error, info};
use regex::Regex;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use structopt::clap::{arg_enum, AppSettings};
use structopt::StructOpt;
use x264::{Encoder, NalData, Param, Picture};

arg_enum! {
    #[derive(Debug)]
    enum FoveationAlg {
        SquareStep,
        Gaussian
    }
}

fn parse_qo_max(src: &str) -> Result<f32> {
    let qo_max = f32::from_str(src)?;
    if qo_max < 0.0 || qo_max > 81.0 {
        Err(anyhow!("QO max offset not in valid range [0, 81]."))
    } else {
        Ok(qo_max)
    }
}

/// Parse the width, height, and frame rate from the Y4M header.
///
/// See https://wiki.multimedia.cx/index.php/YUV4MPEG2 for details.
fn parse_y4m_header(src: &str) -> Result<(usize, usize, f64)> {
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
        None => return Err(anyhow!("Invalid Y4M Header.")),
        Some(caps) => caps,
    };

    let width: usize = caps["width"].parse()?;
    let height: usize = caps["height"].parse()?;

    let fps = match &caps["frame"] {
        "30:1" => 30.0,
        "25:1" => 25.0,
        "24:1" => 24.0,
        "30000:1001" => 29.97,
        "24000:1001" => 23.976,
        _ => return Err(anyhow!("Invalid framerate.")),
    };

    Ok((width, height, fps))
}

#[derive(StructOpt, Debug)]
#[structopt(
    about("A tool for foveated encoding an input Y4M and decoding/displaying the results."),
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// The parameter for the size of the foveal region (0 = disable foveation).
    ///
    /// The meaning of this value depends on the Foveation Algorithm.
    #[structopt(short, long, default_value = "0")]
    fovea: i32,

    /// The parameter for the size of the foveal region.
    #[structopt(short, long, default_value = "Gaussian", possible_values = &FoveationAlg::variants(), case_insensitive=true)]
    alg: FoveationAlg,

    /// The maximum qp offset outside of the foveal region (only range 0 to 81 valid).
    #[structopt(short, long, default_value = "35.0", parse(try_from_str = parse_qo_max))]
    qo_max: f32,

    /// The video to encode and display.
    #[structopt(name = "VIDEO", parse(from_os_str))]
    video: PathBuf,
}

fn setup_x264_params(fovea: i32, width: usize, height: usize) -> Result<Param> {
    let mut par = match Param::default_preset("fast", "zerolatency") {
        Ok(p) => p,
        Err(s) => return Err(anyhow!("{}", s)),
    };

    // TODO(lukehsiao): this is hacky, and shoould probably be cleaned up.
    par = par.set_x264_defaults();
    par = par.set_dimension(width, height);
    par = par.set_fovea(fovea);
    par = par.set_min_keyint(i32::MAX);
    par = par.set_no_scenecut();

    Ok(par)
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let input = File::open(opt.video)?;
    let mut f = BufReader::new(input);
    let mut timestamp = 0;

    // First, read dimensions/FPS from Y4M header.
    let mut hdr = String::new();
    f.read_line(&mut hdr).unwrap();
    let (width, height, fps) = parse_y4m_header(&hdr)?;

    let mut par = setup_x264_params(opt.fovea, width, height)?;
    let mut pic = Picture::from_param(&par).unwrap();
    let mut enc = Encoder::open(&mut par).unwrap();

    let mut decoder = decoder::new()
        .open_as(decoder::find(codec::Id::H264))
        .unwrap()
        .video()
        .unwrap();

    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    let mut event_pump = sdl_context.event_pump().unwrap();

    let window = video_subsystem
        .window("fvideo.rs", 0, 0)
        .fullscreen_desktop()
        // .position_centered()
        .build()
        .unwrap();

    let mut canvas = window
        .into_canvas()
        .accelerated()
        // .present_vsync()
        .target_texture()
        .build()
        .unwrap();

    let (disp_x, disp_y) = {
        let disp_rect = video_subsystem.display_bounds(0).unwrap();
        (disp_rect.w, disp_rect.h)
    };

    // TODO(lukehsiao): Not sure if we should use swap 1 or 0.
    video_subsystem.gl_set_swap_interval(0).unwrap();

    let texture_creator = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(PixelFormatEnum::YV12, width as u32, height as u32)
        .unwrap();

    let mut frame_index = 0;
    let mut frame = Video::empty();
    let mut process_nal_unit = |nal: &NalData| {
        let packet = Packet::copy(nal.as_bytes());
        match decoder.decode(&packet, &mut frame) {
            Ok(true) => {
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

                canvas.clear();
                let _ = canvas.copy(&texture, None, None);
                canvas.present();

                // dump_frame(&rgb_frame, frame_index).unwrap();
                frame_index += 1;
            }
            Ok(false) => (),
            Err(_) => {
                error!("Error occured while decoding packet.");
            }
        }
    };

    // The frame dimensions in terms of macroblocks
    let mb_x = width / 16;
    let mb_y = height / 16;
    let mut m_x = mb_x / 2;
    let mut m_y = mb_y / 2;

    let now = Instant::now();

    let mut inner_loop_time: f32 = 0.0;

    // Assumes input video is 24 FPS. Could read this from the Y4M Header.
    let frame_dur = Duration::from_secs_f64(1.0 / fps);

    'out: loop {
        let frame_time = Instant::now();
        // Skip header data of the frame
        if f.read_line(&mut hdr).is_err() {
            break 'out;
        }

        // Read the input YUV frame
        for plane in 0..3 {
            let mut buf = pic.as_mut_slice(plane).unwrap();
            if f.read_exact(&mut buf).is_err() {
                break 'out;
            }
        }

        let buffer_time = Duration::from_secs_f32(inner_loop_time);

        // Re-run using this same input frame until the time to display it has
        // passed.
        while frame_time.elapsed() < (frame_dur - buffer_time) {
            let inner_time = Instant::now();

            // TODO(lukehsiao): 5x5px white square where mouse cursor is.
            // Note that 235 = white for luma
            // Also note that trying to iterate over the whole image here was too slow.
            // const SQ_WIDTH: usize = 4;
            // let luma = pic.as_mut_slice(0).unwrap();
            // for x in 0..SQ_WIDTH {
            //     for y in 0..SQ_WIDTH {
            //         luma[cmp::min(WIDTH, (WIDTH * (p_y + y)) + (p_x + x))] = 0xEB;
            //     }
            // }

            if opt.fovea > 0 {
                // Get current mouse position
                match event_pump.poll_iter().last() {
                    Some(_) => {
                        let mut p_x = event_pump.mouse_state().x() as usize;
                        let mut p_y = event_pump.mouse_state().y() as usize;

                        // Scale from display to video resolution
                        p_x = (p_x as f64 * (width as f64 / disp_x as f64)) as usize;
                        p_y = (p_y as f64 * (height as f64 / disp_y as f64)) as usize;

                        m_x = p_x / 16;
                        m_y = p_y / 16;
                    }
                    None => (),
                }

                let mut qp_offsets = vec![0.0; mb_x * mb_y];

                // Calculate Offsets based on Foveation Alg
                match opt.alg {
                    FoveationAlg::Gaussian => {
                        for j in 0..mb_y {
                            for i in 0..mb_x {
                                // Below is the 2d gaussian used by Illahi et al.
                                qp_offsets[(mb_x * j) + i] = opt.qo_max
                                    - (opt.qo_max
                                        * (-1.0
                                            * (((i as i32 - m_x as i32).pow(2)
                                                + (j as i32 - m_y as i32).pow(2))
                                                as f32
                                                / (2.0
                                                    * (mb_x as f32 / opt.fovea as f32).powi(2))))
                                        .exp());
                            }
                        }
                    }
                    FoveationAlg::SquareStep => {
                        for j in 0..mb_y {
                            for i in 0..mb_x {
                                // Keeps (2(dim) - 1)^2 macroblocks in HQ
                                qp_offsets[(mb_x * j) + i] = if (m_x as i32 - i as i32).abs()
                                    < opt.fovea
                                    && (m_y as i32 - j as i32).abs() < opt.fovea
                                {
                                    0.0
                                } else {
                                    opt.qo_max
                                };
                            }
                        }
                    }
                }

                // Calculate offsets
                pic.pic.prop.quant_offsets = qp_offsets.as_mut_ptr();

                // EWMA as a heuristic of how long this takes to try and stay
                // more on time.
                inner_loop_time =
                    (0.99 * inner_time.elapsed().as_secs_f32()) + (0.01 * inner_loop_time);
            }

            pic = pic.set_timestamp(timestamp);

            timestamp += 1;
            if let Some((nal, _, _)) = enc.encode(&pic).unwrap() {
                process_nal_unit(&nal);
            }
        }
    }

    while enc.delayed_frames() {
        if let Some((nal, _, _)) = enc.encode(None).unwrap() {
            process_nal_unit(&nal);
        }
    }

    let elapsed = now.elapsed();
    info!(
        "FPS: {}/{} = {}",
        frame_index,
        elapsed.as_secs_f64(),
        frame_index as f64 / elapsed.as_secs_f64()
    );

    decoder.flush();

    Ok(())
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
