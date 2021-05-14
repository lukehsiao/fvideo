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
sns.set_context("paper", font_scale=1.5, rc={"lines.linewidth": 1.50})


# Configure logging
logging.basicConfig(
    stream=sys.stdout,
    format="[%(asctime)s][%(levelname)s] %(name)s:%(lineno)s - %(message)s",
    level=logging.INFO,
)
logger = logging.getLogger(__name__)

FIGSIZE = (7, 3)


def _plot(infile):
    """Plotting logic."""
    fig, ax = plt.subplots(figsize=FIGSIZE)

    # type,b,f1,precision,recall
    data = pd.read_csv(infile, skipinitialspace=True)

    # Plot PDF
    plot = sns.distplot(data["e2e_us"] / 1000.0, kde=False, bins=25, ax=ax)

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
    plot = sns.boxplot(data=data, orient="h", ax=ax)

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
    min_data = pd.read_csv("../data/min_results_1000.csv", skipinitialspace=True)
    min_data["kind"] = "lower-bound"
    min_data["e2e_ms"] = min_data["e2e (us)"] / 1000.0

    fig, ax = plt.subplots(figsize=FIGSIZE)
    data["e2e_ms"] = data["e2e_us"] / 1000.0
    data["kind"] = "fvideo"
    data = data.append(min_data[["e2e_ms", "kind"]])

    plot = sns.ecdfplot(data=data, x="e2e_ms", hue="kind", palette="colorblind")

    # Remove legend title
    plot.get_legend().set_title("")
    ax.set_ylim([0, 1])
    #  ax.set_xlim([0.5, 1])

    sns.despine(bottom=True, left=True)
    plot.set(xlabel="End-to-end Latency (ms)")
    plot.set(ylabel="Proportion")
    outfile = "cdf.pdf"
    pp = PdfPages(outfile)
    pp.savefig(plot.get_figure().tight_layout())
    pp.close()
    run(["pdfcrop", outfile, outfile], stdout=DEVNULL, check=True)
    logger.info(f"Plot saved to {outfile}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--data",
        type=str,
        default="../data/latency.csv",
        help="CSV file of latency data",
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
