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
    echo maintain: nothing wrote %~1 within 300 seconds; the journey holding this sweep releases it by writing that path, so a hold this long is a journey that ended without releasing it — let the sibling record the failure and read the journey's own panic 1>&2
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
if not defined ONEPIPELINE_E2E_MAINTAIN_SLEEP goto mark
rem Read through delayed expansion only, which substitutes after cmd has parsed
rem the line, so nothing in the value is ever read as part of a command. A
rem value holding anything but digits leaves a token for `for /f` to find; a
rem leading zero is refused too, because `set /a` would read it as octal.
set "sleep_for=!ONEPIPELINE_E2E_MAINTAIN_SLEEP!"
for /f "delims=0123456789" %%c in ("!sleep_for!") do goto badsleep
if "!sleep_for:~0,1!"=="0" if not "!sleep_for!"=="0" goto badsleep
set /a "slept=!sleep_for!"
:sleeploop
if !slept! LEQ 0 goto mark
set /a "slept=slept-1"
ping -n 2 -w 1000 127.0.0.1 >nul 2>&1
goto sleeploop

:mark
echo maintained in %CD%>>maintained.log
if errorlevel 1 (
  echo maintain: cannot write maintained.log in %CD%; this runs in the slot's worktree, which onevcs cut under ONEVCS_HOME, so check that state root is on a writable mount and that nothing holds the worktree read-only 1>&2
  exit /b 1
)
echo maintain: wrote maintained.log in %CD%
if defined ONEPIPELINE_E2E_MAINTAIN_EXIT if not "%ONEPIPELINE_E2E_MAINTAIN_EXIT%"=="0" (
  echo maintain: exiting %ONEPIPELINE_E2E_MAINTAIN_EXIT% because ONEPIPELINE_E2E_MAINTAIN_EXIT asks for a maintenance that fails after writing; unset it for one that succeeds 1>&2
  exit /b %ONEPIPELINE_E2E_MAINTAIN_EXIT%
)
exit /b 0

:badsleep
echo maintain: ONEPIPELINE_E2E_MAINTAIN_SLEEP is '!sleep_for!', which is not a whole number of seconds; set it to digits with no leading zero, such as 20, or unset it for no wait 1>&2
exit /b 64
