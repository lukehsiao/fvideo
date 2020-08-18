extern crate ffmpeg_next as ffmpeg;

use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::time::Instant;

use ffmpeg::{codec, decoder, format, frame, software, Packet};
use log::info;
use regex::Regex;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use x264::{Encoder, NalData, Param, Picture};

fn main() {
    pretty_env_logger::init();
    ffmpeg::init().unwrap();

    let args: Vec<String> = env::args().collect();
    let re = Regex::new(r"(\d+)x(\d+)").unwrap();
    if args.len() < 2 {
        panic!("Missing argument:\nUsage:\n{} 640x480 in.yuv\n", args[0]);
    }
    let caps = re.captures(args[1].as_str()).unwrap();
    let w: usize = caps[1].parse().unwrap();
    let h: usize = caps[2].parse().unwrap();

    let mut par = Param::default_preset("medium", None).unwrap();

    par = par.set_dimension(h, w);
    par = par.param_parse("repeat_headers", "1").unwrap();
    par = par.param_parse("annexb", "1").unwrap();
    par = par.apply_profile("high").unwrap();

    let mut pic = Picture::from_param(&par).unwrap();

    let mut enc = Encoder::open(&mut par).unwrap();
    let mut input = File::open(args[2].as_str()).unwrap();
    let mut timestamp = 0;

    let mut decoder = decoder::new()
        .open_as(decoder::find(codec::Id::H264))
        .unwrap()
        .video()
        .unwrap();

    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();

    let window = video_subsystem
        .window("player.rs", 1920, 1080)
        .position_centered()
        .build()
        .unwrap();

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .present_vsync()
        .target_texture()
        .build()
        .unwrap();

    video_subsystem.gl_set_swap_interval(1).unwrap();

    let texture_creator = canvas.texture_creator();

    let mut scaler: Option<software::scaling::Context> = None;
    let mut frame_index = 0;
    let mut process_nal_unit = |nal: &NalData| {
        let packet = Packet::copy(nal.as_bytes());
        decoder.send_packet(&packet).unwrap();
        let mut frame = frame::Video::empty();
        let mut out_frame = frame::Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            let mut texture = texture_creator
                .create_texture_streaming(PixelFormatEnum::YV12, decoder.width(), decoder.height())
                .unwrap();
            // Do something with the frame.
            //
            // As an example, we dump the frame in ppm format. Note that scaler
            // initialization has to be deferred due to decoder.format() etc.
            // not available before packets being sent.
            if scaler.is_none() {
                scaler = Some(
                    software::converter(
                        (decoder.width(), decoder.height()),
                        decoder.format(),
                        format::Pixel::YUV420P,
                    )
                    .unwrap(),
                );
            }
            scaler
                .as_mut()
                .map(|s| s.run(&frame, &mut out_frame).unwrap());

            let rect = Rect::new(0, 0, out_frame.width(), out_frame.height());
            let _ = texture.update_yuv(
                rect,
                out_frame.data(0),
                out_frame.stride(0),
                out_frame.data(1),
                out_frame.stride(1),
                out_frame.data(2),
                out_frame.stride(2),
            );

            canvas.clear();
            let _ = canvas.copy(&texture, None, None);
            canvas.present();

            // dump_frame(&rgb_frame, frame_index).unwrap();
            frame_index += 1;
        }
    };

    let now = Instant::now();
    'out: loop {
        // TODO read by line, the stride could be different from width
        for plane in 0..3 {
            let mut buf = pic.as_mut_slice(plane).unwrap();
            if input.read_exact(&mut buf).is_err() {
                break 'out;
            }
        }

        pic = pic.set_timestamp(timestamp);
        timestamp += 1;
        if let Some((nal, _, _)) = enc.encode(&pic).unwrap() {
            process_nal_unit(&nal);
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

    // You also need to flush the decoder in the end, which is omitted here.
}

fn _dump_frame(frame: &frame::Video, index: usize) -> io::Result<()> {
    let path = format!("/tmp/frame{}.ppm", index);
    let mut file = File::create(path.to_owned())?;
    file.write_all(format!("P6\n{} {}\n255\n", frame.width(), frame.height()).as_bytes())?;
    file.write_all(frame.data(0))?;
    eprintln!("frame {} written to {}", index, path);
    Ok(())
}
