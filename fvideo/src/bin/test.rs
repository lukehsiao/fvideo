extern crate ffmpeg_next as ffmpeg;

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use ffmpeg::util::frame::video::Video;
use ffmpeg::{codec, decoder, Packet};
use log::{error, info};
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use x264::{Encoder, NalData, Param, Picture};

#[derive(StructOpt, Debug)]
#[structopt(
    about,
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// The parameter for the size of the foveal region.
    #[structopt(short, long, default_value = "0")]
    fovea: i32,

    /// The video to encode and play.
    #[structopt(name = "VIDEO", parse(from_os_str))]
    video: PathBuf,
}

const WIDTH: usize = 3840;
const HEIGHT: usize = 2160;
// The maximum quantization offset. QP can range from 0-81.
const QO_MAX: f32 = 35.0;

fn setup_x264_params(opt: &Opt) -> Result<Param> {
    let mut par = match Param::default_preset("fast", "zerolatency") {
        Ok(p) => p,
        Err(s) => return Err(anyhow!("{}", s)),
    };

    // TODO(lukehsiao): this is hacky, and shoould probably be cleaned up.
    par = par.set_x264_defaults();
    par = par.set_dimension(WIDTH, HEIGHT);
    par = par.set_fovea(opt.fovea);
    par = par.set_min_keyint(50000);
    par = par.set_no_scenecut();

    Ok(par)
}

fn main() -> Result<()> {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();
    let opt = Opt::from_args();

    let mut par = setup_x264_params(&opt)?;
    let mut pic = Picture::from_param(&par).unwrap();
    let mut enc = Encoder::open(&mut par).unwrap();
    let input = File::open(opt.video)?;
    let mut f = BufReader::new(input);
    let mut timestamp = 0;

    let mut decoder = decoder::new()
        .open_as(decoder::find(codec::Id::H264))
        .unwrap()
        .video()
        .unwrap();

    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    let mut event_pump = sdl_context.event_pump().unwrap();

    let window = video_subsystem
        .window("test.rs", WIDTH as u32, HEIGHT as u32)
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
        .create_texture_streaming(PixelFormatEnum::YV12, WIDTH as u32, HEIGHT as u32)
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
    let mb_x = WIDTH / 16;
    let mb_y = HEIGHT / 16;
    let mut m_x = mb_x / 2;
    let mut m_y = mb_y / 2;

    let frame_dur = Duration::from_secs_f64(1.0 / 24.0);

    let now = Instant::now();

    let mut inner_loop_time: f32 = 0.0;

    // First, skip header data of the Y4M
    let mut hdr = vec![];
    f.read_until(0x0A, &mut hdr).unwrap();
    'out: loop {
        let frame_time = Instant::now();
        // Skip header data of the frame
        if f.read_until(0x0A, &mut hdr).is_err() {
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

            // Get current mouse position
            match event_pump.poll_iter().last() {
                Some(_) => {
                    let mut p_x = event_pump.mouse_state().x() as usize;
                    let mut p_y = event_pump.mouse_state().y() as usize;

                    // Scale from display to video resolution
                    p_x = (p_x as f64 * (WIDTH as f64 / disp_x as f64)) as usize;
                    p_y = (p_y as f64 * (HEIGHT as f64 / disp_y as f64)) as usize;

                    m_x = p_x / 16;
                    m_y = p_y / 16;
                }
                None => (),
            }

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
                let mut qp_offsets = vec![0.0; mb_x * mb_y];
                // pic.prop.quant_offsets = (float*)malloc( sizeof( float ) * mb_x * mb_y );

                // Calculate offsets
                //
                // We just use a step function outside `dim` macroblocks
                for j in 0..mb_y {
                    for i in 0..mb_x {
                        // Keeps (2(dim) - 1)^2 macroblocks in HQ
                        // qp_offsets[(mb_x * j) + i] = if (m_x as i32 - i as i32).abs() < opt.fovea
                        //     && (m_y as i32 - j as i32).abs() < opt.fovea
                        // {
                        //     0.0
                        // } else {
                        //     QO_MAX as f32
                        // };

                        // Below is the 2d gaussian used by Illahi et al.
                        qp_offsets[(mb_x * j) + i] = QO_MAX
                            - (QO_MAX
                                * (-1.0
                                    * (((i as i32 - m_x as i32).pow(2)
                                        + (j as i32 - m_y as i32).pow(2))
                                        as f32
                                        / (2.0 * (mb_x as f32 / opt.fovea as f32).powi(2))))
                                .exp());
                    }
                }
                pic.pic.prop.quant_offsets = qp_offsets.as_mut_ptr();

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
