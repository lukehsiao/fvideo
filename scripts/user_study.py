#!/usr/bin/env python
import argparse
import logging
import sys
from subprocess import DEVNULL, run

import matplotlib
import matplotlib.pyplot as plt
import pandas as pd
import seaborn as sns
import numpy as np
from matplotlib.backends.backend_pdf import PdfPages

sns.set(style="whitegrid")
sns.set_context("paper", font_scale=1.5, rc={"lines.linewidth": 2.25})


# Configure logging
logging.basicConfig(
    stream=sys.stdout,
    format="[%(asctime)s][%(levelname)s] %(name)s:%(lineno)s - %(message)s",
    level=logging.INFO,
)
logger = logging.getLogger(__name__)

FIGSIZE = (7, 4)


def _plot(infile):
    """Plotting logic."""
    fig, ax = plt.subplots(figsize=FIGSIZE)

    # type,b,f1,precision,recall
    data = pd.read_csv(infile, skipinitialspace=True)

    data["e2e_delay"] = data["delay_ms"] + 14

    baselines = {"barscene": 67396533, "square_timelapse": 33822082}

    estimates = {
        "barscene": {
            0: {
                9: 4.2,
                8: 5.9,
                7: 7.2,
                6: 8.0,
                5: 8.6,
                4: 9.7,
                3: 10.7,
                2: 11.8,
                1: 13.8,
                0: 17.8,
                -1: 1.0,
            },
            31: {
                9: 4.2,
                8: 5.9,
                7: 7.2,
                6: 8.0,
                5: 8.6,
                4: 9.7,
                3: 10.7,
                2: 11.8,
                1: 13.8,
                0: 17.8,
                -1: 1.0,
            },
            67: {
                9: 1.2,
                8: 2.0,
                7: 3.0,
                6: 4.5,
                5: 5.9,
                4: 7.2,
                3: 8.6,
                2: 9.7,
                1: 10.7,
                0: 11.8,
                -1: 1.0,
            },
        },
        "square_timelapse": {
            0: {
                9: 4.0,
                8: 4.7,
                7: 5.2,
                6: 6.7,
                5: 7.4,
                4: 8.7,
                3: 9.9,
                2: 10.7,
                1: 11.6,
                0: 12.4,
                -1: 1.0,
            },
            31: {
                9: 3.2,
                8: 3.6,
                7: 4.0,
                6: 4.7,
                5: 5.2,
                4: 6.7,
                3: 7.4,
                2: 8.7,
                1: 9.9,
                0: 10.7,
                -1: 1.0,
            },
            67: {
                9: 1.5,
                8: 1.7,
                7: 2.2,
                6: 3.2,
                5: 4.4,
                4: 4.7,
                3: 5.2,
                2: 6.7,
                1: 7.4,
                0: 8.7,
                -1: 1.0,
            },
        },
    }

    bitrate = []
    for row in data.itertuples():
        bitrate.append((100 / estimates[row.video][row.delay_ms][row.quality]))

    data["bitrate"] = bitrate

    plot = sns.relplot(
        x="e2e_delay", y="bitrate", hue="video", data=data, kind="line", style="video"
    )

    # Draw minimum line
    plt.plot([14, 14], [0, 100], color='gray',linewidth=1, linestyle="dotted")
    plt.annotate(
        "min. system latency",
        color="black",
        xy=(14, 83),
        xytext=(20, 95),
        size=12,
        arrowprops=dict(color="black", arrowstyle="->"),
    )

    # Tweak legend
    leg = plot._legend
    leg.set_title("")
    leg.set_bbox_to_anchor([0.98,0.95])
    leg._loc = 1

    sns.despine(bottom=True, left=True)
    plot.set(xlabel="End-to-end Latency (ms)")
    plot.set(ylabel="Bitrate (% of baseline)")
    plot.set(xlim=(0, 90), ylim=(0, 100))
    outfile = "user_study.pdf"
    pp = PdfPages(outfile)
    pp.savefig(plot._fig.tight_layout())
    pp.close()
    #  run(["pdfcrop", outfile, outfile], stdout=DEVNULL, check=True)
    logger.info(f"Plot saved to {outfile}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--data",
        type=str,
        default="../data/user_study.csv",
        help="CSV file of user_study data",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        dest="verbose",
        action="store_true",
        help="Output INFO level logging.",
    )
    args = parser.parse_args()

    if args.verbose:
        ch = logging.StreamHandler()
        logger.setLevel(logging.INFO)
        ch.setLevel(logging.INFO)
        formatter = logging.Formatter("[%(levelname)s] %(name)s - %(message)s")
        ch.setFormatter(formatter)
        logger.addHandler(ch)

    _plot(args.data)
