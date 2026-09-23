@echo off
rem The Windows half of the host's `maintain` command the journeys in
rem `maintenance.rs` name. `maintain.sh` is where what it leaves behind, what it
rem waits for, and why the wait is bounded are written down; this answers the same
rem way — the recorded line carries the arguments it was given, and only a
rem non-empty first argument is a path to wait for. What is written down here is
rem what cmd makes different.
setlocal enabledelayedexpansion

if not "%~1"=="" (
  set /a "left=300"
  :waitloop
  if exist "%~1" goto write
  if !left! LEQ 0 (
    echo maintain: nothing wrote %~1 within 300 seconds, so this hold expired and the sibling records the maintenance as failed. Write that path to release a hold you meant to keep; a hold that ran this long is a journey that ended without releasing it, so read the journey's own panic for why it stopped 1>&2
    exit /b 1
  )
  set /a "left=left-1"
  rem One second a turn, counted the way `hook.bat` counts its ceiling: two pings
  rem to the loopback is the interval, with no dependency on a `timeout` that
  rem refuses a redirected stdin.
  ping -n 2 -w 1000 127.0.0.1 >nul 2>&1
  goto waitloop
)

:write
echo maintained in %CD% with [%*]>>maintained.log
if errorlevel 1 (
  echo maintain: cannot write maintained.log in %CD%; this runs in the slot's worktree, which onevcs cut under ONEVCS_HOME, so check that state root is on a writable mount and that nothing holds the worktree read-only 1>&2
  exit /b 1
)
echo maintain: wrote maintained.log in %CD%
rem The status this fixture is asked to exit with, checked before it reaches
rem `exit /b`. `maintain.sh` says why it is refused rather than clamped and why a
rem leading zero goes with the rest.
rem
rem **Every test below is a quoted `set` or a quoted `if`, and none of them puts the
rem value on a command line.** An unvalidated environment value expanded unquoted in
rem cmd — into a pipe, into `set /a`, into a bare `echo` — is parsed as syntax, so
rem one carrying `&` or `|` would run rather than be refused. So the digits are
rem stripped by substitution and the remainder compared as a quoted string: nothing
rem here is a pipe, nothing is arithmetic, and the one numeric comparison happens
rem only after the value is known to be one to three digits. The refusal names the
rem variable and the range and deliberately does **not** echo the value, for the
rem same reason; `maintain.sh` can quote it safely and does.
set "asked=0"
if defined ONEPIPELINE_E2E_MAINTAIN_EXIT set "asked=%ONEPIPELINE_E2E_MAINTAIN_EXIT%"
if "!asked!"=="0" exit /b 0
if not "!asked:~3!"=="" goto badexit
if "!asked:~0,1!"=="0" goto badexit
set "rest=!asked!"
for %%d in (0 1 2 3 4 5 6 7 8 9) do set "rest=!rest:%%d=!"
if not "!rest!"=="" goto badexit
if !asked! GTR 255 goto badexit
echo maintain: ONEPIPELINE_E2E_MAINTAIN_EXIT=!asked! asks for a maintenance that fails after writing maintained.log, so this exits !asked! and the sibling records a command that failed. Unset it for one that succeeds 1>&2
exit /b !asked!

:badexit
echo maintain: ONEPIPELINE_E2E_MAINTAIN_EXIT holds a value this fixture cannot exit with. Set it to a whole number between 1 and 255 with no leading zero, or unset it for a maintenance that succeeds 1>&2
exit /b 64
