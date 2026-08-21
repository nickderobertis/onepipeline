@echo off
rem The Windows half of the repository's own `pre-push` hook body: the same three
rem verbs `hook.sh` answers, with the same exit codes — 0 when the verb did what
rem it names, 64 (EX_USAGE) for a verb or an argument this script does not have —
rem including an argument the verb does not take, because one this script ignored
rem is one the caller believed it was steering the hook with — and 1 for a verb
rem the host would not let it carry out. See
rem `hook.sh` for what each verb is for and why it refuses rather than defaults;
rem `both_hook_scripts_answer_the_same_verbs` holds the two halves to each other,
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
call :requirestateroot
if errorlevel 1 exit /b 64
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
call :sessionstream
if errorlevel 1 (
  call :fail "append-future-event runs in a tree under a session's run root; no ancestor of %CD% names a stream under %ONEVCS_HOME%\streams"
  exit /b 64
)
echo {"from":"a newer onevcs"}>>"!stream!"
if errorlevel 1 (
  call :broke "cannot append to !stream!"
  exit /b 1
)
exit /b 0

rem The state root itself, established before `break-streams` removes a tree
rem under it. A defined `ONEVCS_HOME` holding a `streams` directory is not one: a
rem profile directory or a typo can be that. Refuses rather than skips. See
rem `hook.sh` for what each check below identifies and why.
:requirestateroot
if not defined ONEVCS_HOME (
  call :fail "ONEVCS_HOME is unset, so there is no session stream to reach; set it to the state root this world gave onevcs, the way World::cmd does"
  exit /b 64
)
if not exist "%ONEVCS_HOME%\registry.json" (
  call :fail "ONEVCS_HOME=%ONEVCS_HOME% holds no registry.json, so it is not a state root onevcs wrote; refusing to remove anything under it"
  exit /b 64
)
for %%h in (locks sessions streams workspaces) do (
  if not exist "%ONEVCS_HOME%\%%h\" (
    call :fail "ONEVCS_HOME=%ONEVCS_HOME% holds no %%h directory, so it is not a state root onevcs wrote; refusing to remove anything under it"
    exit /b 64
  )
)
call :sessionstream
if errorlevel 1 (
  call :fail "ONEVCS_HOME=%ONEVCS_HOME% holds no stream for any ancestor of %CD%, so it is not the state root of the session making this push; refusing to remove %ONEVCS_HOME%\streams"
  exit /b 64
)
exit /b 0

rem The stream of the session this push belongs to, left in `stream`.
rem
rem The tree git runs the hook in depends on the publication policy — a branch
rem pushed from the session's own worktree, or a squash pushed from a scratch
rem worktree beside it — so the session is found by walking up to the first
rem ancestor the state root already holds a stream for. Checked rather than
rem assumed at every step: run anywhere else this finds none and answers 1,
rem instead of naming a stream file no session is writing — a line appended where
rem nothing reads it, and a journey that passes having proved nothing.
:sessionstream
set "dir=%CD%"
:streamloop
for %%i in ("!dir!") do set "candidate=%%~nxi"
set "stream=%ONEVCS_HOME%\streams\!candidate!.ndjson"
if exist "!stream!" exit /b 0
for %%i in ("!dir!") do set "parent=%%~dpi"
if "!parent:~-1!"=="\" set "parent=!parent:~0,-1!"
if "!parent!"=="!dir!" goto streammissing
set "dir=!parent!"
goto streamloop
:streammissing
set "stream="
exit /b 1

:fail
echo pre-push: %~1 1>&2
echo pre-push: the verbs are: wait-for PATH ^| break-streams ^| append-future-event 1>&2
goto :eof

rem A verb that could not do what it names — the host's fault rather than the
rem caller's, and neither is a push the merge path accepted. See `hook.sh`.
:broke
echo pre-push: %~1 1>&2
echo pre-push: the host refused the write, not the caller: check that ONEVCS_HOME is on a writable mount and that no other process is holding this session's stream 1>&2
goto :eof
