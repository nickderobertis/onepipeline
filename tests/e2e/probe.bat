@echo off
rem The Windows half of probe.sh. Both answer one line on stdout and exit 0 for a
rem release, nothing and exit 0 for no release, and a named failure and a
rem non-zero exit for an answer they could not give, and both leave one line per
rem run in the file a journey counts this host's asks by. See that file for what
rem each of them is for.
>>"@RUNS_FILE@" echo run
if errorlevel 1 (
  echo probe: cannot record this run in @RUNS_FILE@, which is where this probe's runs are counted; check that its directory is writable 1>&2
  exit /b 1
)
if not exist "@VERSION_FILE@" exit /b 0
type "@VERSION_FILE@"
if errorlevel 1 (
  echo probe: cannot read @VERSION_FILE@, which is where this probe's answer is written; check its permissions 1>&2
  exit /b 1
)
exit /b 0
