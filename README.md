# fvideo

[![GitHub Workflow Status](https://img.shields.io/github/actions/workflow/status/lukehsiao/fvideo/rust.yml)](https://github.com/lukehsiao/fvideo/actions/workflows/rust.yml)
![Maintenance](https://img.shields.io/maintenance/no/2021)

Low-latency foveated video encoding prototype.

## Installation

### Dependencies

`fvideo` has only been tested on Ubuntu 18.04/20.04. First, you must have the
following system dependencies:

```
sudo apt install libx264-dev ffmpeg libavutil-dev libavformat-dev libavfilter-dev libavdevice-dev llvm-dev libudev-dev mpv
```

Note that `fvideo` requires FFmpeg 4.3.x, so if running on Ubuntu 18.04, you'll need to install it
yourself or use the unofficial PPA:

```
sudo add-apt-repository ppa:jonathonf/ffmpeg-4
```

Next, you must install Eyelink's libraries.

```
sudo add-apt-repository universe
sudo apt update
sudo apt install ca-certificates
sudo apt-key adv --fetch-keys https://apt.sr-research.com/SRResearch_key
sudo add-apt-repository 'deb [arch=amd64] https://apt.sr-research.com SRResearch main'
sudo apt update
sudo apt install eyelink-display-software
```

Then, you can use [cargo] to build the binaries:

```
cargo build --release
```

## Usage

### User Study

Note that the user study binary currently expect videos to be in specific directories with specific
names. This will be changed in the future.

```
user_study 0.1.0
The user study experiment interface.

USAGE:
    user_study [OPTIONS] <SOURCE> --name <name>

FLAGS:
    -h, --help
            Prints help information

    -V, --version
            Prints version information


OPTIONS:
    -n, --name <name>
            The full name of the participant

    -o, --output <output>
            Where to save the foveated h264 bitstream and tracefile.

            No output is saved unless this is specified.

ARGS:
    <SOURCE>
            Source for gaze data [possible values: PierSeaside, Barscene, SquareTimelapse,
            Rollercoaster, ToddlerFountain]
```

Once in the user study, the interface is:

| key        | action                                |
| ---------- | ------------------------------------- |
| Esc/Ctrl+C | quit                                  |
| 0-9        | video qualities 0-9                   |
| p          | pause                                 |
| c          | calibrate                             |
| b          | play baseline                         |
| r          | resume (video qual 0)                 |
| Enter      | accept current video quality          |
| n          | none of the qualities are good enough |

All data is logged to `data/user_study.csv`.

[cargo]: https://doc.rust-lang.org/cargo/getting-started/installation.html
