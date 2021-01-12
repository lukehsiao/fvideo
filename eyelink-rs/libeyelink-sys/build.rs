fn main() {
    // Tell cargo to tell rustc to link the system eyelink shared libraries.
    println!("cargo:rustc-link-search=/usr/lib");
    println!("cargo:rustc-link-lib=eyelink_core_graphics");
    println!("cargo:rustc-link-lib=eyelink_core");

    if cfg!(feature = "sdl-graphics") {
        println!("cargo:rustc-link-lib=sdl_util");
        pkg_config::Config::new()
            .atleast_version("1.2.15")
            .probe("sdl")
            .unwrap();
        pkg_config::Config::new()
            .atleast_version("2.0.11")
            .probe("SDL_ttf")
            .unwrap();
        pkg_config::Config::new()
            .atleast_version("2.0.25")
            .probe("SDL_gfx")
            .unwrap();
        pkg_config::Config::new()
            .atleast_version("1.2.12")
            .probe("SDL_image")
            .unwrap();
        pkg_config::Config::new()
            .atleast_version("1.2.12")
            .probe("SDL_mixer")
            .unwrap();
    }

    println!("cargo:rerun-if-changed=build.rs");
}
