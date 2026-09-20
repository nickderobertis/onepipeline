@echo off
rem The Windows half of the host's `maintain` command the journeys in
rem `maintenance.rs` name. `maintain.sh` is where what it leaves behind, what it
rem waits for, and why the wait is bounded are written down; this answers the same
rem way. What is written down here is what cmd makes different.
setlocal enabledelayedexpansion

if not "%~2"=="" (
  echo maintain: takes at most one argument, the path to wait for 1>&2
  exit /b 64
)

if not "%~1"=="" (
  set /a "left=300"
  :waitloop
  if exist "%~1" goto write
  if !left! LEQ 0 (
    echo maintain: nothing wrote %~1 within 300 seconds 1>&2
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
echo maintained in %CD%>>maintained.log
if errorlevel 1 (
  echo maintain: cannot write maintained.log in %CD% 1>&2
  exit /b 1
)
echo maintain: wrote maintained.log in %CD%
exit /b 0
