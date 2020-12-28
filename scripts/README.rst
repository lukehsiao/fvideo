Plotting Scripts
===============

This directory contains some helper plotting scripts. The dependencies are
managed using Poetry_.

Setup
-----

Assuming you have installed Poetry_ and configured it to use Python3, you can
install the dependencies by running::

    poetry shell
    poetry install

Scripts
-------

plot_curves.py
^^^^^^^^^^^^^^
Plots the latency vs filesize curves from compression.csv. From within the
poetry shell, run::

    poetry run python plot_curves.py


.. _Poetry: https://python-poetry.org/
