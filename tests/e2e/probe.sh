#!/bin/sh
# A release probe, as the release-targets document's `script` form runs one: a
# direct subprocess in the publication checkout, one line of stdout, exit 0.
#
# What it answers is the file the journey that installed it names, so the same
# committed script answers "nothing is released yet", "0.1.0 is", and "0.2.0 is"
# as the journey moves through them.
#
# The two endings are different facts and are reported differently. A file that
# is not there is a target with **no release**, which is an answer and not a
# failure: nothing on stdout, exit 0. A file that is there and cannot be read is
# a probe that could not answer, so it says which file and what to do about it,
# and exits non-zero — which `onevcs` reads as "not answered" and never as "not
# released".
#
# A file that is there and **empty** is the second of those and not the first. No
# release is spelled by the file not being there at all, so a present one holding
# nothing is a write this probe was caught in the middle of — and answering
# nothing on exit 0 would have `onevcs` read it as no release, which a host that
# already has a baseline reports as "not released": a probe that could not answer,
# read as a release that has not happened. Nothing this file can hold is read that
# way.
#
# Every run leaves one line in a second file, which is a journey's only way to
# count how often this host asked. A tally nothing recorded would let such a
# journey pass having counted a host that never ran anything, so failing to write
# one fails the probe — exit 1, which holds a node rather than releasing it.
if ! echo run >>"@RUNS_FILE@"; then
  echo "probe: cannot record this run in @RUNS_FILE@, which is where this probe's runs are counted; check that its directory is writable" >&2
  exit 1
fi
if [ ! -f "@VERSION_FILE@" ]; then
  exit 0
fi
if [ ! -s "@VERSION_FILE@" ]; then
  echo "probe: @VERSION_FILE@ is there and holds nothing, which is not an answer; a target with no release has no such file at all, so this is an answer half-written; write it whole by renaming a complete file into place, or remove it to mean no release" >&2
  exit 1
fi
if ! cat "@VERSION_FILE@"; then
  echo "probe: cannot read @VERSION_FILE@, which is where this probe's answer is written; check its permissions" >&2
  exit 1
fi
