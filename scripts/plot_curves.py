#!/usr/bin/env python
import argparse
import logging
import math
import os
import sys
from subprocess import DEVNULL, run

import matplotlib
import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
import seaborn as sns
from matplotlib.backends.backend_pdf import PdfPages

sns.set(style="whitegrid")
sns.set_context("paper", font_scale=1.5)

logging.basicConfig(
    stream=sys.stdout,
    format="[%(asctime)s][%(levelname)s] %(name)s:%(lineno)s - %(message)s",
    level=logging.WARNING,
)
logger = logging.getLogger(__name__)


def _plot(datafile, outfile):
    """Plotting logic."""
    # Mfr PartNumber,GBWP (kHz),Supply Current (uA),GBWP/uA,min_voltage,max_voltage
    logger.info("Plotting from {}...".format(datafile))

    data = pd.read_csv(datafile, skipinitialspace=True, thousands=",")

    fig, ax = plt.subplots(figsize=(7, 5))
    plot = sns.catplot(
        x="latency",
        y="filesize",
        hue="kind",
        kind="point",
        data=data,
    )

    sns.despine(bottom=True, left=True)
    plot.set(xlabel="motion-to-photon latency (ms)")
    plot.set(ylabel="percent of normal filseize")
    #  plot.set(xlim=(0, 100))
    plot.set(ylim=(0, 100))
    pp = PdfPages(outfile)
    pp.savefig(plot.fig.tight_layout())
    pp.close()
    run(["pdfcrop", outfile, outfile], stdout=DEVNULL, check=True)
    logger.info(f"Plot saved to {outfile}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--data",
        type=str,
        default="./compression.csv",
        help="CSV file of experiment data",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=str,
        default="./latency_vs_size.pdf",
        help="Path where the PDF should be saved.",
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

    _plot(args.data, args.output)
