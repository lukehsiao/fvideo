# libeyelink-sys

The `libeyelink-sys` crate provides declarations and linkage for the
`libeyelink_core` and `libeyelink_core_graphics` C libraries. Following the
`*-sys` package conventions, the `libeyelink-sys` crate does not define
higher-level or safe abstractions over the native library functions.

The bindings were generated automatically with [bindgen]:
```
bindgen /usr/include/core_expt.h -o src/base.rs --with-derive-default
```

If the `sdl-graphics` feature is enabled, the bindings are generated from:
```
bindgen /usr/include/sdl_expt.h -o src/sdl-graphics.rs --blacklist-function '^str.*' --blacklist-function '.*cvt.*' --with-derive-default
```

## Dependencies
You must have the Linux Eyelink SDK installed from SR Research. Steps to
install:

  1. Add signing key
     ```
     $ wget -O - "http://download.sr-support.com/software/dists/SRResearch/SRResearch_key" | sudo apt-key add -
     ```
  2. Add apt repository
     ```
     $ sudo add-apt-repository "deb http://download.sr-support.com/software SRResearch main"
     $ sudo apt-get update
     ```
  3. Install latest release of EyeLink Developers Kit for Linux
     ```
     $ sudo apt-get install eyelink-display-software
     ```
     Alternatively, a tar of DEBs is available [at this link][debs].

This crate has only been tested on Ubuntu 18.04.

## Usage
Add `libeyelink-sys` as a dependency in `Cargo.toml`:
```
[dependencies]
libeyelink-sys = "0.1"
```

## API Documentation
The best source for help on the API is the native documentation:
* [API Documentation][api]

[api]: http://download.sr-support.com/dispdoc/index.html
[debs]: http://download.sr-support.com/linuxDisplaySoftwareRelease/eyelink-display-software_1.11_x64_debs.tar.gz
[bindgen]: https://github.com/rust-lang/rust-bindgen
