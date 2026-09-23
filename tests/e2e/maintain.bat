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
if defined ONEPIPELINE_E2E_MAINTAIN_EXIT if not "%ONEPIPELINE_E2E_MAINTAIN_EXIT%"=="0" (
  echo maintain: ONEPIPELINE_E2E_MAINTAIN_EXIT=%ONEPIPELINE_E2E_MAINTAIN_EXIT% asks for a maintenance that fails after writing maintained.log, so this exits %ONEPIPELINE_E2E_MAINTAIN_EXIT% and the sibling records a command that failed. Unset it for one that succeeds 1>&2
  exit /b %ONEPIPELINE_E2E_MAINTAIN_EXIT%
)
exit /b 0
