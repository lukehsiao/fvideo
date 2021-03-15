//! Custom implemention of Eyelink Graphics Programming for calibration.
//!
//! Rather than deal with the buggy usage of the legacy SDL1.2 library provided
//! by SR Research, we build our own minimal implementation using SDL2.
//!
//! See: <http://download.sr-support.com/dispdoc/page12.html>

use core::ffi::c_void;
use std::io;
use std::num::{ParseFloatError, ParseIntError};

use libeyelink_sys::{self, HOOKFCNS2, INT16};
use log::error;
use sdl2::pixels::Color;
use sdl2::rect::{Point, Rect};
use sdl2::render::Canvas;
use sdl2::video::{DisplayMode, Window, WindowBuildError};
use sdl2::IntegerOrSdlError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GraphicsError {
    #[error("Cannot parse eye sample from: {self}")]
    UnrecognizedString(String),
    #[error(transparent)]
    ParseIntError(#[from] ParseIntError),
    #[error(transparent)]
    ParseFloatError(#[from] ParseFloatError),
    #[error(transparent)]
    IoError(#[from] io::Error),
    #[error("SDL2 Error: {self}")]
    SDL2Error(String),
    #[error(transparent)]
    SDL2WindowBuildError(#[from] WindowBuildError),
    #[error(transparent)]
    SDL2IntegerOrSdlError(#[from] IntegerOrSdlError),
}

#[no_mangle]
unsafe extern "C" fn clear_cal_display(user_data: *mut c_void) -> INT16 {
    match (user_data as *mut Canvas<Window>).as_mut() {
        Some(c) => {
            c.set_draw_color(Color::RGB(200, 200, 200));
            c.clear();
            c.present();
            0
        }
        None => 1,
    }
}

#[no_mangle]
unsafe extern "C" fn erase_cal_target(user_data: *mut c_void) -> INT16 {
    clear_cal_display(user_data)
}

#[no_mangle]
unsafe extern "C" fn draw_cal_target(user_data: *mut c_void, x: f32, y: f32) -> INT16 {
    // Calibration target size in px.
    const TARGET_SIZE: u32 = 24;
    const INNER_SIZE: u32 = 4;
    let canvas = match (user_data as *mut Canvas<Window>).as_mut() {
        Some(c) => c,
        None => return 1,
    };

    canvas.set_draw_color(Color::RGB(0, 0, 0));
    if let Err(e) = canvas.fill_rect(Rect::from_center(
        Point::new(x.round() as i32, y.round() as i32),
        TARGET_SIZE,
        TARGET_SIZE,
    )) {
        error!("Failed drawing rectangle: {}.", e);
        return 1;
    }

    canvas.set_draw_color(Color::RGB(200, 200, 200));
    match canvas.fill_rect(Rect::from_center(
        Point::new(x.round() as i32, y.round() as i32),
        INNER_SIZE,
        INNER_SIZE,
    )) {
        Ok(_) => {
            canvas.present();
            0
        }
        Err(e) => {
            error!("Failed drawing rectangle: {}.", e);
            1
        }
    }
}

#[no_mangle]
unsafe extern "C" fn setup_cal_display(user_data: *mut c_void) -> INT16 {
    // For custom targets, we could initialize them here and release in
    // exit_cal_display. For simplicity, just using rects so we don't need this.
    match (user_data as *mut Canvas<Window>).as_mut() {
        Some(c) => {
            c.set_draw_color(Color::RGB(200, 200, 200));
            c.clear();
            c.present();
            0
        }
        None => 1,
    }
}

#[no_mangle]
unsafe extern "C" fn exit_cal_display(user_data: *mut c_void) -> INT16 {
    match (user_data as *mut Canvas<Window>).as_mut() {
        Some(c) => {
            c.set_draw_color(Color::RGB(0, 0, 0));
            c.clear();
            0
        }
        None => 1,
    }
}

/// Initialize SDL2 and hooks for the Eyelink Core Library.
///
/// Returns a DisplayMode and a ptr to the SDL context that must be passed back into
/// close_expt_graphics() to clean up.
pub fn init_expt_graphics() -> Result<(DisplayMode, *mut Canvas<Window>), GraphicsError> {
    // Initialize SDL2
    let sdl_context = sdl2::init().map_err(GraphicsError::SDL2Error)?;
    let video = sdl_context.video().map_err(GraphicsError::SDL2Error)?;

    match video.num_video_displays() {
        Ok(n) if n == 1 => (),
        Ok(n) => {
            return Err(GraphicsError::SDL2Error(format!(
                "We currently only support 1 display. You have {}.",
                n
            )));
        }
        Err(e) => return Err(GraphicsError::SDL2Error(e)),
    }

    // Assumes a single display at index 0.
    const DISP_IDX: i32 = 0;
    let display_mode = video
        .desktop_display_mode(DISP_IDX)
        .map_err(GraphicsError::SDL2Error)?;

    let window = video
        .window("calibration", display_mode.w as u32, display_mode.h as u32)
        .fullscreen_desktop()
        .build()?;

    let canvas = Box::new(
        window
            .into_canvas()
            .accelerated()
            .target_texture()
            .build()?,
    );

    let canvas_ptr = Box::into_raw(canvas);

    // We don't need audio, so we leave the following None:
    //     - fcns.cal_target_beep_hook
    //     - fcns.cal_done_beep_hook
    //     - fcns.dc_done_beep_hook
    //     - fcns.dc_target_beep_hook
    //
    // We don't need camera setup features, so we leave the following None:
    //     - fcns.setup_image_display_hook
    //     - fcns.exit_image_display_hook
    //     - fcns.image_title_hook
    //     - fcns.draw_image_line_hook
    //     - fcns.set_image_palette_hook
    let fcns = Box::new(HOOKFCNS2 {
        major: 1,
        minor: 0,
        userData: canvas_ptr as *mut _,
        setup_cal_display_hook: Some(setup_cal_display),
        exit_cal_display_hook: Some(exit_cal_display),
        setup_image_display_hook: None,
        image_title_hook: None,
        draw_image: None,
        exit_image_display_hook: None,
        clear_cal_display_hook: Some(clear_cal_display),
        erase_cal_target_hook: Some(erase_cal_target),
        draw_cal_target_hook: Some(draw_cal_target),
        play_target_beep_hook: None,
        get_input_key_hook: None,
        alert_printf_hook: None,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        reserved4: 0,
    });

    // Register all the hooks with Eyelink Core
    unsafe {
        match libeyelink_sys::setup_graphic_hook_functions_V2(Box::into_raw(fcns)) {
            0 => (),
            n => return Err(GraphicsError::SDL2Error(format!("Graphic Hooks: {}", n))),
        }
    }
    Ok((display_mode, canvas_ptr))
}

/// Clean up all expt graphics.
///
/// # Safety
/// This function should be passed the canvas pointer returned by init_expt_graphics.
pub unsafe fn close_expt_graphics(canvas_ptr: *mut Canvas<Window>) -> Result<(), GraphicsError> {
    // Take ownership of the canvas to allow it to be cleaned up.
    if !canvas_ptr.is_null() {
        Box::from_raw(canvas_ptr);
        Ok(())
    } else {
        Err(GraphicsError::SDL2Error(
            "Received a NULL ptr for canvas.".to_string(),
        ))
    }
}
