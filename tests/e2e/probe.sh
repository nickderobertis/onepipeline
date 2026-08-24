#!/bin/sh
# A release probe, as the release-targets document's `script` form runs one: a
# direct subprocess in the publication checkout, one line of stdout, exit 0.
#
# What it answers is the file the journey that installed it names, so the same
# committed script answers "nothing is released yet", "0.1.0 is", and "0.2.0 is"
# as the journey moves through them. A file that is not there is a target with no
# release, which is an answer and not a failure — so it prints nothing and exits
# 0, exactly as a probe for an unpublished crate does.
if [ -f "@VERSION_FILE@" ]; then
  cat "@VERSION_FILE@"
fi
exit 0
