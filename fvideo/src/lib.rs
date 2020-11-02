#![forbid(unsafe_code)]
// #![forbid(warnings)]

use std::time::Instant;

pub mod client;
pub mod dummyserver;
pub mod server;

/// Structure of a single gaze sample.
#[derive(Copy, Clone, Debug)]
pub struct GazeSample {
    pub time: Instant, // time of the sample
    pub p_x: u32,      // x position in px
    pub p_y: u32,      // y position in px
    pub m_x: u32,      // x position in macroblock
    pub m_y: u32,      // y position in macroblock
}
