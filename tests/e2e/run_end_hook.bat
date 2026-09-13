@echo off
rem The Windows half of the run-end hook the journeys in `run_end_hooks.rs` name.
rem `run_end_hook.sh` is where what it records, what it reads and what it says are
rem written down; this answers the same way. What is written down here is what cmd
rem makes different.
setlocal enabledelayedexpansion

if not defined ONEPIPELINE_E2E_HOOK_RECORD (
  echo ONEPIPELINE_E2E_HOOK_RECORD names no directory to record into 1>&2
  exit /b 64
)
if not defined ONEPIPELINE_RUN_ID (
  echo the engine named no run 1>&2
  exit /b 64
)
set "record=%ONEPIPELINE_E2E_HOOK_RECORD%"
set "run=%ONEPIPELINE_RUN_ID%"
if not exist "%record%\%run%\" mkdir "%record%\%run%"

set "count=0"
if exist "%record%\%run%\invocations" (
  for /f %%c in ('type "%record%\%run%\invocations" ^| find /c /v ""') do set "count=%%c"
)
set /a "nth=count+1"
set "here=%record%\%run%\!nth!"
mkdir "!here!"
echo %ONEPIPELINE_HOOK%>>"%record%\%run%\invocations"

echo %CD%>"!here!\cwd"
rem `findstr "^"` copies stdin through to its end, which is the whole document.
findstr "^" >"!here!\stdin"
echo hook=%ONEPIPELINE_HOOK%>"!here!\env"
echo run_id=%run%>>"!here!\env"
echo run_root=%ONEPIPELINE_RUN_ROOT%>>"!here!\env"
if defined ONEPIPELINE_LAUNCHER (
  echo launcher=!ONEPIPELINE_LAUNCHER!>>"!here!\env"
) else (
  echo launcher unset>>"!here!\env"
)
if defined ONEPIPELINE_LAUNCHER_SESSION (
  echo session=!ONEPIPELINE_LAUNCHER_SESSION!>>"!here!\env"
) else (
  echo session unset>>"!here!\env"
)

for /l %%i in (1,1,25) do echo said line %%i
echo said on stderr 1>&2
type nul >"!here!\started"

if not exist "%record%\%run%.hold" goto finish
set /a "left=300"
:waitloop
if exist "%record%\%run%.go" goto finish
if !left! LEQ 0 (
  echo nothing wrote %record%\%run%.go within 300 seconds 1>&2
  exit /b 1
)
set /a "left=left-1"
rem One second a turn, for the reason `hook.bat` gives for the same loop.
ping -n 2 -w 1000 127.0.0.1 >nul 2>&1
goto waitloop

:finish
set "status=0"
if exist "%record%\%run%.exit" set /p status=<"%record%\%run%.exit"
exit /b %status%
