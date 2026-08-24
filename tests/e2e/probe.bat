@echo off
rem The Windows half of probe.sh. Both answer one line on stdout and exit 0 for a
rem release, nothing and exit 0 for no release, and a named failure and a
rem non-zero exit for an answer they could not give. See that file for what the
rem three are for.
if not exist "@VERSION_FILE@" exit /b 0
type "@VERSION_FILE@"
if errorlevel 1 (
  echo probe: cannot read @VERSION_FILE@, which is where this probe's answer is written; check its permissions 1>&2
  exit /b 1
)
exit /b 0
