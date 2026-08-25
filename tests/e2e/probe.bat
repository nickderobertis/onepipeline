@echo off
rem The Windows half of probe.sh. Both answer one line on stdout and exit 0 for a
rem release, nothing and exit 0 for no release, and a named failure and a
rem non-zero exit for an answer they could not give -- an answer file that is
rem there and empty being one of those, never no release -- and both leave one
rem line per run in the file a journey counts this host's asks by. See that file
rem for what each of them is for.
>>"@RUNS_FILE@" echo run
if errorlevel 1 (
  echo probe: cannot record this run in @RUNS_FILE@, which is where this probe's runs are counted; check that its directory is writable 1>&2
  exit /b 1
)
if not exist "@VERSION_FILE@" exit /b 0
rem The size, taken through a variable rather than tested inside the `for` body:
rem a block nested in a `do` is the one construct in this file whose parsing no
rem journey on the other platform can check.
set PROBE_ANSWER_BYTES=0
for %%A in ("@VERSION_FILE@") do set PROBE_ANSWER_BYTES=%%~zA
if %PROBE_ANSWER_BYTES% equ 0 (
  echo probe: @VERSION_FILE@ is there and holds nothing, which is not an answer; a target with no release has no such file at all, so this is an answer half-written 1>&2
  exit /b 1
)
type "@VERSION_FILE@"
if errorlevel 1 (
  echo probe: cannot read @VERSION_FILE@, which is where this probe's answer is written; check its permissions 1>&2
  exit /b 1
)
exit /b 0
