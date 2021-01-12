# fvideo

[![GitHub Workflow Status](https://img.shields.io/github/workflow/status/lukehsiao/fvideo/rust)](https://github.com/lukehsiao/fvideo/actions)

:construction: This is a work in progress.

Low-latency foveated video encoding.

## Installation

### Dependencies

`fvideo` has only been tested on Ubuntu 18.04/20.04. First, you must have the
following system dependencies:

```
sudo apt install libx264-dev ffmpeg libavutil-dev libavformat-dev libavfilter-dev libavdevice-dev llvm-dev libudev-dev
```

Next, you must install Eyelink's libraries.

```
wget -O - "http://download.sr-support.com/software/dists/SRResearch/SRResearch_key" | sudo apt-key add -
sudo add-apt-repository "deb http://download.sr-support.com/software SRResearch main"
sudo apt update
sudo apt install eyelink-display-software
```

Then, you can use [cargo] to build the binaries:

```
cargo build --release
```

## Usage

[cargo]: https://doc.rust-lang.org/cargo/getting-started/installation.html
