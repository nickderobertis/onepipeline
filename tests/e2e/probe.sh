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
# Every run leaves one line in a second file, which is a journey's only way to
# count how many times this host actually asked. A probe is a subprocess, so how
# often one is run is behaviour a journey asserts on — and refusing when the
# tally cannot be written is the point of writing it: a tally nothing recorded
# would let such a journey pass having counted a host that never ran anything.
# Exit 1 is "not answered", which holds a node rather than releasing it.
if ! echo run >>"@RUNS_FILE@"; then
  echo "probe: cannot record this run in @RUNS_FILE@, which is where this probe's runs are counted; check that its directory is writable" >&2
  exit 1
fi
if [ ! -f "@VERSION_FILE@" ]; then
  exit 0
fi
if ! cat "@VERSION_FILE@"; then
  echo "probe: cannot read @VERSION_FILE@, which is where this probe's answer is written; check its permissions" >&2
  exit 1
fi
