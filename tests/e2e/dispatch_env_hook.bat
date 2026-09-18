@echo off
rem The Windows half of the dispatch-env hook the journeys in
rem `dispatch_env_hook.rs` name. `dispatch_env_hook.sh` is where what it records,
rem what it reads, what it says, and why a record it cannot write is refused out
rem loud are written down; this answers the same way. What is written down here is
rem what cmd makes different.
setlocal enabledelayedexpansion

if not defined ONEPIPELINE_E2E_HOOK_RECORD (
  echo dispatch_env_hook: ONEPIPELINE_E2E_HOOK_RECORD names no directory to record into; set it to the world scratch dispatch_env_hook.rs creates 1>&2
  exit /b 64
)
if not defined ONEPIPELINE_RUN_ID (
  echo dispatch_env_hook: the engine named no run in ONEPIPELINE_RUN_ID, which every dispatch-env hook is given; run this only as the command a launch names with --dispatch-env-hook 1>&2
  exit /b 64
)
if not defined ONEPIPELINE_NODE_ID (
  echo dispatch_env_hook: the engine named no node in ONEPIPELINE_NODE_ID, which every dispatch-env hook is given; run this only as the command a launch names with --dispatch-env-hook 1>&2
  exit /b 64
)
set "record=%ONEPIPELINE_E2E_HOOK_RECORD%"
set "run=%ONEPIPELINE_RUN_ID%"
set "node=%ONEPIPELINE_NODE_ID%"
rem One path segment of a run id's own characters, for the reason
rem `dispatch_env_hook.sh` gives; `findstr` is the pattern cmd has.
echo !run!|findstr /r "^[A-Za-z0-9._-][A-Za-z0-9._-]*$" >nul || (call :broke "ONEPIPELINE_RUN_ID holds '!run!', which is not a single run id path segment" & exit /b 70)
if "!run!"=="." (call :broke "ONEPIPELINE_RUN_ID holds '.', which is not a single run id path segment" & exit /b 70)
if "!run!"==".." (call :broke "ONEPIPELINE_RUN_ID holds '..', which is not a single run id path segment" & exit /b 70)
if not exist "%record%\%run%\" mkdir "%record%\%run%" || (call :broke "cannot create %record%\%run%" & exit /b 70)

rem Which invocation this is, claimed rather than counted; `run_end_hook.sh` says
rem why. The ceiling is held against the other half's by
rem `both_dispatch_env_hook_halves_number_an_invocation_the_same_way`.
set "ceiling=100"
set "nth=0"
:number
set /a "nth+=1"
if !nth! GTR !ceiling! goto exhausted
mkdir "%record%\%run%\!nth!" 2>nul || goto number
set "here=%record%\%run%\!nth!"
(echo %node%)>>"%record%\%run%\invocations" || (call :broke "cannot append to %record%\%run%\invocations" & exit /b 70)

(echo %CD%)>"!here!\cwd" || (call :broke "cannot write !here!\cwd" & exit /b 70)
(
  echo hook=%ONEPIPELINE_HOOK%
  echo run_id=%run%
  echo run_root=%ONEPIPELINE_RUN_ROOT%
  echo node_id=%node%
)>"!here!\env" || (call :broke "cannot write !here!\env" & exit /b 70)
rem `set` alone lists every variable as NAME=value, which is what `env` prints.
set >"!here!\environment" || (call :broke "cannot write !here!\environment" & exit /b 70)
rem `findstr "^"` copies stdin through to its end, which is nothing here.
findstr "^" >"!here!\stdin"
if not exist "!here!\stdin" type nul >"!here!\stdin"
rem Parenthesised, because cmd's `echo` keeps the space before a trailing
rem redirection as part of what it says, and the journeys read this line exactly.
(echo dispatch-env hook ran for %node%)1>&2
type nul >"!here!\started" || (call :broke "cannot write !here!\started" & exit /b 70)

if not exist "%record%\%run%.hold" goto scripted
set /a "left=300"
:waitloop
if exist "%record%\%run%.go" goto scripted
if !left! LEQ 0 (
  echo dispatch_env_hook: nothing wrote %record%\%run%.go within 300 seconds; write it to release this hook 1>&2
  exit /b 1
)
set /a "left=left-1"
rem One second a turn, for the reason `hook.bat` gives for the same loop.
ping -n 2 -w 1000 127.0.0.1 >nul 2>&1
goto waitloop

:scripted
set "status=0"
if not exist "%record%\%run%.exit" goto print
set "from=1"
if exist "%record%\%run%.exit-from-nth" set /p from=<"%record%\%run%.exit-from-nth"
echo !from!|findstr /r "^[0-9][0-9]*$" >nul || (call :broke "%record%\%run%.exit-from-nth holds '!from!', which is not an invocation number" & exit /b 70)
if !nth! GEQ !from! set /p status=<"%record%\%run%.exit"
rem 0 to 255, and the length is checked before the value for the reason `hook.bat`
rem gives: `set /a` and `GTR` are 32-bit.
echo !status!|findstr /r "^[0-9][0-9]*$" >nul || (call :broke "%record%\%run%.exit holds '!status!', which is not an exit status from 0 to 255" & exit /b 70)
if not "!status:~3!"=="" (call :broke "%record%\%run%.exit holds '!status!', which is not an exit status from 0 to 255" & exit /b 70)
if !status! GTR 255 (call :broke "%record%\%run%.exit holds '!status!', which is not an exit status from 0 to 255" & exit /b 70)

:print
rem The document, last: a hook that fails may still have printed one, which the
rem engine must not read.
if exist "%record%\%run%.stdout" (
  type "%record%\%run%.stdout" || (call :broke "cannot read %record%\%run%.stdout" & exit /b 70)
) else (
  echo {"version":1,"env":{}}
)
exit /b %status%

rem Nothing under %record%\%run% could be claimed, told apart by whether the last
rem number is there: present is a run past the ceiling, absent is a `mkdir` that was
rem refused for some other reason.
:exhausted
if exist "%record%\%run%\!ceiling!\" (
  call :broke "this run has already recorded !ceiling! invocations under %record%\%run%, which is this fixture's ceiling; a journey that needs more raises it in both halves, which both_dispatch_env_hook_halves_number_an_invocation_the_same_way holds in step"
  exit /b 70
)
call :broke "no record directory could be created under %record%\%run%, so nothing there is claimable"
exit /b 70

rem A record this fixture could not write, or a scripted value it could not use.
:broke
echo dispatch_env_hook: %~1 1>&2
echo dispatch_env_hook: point ONEPIPELINE_E2E_HOOK_RECORD at a writable directory the journey owns, as dispatch_env_hook.rs does, and write ^<run^>.exit as a whole number 1>&2
goto :eof
