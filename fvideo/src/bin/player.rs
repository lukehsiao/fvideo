//! A placeholder binary for testing x264 integration a custom video player in
//! Rust. The input must be a Y4M, which will be processed by x264.
//!
//! # Usage
//! ```
//! $ cargo run --release --bin=player -- video.y4m
//! ```
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use anyhow::{anyhow, Result};
use ffmpeg_next::format;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::scaling::flag::Flags;
use ffmpeg_next::util::frame::video::Video;
use log::{error, info};
use num_rational::Rational64;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use structopt::clap::AppSettings;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(
    about,
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::ColorAuto)
)]
struct Opt {
    /// The input to encode and display
    #[structopt(name = "VIDEO", parse(from_os_str))]
    video: PathBuf,
}

fn play_video(opt: &Opt) -> Result<()> {
    // Use ffmpeg to decode
    ffmpeg_next::init()?;

    let mut ictx = format::input(&opt.video)?;
    let in_stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| ffmpeg_next::Error::StreamNotFound)?;

    let mut video_decoder = in_stream.codec().decoder().video()?;

    info!(
        "W: {}, H: {}",
        video_decoder.width(),
        video_decoder.height()
    );

    let mut context = video_decoder.scaler(
        video_decoder.width(),
        video_decoder.height(),
        Flags::BILINEAR,
    )?;

    let sdl_context = sdl2::init().map_err(|e| anyhow!(e))?;
    let video_subsystem = sdl_context.video().map_err(|e| anyhow!(e))?;

    let window = video_subsystem
        .window("player.rs", 960, 540)
        .position_centered()
        .build()?;

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .target_texture()
        .build()?;

    video_subsystem
        .gl_set_swap_interval(0)
        .map_err(|e| anyhow!("Failed setting swap: {}", e))?;

    let texture_creator = canvas.texture_creator();
    let mut texture = texture_creator.create_texture_streaming(
        PixelFormatEnum::YV12,
        video_decoder.width(),
        video_decoder.height(),
    )?;

    // let mut prev_pts = None;
    let mut now = Instant::now();
    let mut total_frames = 0;
    for (i, (stream, packet)) in ictx.packets().enumerate() {
        match stream.codec().codec() {
            Some(codec) if codec.is_video() => {
                let mut frame = Video::empty();
                match video_decoder.decode(&packet, &mut frame) {
                    // If the frame is finished
                    Ok(true) => {
                        let mut yuv_frame = Video::empty();
                        context.run(&frame, &mut yuv_frame)?;

                        let rect = Rect::new(0, 0, yuv_frame.width(), yuv_frame.height());
                        // info!("rendering frame {}", i);
                        let _ = texture.update_yuv(
                            rect,
                            yuv_frame.data(0),
                            yuv_frame.stride(0),
                            yuv_frame.data(1),
                            yuv_frame.stride(1),
                            yuv_frame.data(2),
                            yuv_frame.stride(2),
                        );

                        canvas.clear();
                        let _ = canvas.copy(&texture, None, None); //copy texture to our canvas
                        canvas.present();

                        total_frames += 1;

                        // let pts = (Rational64::from(packet.pts().unwrap() * 1_000_000_000)
                        //     * Rational64::new(
                        //         stream.time_base().numerator() as i64,
                        //         stream.time_base().denominator() as i64,
                        //     ))
                        // .to_integer();
                        // // TODO(lukehsiao): This sleep seems wrong, it is too
                        // // slow and causes the video to look like it's playing
                        // // in slow motion.
                        // if let Some(prev) = prev_pts {
                        //     let elapsed = now.elapsed();
                        //     if pts > prev {
                        //         let sleep = time::Duration::new(0, (pts - prev) as u32);
                        //         if elapsed < sleep {
                        //             info!("Sleep for {} - {:?}", pts - prev, sleep - elapsed);
                        //             // std::thread::sleep(sleep - elapsed);
                        //         }
                        //     }
                        // }
                        //
                        // now = time::Instant::now();
                        // prev_pts = Some(pts);
                    }
                    Ok(false) => (),
                    Err(_) => {
                        error!("Error occurred while decoding packet.");
                    }
                }
            }
            _ => {}
        }
    }
    let elapsed = now.elapsed();

    info!(
        "FPS: {}/{} = {}",
        total_frames,
        elapsed.as_secs_f64(),
        total_frames as f64 / elapsed.as_secs_f64()
    );

    Ok(())
}

fn main() {
    pretty_env_logger::init();
    let opt = Opt::from_args();

    // Play the video clip
    if let Err(e) = play_video(&opt) {
        error!("Failed playing video: {}", e);
        process::exit(1);
    }
}
