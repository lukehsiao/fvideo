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

    # Plot PDF
    plot = sns.distplot(
        data["e2e_us"] / 1000.0,
        kde=False,
        bins=25,
        ax=ax
    )

    sns.despine(bottom=True, left=True)
    plot.set(xlabel="End-to-end Latency (ms)")
    plot.set(ylabel="Histogram")
    outfile = "histogram.pdf"
    pp = PdfPages(outfile)
    pp.savefig(plot.get_figure().tight_layout())
    pp.close()
    #  run(["pdfcrop", outfile, outfile], stdout=DEVNULL, check=True)
    logger.info(f"Plot saved to {outfile}")

    # Plot Boxplot
    fig, ax = plt.subplots(figsize=FIGSIZE)
    plot = sns.boxplot(
        data = data,
        orient = "h",
        ax=ax
    )

    sns.despine(bottom=True, top=True)
    plot.set(ylabel="Delay Type")
    plot.set(xlabel="Time (us)")
    outfile = "boxplot.pdf"
    pp = PdfPages(outfile)
    pp.savefig(plot.get_figure().tight_layout())
    pp.close()
    #  run(["pdfcrop", outfile, outfile], stdout=DEVNULL, check=True)
    logger.info(f"Plot saved to {outfile}")

    # Plot CDF
    fig, ax = plt.subplots(figsize=FIGSIZE)
    plot = sns.distplot(
        data["e2e_us"] / 1000.0,
        hist_kws={"cumulative": True, "rwidth": 0.85},
        norm_hist=True,
        #  bins = 45,
        kde=False
    )

    #  handles, labels = ax.get_legend_handles_labels()
    #  ax.legend(handles=handles[1:], labels=labels[1:])
    ax.set_ylim([0, 1])
    #  ax.set_xlim([0.5, 1])

    sns.despine(bottom=True, left=True)
    plot.set(xlabel="End-to-end Latency (ms)")
    plot.set(ylabel="Cumulative Probability")
    outfile = "cdf.pdf"
    pp = PdfPages(outfile)
    pp.savefig(plot.get_figure().tight_layout())
    pp.close()
    #  run(["pdfcrop", outfile, outfile], stdout=DEVNULL, check=True)
    logger.info(f"Plot saved to {outfile}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--data", type=str, default="../data/latency.csv", help="CSV file of latency data"
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
