#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(clippy::upper_case_acronyms)]

#[cfg(not(feature = "sdl-graphics"))]
include!("./base.rs");

#[cfg(feature = "sdl-graphics")]
include!("./sdl-graphics.rs");
