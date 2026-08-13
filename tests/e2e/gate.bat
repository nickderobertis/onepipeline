@echo off
rem The Windows half of the repository gate command: the same three verbs
rem `gate.sh` answers, with the same exit codes — 0 when the verb did what it
rem names, 64 (EX_USAGE) for a verb or an argument this script does not have —
rem including an argument the verb does not take, because one this script ignored
rem is one the caller believed it was steering the gate with — and 1 for a verb
rem the host would not let it carry out. See
rem `gate.sh` for what each verb is for and why it refuses rather than defaults;
rem `both_gate_scripts_answer_the_same_verbs` holds the two halves to each other,
rem because no platform runs both.
setlocal enabledelayedexpansion

if "%~1"=="wait-for" goto waitfor
if "%~1"=="break-streams" goto breakstreams
if "%~1"=="append-future-event" goto appendfuture
call :fail "unknown command '%~1'"
exit /b 64

:waitfor
if "%~2"=="" (
  call :fail "wait-for takes the path to wait for"
  exit /b 64
)
if not "%~3"=="" (
  call :fail "wait-for takes the path to wait for, and nothing else"
  exit /b 64
)
:waitloop
if exist "%~2" exit /b 0
rem A short sleep with no dependency on a `timeout` that refuses a redirected
rem stdin, or a powershell a runner may not have: one ping to the loopback.
ping -n 1 -w 50 127.0.0.1 >nul 2>&1
goto waitloop

:breakstreams
if not "%~2"=="" (
  call :fail "break-streams takes no arguments"
  exit /b 64
)
if not defined ONEVCS_HOME (
  call :fail "ONEVCS_HOME is unset, so there is no session stream to reach; set it to the state root this world gave onevcs, the way World::cmd does"
  exit /b 64
)
rem `ONEVCS_HOME` is this script's external input and the next line removes a
rem tree under it, so being *defined* is not enough to act on: a variable holding
rem a drive root, a profile directory, or a typo would have this delete a
rem `streams` somewhere nobody meant. What makes it the state root is that
rem `onevcs` has already written the store this verb exists to break — so the
rem store is what is checked, and a path that does not hold one is refused
rem rather than destroyed.
if not exist "%ONEVCS_HOME%\streams\" (
  call :fail "ONEVCS_HOME=%ONEVCS_HOME% holds no streams directory, so it is not the state root onevcs wrote; there is nothing here to break"
  exit /b 64
)
rmdir /s /q "%ONEVCS_HOME%\streams" 2>nul
type nul >"%ONEVCS_HOME%\streams"
if errorlevel 1 (
  call :broke "cannot leave a file where %ONEVCS_HOME%\streams was"
  exit /b 1
)
exit /b 0

:appendfuture
if not "%~2"=="" (
  call :fail "append-future-event takes no arguments"
  exit /b 64
)
if not defined ONEVCS_HOME (
  call :fail "ONEVCS_HOME is unset, so there is no session stream to reach; set it to the state root this world gave onevcs, the way World::cmd does"
  exit /b 64
)
rem The working directory is `onevcs`'s to choose, so the layout this reads a
rem token out of is checked rather than assumed: run anywhere else, the
rem derivation would name some other directory's basename and this would create
rem a stream file no session is writing — a line appended where nothing reads
rem it, and a journey that passes having proved nothing.
for %%i in ("%CD%") do set "here=%%~nxi"
if /i not "!here!"=="worktree" (
  call :fail "append-future-event runs in a session worktree; this is %CD%"
  exit /b 64
)
pushd ..
for %%i in ("%CD%") do set "token=%%~nxi"
popd
set "stream=%ONEVCS_HOME%\streams\!token!.ndjson"
if not exist "!stream!" (
  call :fail "no session stream at !stream!"
  exit /b 64
)
echo {"from":"a newer onevcs"}>>"!stream!"
if errorlevel 1 (
  call :broke "cannot append to !stream!"
  exit /b 1
)
exit /b 0

:fail
echo gate: %~1 1>&2
echo gate: the verbs are: wait-for PATH ^| break-streams ^| append-future-event 1>&2
goto :eof

rem A verb that could not do what it names — the host's fault rather than the
rem caller's, and neither is a gate that passed. See `gate.sh`.
:broke
echo gate: %~1 1>&2
echo gate: the host refused the write, not the caller: check that ONEVCS_HOME is on a writable mount and that no other process is holding this session's stream 1>&2
goto :eof
