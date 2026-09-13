@echo off
rem The Windows half of the run-end hook the journeys in `run_end_hooks.rs` name.
rem `run_end_hook.sh` is where what it records, what it reads, what it says, and
rem why a record it cannot write is refused out loud are written down; this answers
rem the same way. What is written down here is what cmd makes different.
setlocal enabledelayedexpansion

if not defined ONEPIPELINE_E2E_HOOK_RECORD (
  echo run_end_hook: ONEPIPELINE_E2E_HOOK_RECORD names no directory to record into; set it to the world scratch run_end_hooks.rs creates 1>&2
  exit /b 64
)
if not defined ONEPIPELINE_RUN_ID (
  echo run_end_hook: the engine named no run in ONEPIPELINE_RUN_ID, which every run-end hook is given 1>&2
  exit /b 64
)
set "record=%ONEPIPELINE_E2E_HOOK_RECORD%"
set "run=%ONEPIPELINE_RUN_ID%"
rem One path segment of a run id's own characters, for the reason `run_end_hook.sh`
rem gives; `findstr` is the pattern cmd has.
echo !run!|findstr /r "^[A-Za-z0-9._-][A-Za-z0-9._-]*$" >nul || (call :broke "ONEPIPELINE_RUN_ID holds '!run!', which is not a single run id path segment" & exit /b 70)
if "!run!"=="." (call :broke "ONEPIPELINE_RUN_ID holds '.', which is not a single run id path segment" & exit /b 70)
if "!run!"==".." (call :broke "ONEPIPELINE_RUN_ID holds '..', which is not a single run id path segment" & exit /b 70)
if not exist "%record%\%run%\" mkdir "%record%\%run%" || (call :broke "cannot create %record%\%run%" & exit /b 70)

set "count=0"
if exist "%record%\%run%\invocations" (
  for /f %%c in ('type "%record%\%run%\invocations" ^| find /c /v ""') do set "count=%%c"
)
set /a "nth=count+1"
set "here=%record%\%run%\!nth!"
mkdir "!here!" || (call :broke "cannot create !here!" & exit /b 70)
(echo %ONEPIPELINE_HOOK%)>>"%record%\%run%\invocations" || (call :broke "cannot append to %record%\%run%\invocations" & exit /b 70)

(echo %CD%)>"!here!\cwd" || (call :broke "cannot write !here!\cwd" & exit /b 70)
rem `findstr "^"` copies stdin through to its end, which is the whole document.
findstr "^" >"!here!\stdin"
if not exist "!here!\stdin" (call :broke "cannot write !here!\stdin" & exit /b 70)
(
  echo hook=%ONEPIPELINE_HOOK%
  echo run_id=%run%
  echo run_root=%ONEPIPELINE_RUN_ROOT%
  if defined ONEPIPELINE_LAUNCHER (echo launcher=!ONEPIPELINE_LAUNCHER!) else (echo launcher unset)
  if defined ONEPIPELINE_LAUNCHER_SESSION (echo session=!ONEPIPELINE_LAUNCHER_SESSION!) else (echo session unset)
)>"!here!\env" || (call :broke "cannot write !here!\env" & exit /b 70)

for /l %%i in (1,1,25) do echo said line %%i
echo said on stderr 1>&2
type nul >"!here!\started" || (call :broke "cannot write !here!\started" & exit /b 70)

if not exist "%record%\%run%.hold" goto finish
set /a "left=300"
:waitloop
if exist "%record%\%run%.go" goto finish
if !left! LEQ 0 (
  echo run_end_hook: nothing wrote %record%\%run%.go within 300 seconds; write it to release this hook 1>&2
  exit /b 1
)
set /a "left=left-1"
rem One second a turn, for the reason `hook.bat` gives for the same loop.
ping -n 2 -w 1000 127.0.0.1 >nul 2>&1
goto waitloop

:finish
set "status=0"
if exist "%record%\%run%.exit" set /p status=<"%record%\%run%.exit"
rem 0 to 255, and the length is checked before the value for the reason `hook.bat`
rem gives: `set /a` and `GTR` are 32-bit.
echo !status!|findstr /r "^[0-9][0-9]*$" >nul || (call :broke "%record%\%run%.exit holds '!status!', which is not an exit status from 0 to 255" & exit /b 70)
if not "!status:~3!"=="" (call :broke "%record%\%run%.exit holds '!status!', which is not an exit status from 0 to 255" & exit /b 70)
if !status! GTR 255 (call :broke "%record%\%run%.exit holds '!status!', which is not an exit status from 0 to 255" & exit /b 70)
exit /b %status%

rem A record this fixture could not write, or a scripted value it could not use.
:broke
echo run_end_hook: %~1 1>&2
echo run_end_hook: point ONEPIPELINE_E2E_HOOK_RECORD at a writable directory the journey owns, as run_end_hooks.rs does, and write ^<run^>.exit as a whole number 1>&2
goto :eof
