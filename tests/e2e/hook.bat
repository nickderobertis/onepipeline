@echo off
rem The Windows half of the repository's own `pre-push` hook body. `hook.sh` is
rem where the contract both halves answer is written down — the verbs, the exit
rem codes, and why each one refuses rather than defaults — and
rem `both_hook_scripts_answer_the_same_verbs` holds the two in step, because no
rem platform runs both. What is written down here is what cmd makes different.
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
call :waitceiling
if errorlevel 1 exit /b 64
set /a "left=seconds"
:waitloop
if exist "%~2" exit /b 0
if !left! LEQ 0 (
  call :expired "%~2"
  exit /b 1
)
set /a "left=left-1"
rem One second a turn, so the ceiling above is a count of them — with no
rem dependency on a `timeout` that refuses the redirected stdin git hands a hook,
rem or a powershell a runner may not have. Two pings to the loopback is the
rem interval between them, which is a second; the rendezvous is therefore noticed
rem within a second of being written, which every journey that holds a push
rem waits far longer than.
ping -n 2 -w 1000 127.0.0.1 >nul 2>&1
goto waitloop

:breakstreams
if not "%~2"=="" (
  call :fail "break-streams takes no arguments"
  exit /b 64
)
call :requirestateroot
if errorlevel 1 exit /b 64
if exist "%ONEVCS_HOME%\streams" (
  rmdir /s /q "%ONEVCS_HOME%\streams"
  if errorlevel 1 (
    call :broke "cannot remove %ONEVCS_HOME%\streams"
    exit /b 1
  )
)
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

rem How long `wait-for` waits for its rendezvous before it refuses the push, and
rem the environment variable that carries it — both in `hook.sh`, with why the
rem wait is bounded and why a value it does not accept is refused rather than
rem defaulted. What is cmd's own is the leading zero the pattern below turns down:
rem `set /a` reads one as octal.
:waitceiling
set "seconds=300"
if defined ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS set "seconds=%ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS%"
echo !seconds!|findstr /r "^[1-9][0-9]*$" >nul
if errorlevel 1 goto ceilingrefused
rem Length before value: `set /a` and `if GTR` are 32-bit, so a number longer
rem than the bound's own four digits is turned down before either reads it.
if not "!seconds:~4!"=="" goto ceilingrefused
if !seconds! GTR 3600 goto ceilingrefused
exit /b 0
:ceilingrefused
call :fail "ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS holds '!seconds!', which is not a number of seconds between 1 and 3600. Unset it to wait the 300-second default, or set it to a whole number in that range with no leading zero"
exit /b 64

rem A `wait-for` whose ceiling ran out, which is a verb that could not do what it
rem names — the push is refused and the journey holding it fails on the
rem assertions that no longer hold. Worded as `hook.sh` words it, because the
rem journey that proves the expiry runs on whichever platform it finds itself on.
:expired
echo pre-push: nothing wrote %~1 within the ceiling of !seconds! seconds: the held push expired 1>&2
echo pre-push: nothing released this push; write that path to let it through, or raise ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS, which carries the ceiling and is 300 seconds by default 1>&2
goto :eof

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
  call :fail "ONEVCS_HOME=%ONEVCS_HOME% holds no registry.json, so it is not a state root onevcs wrote; refusing to remove anything under it. Point ONEVCS_HOME at the state root this world gave onevcs, the way World::cmd does"
  exit /b 64
)
for %%h in (locks sessions streams workspaces) do (
  if not exist "%ONEVCS_HOME%\%%h\" (
    call :fail "ONEVCS_HOME=%ONEVCS_HOME% holds no %%h directory, so it is not a state root onevcs wrote; refusing to remove anything under it. Point ONEVCS_HOME at the state root this world gave onevcs, the way World::cmd does"
    exit /b 64
  )
)
call :sessionstream
if errorlevel 1 (
  call :fail "ONEVCS_HOME=%ONEVCS_HOME% holds no stream for any ancestor of %CD%, so it is not the state root of the session making this push; refusing to remove %ONEVCS_HOME%\streams. Run this verb from the session's own tree, under the ONEVCS_HOME that session was given"
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
rem The drive root is the top of the walk, and it is recognised *before* the
rem separator is stripped off it, because stripped it stops being a root: cmd
rem resolves a bare `C:` against that drive's own current directory, which is the
rem tree this walk started in, so the turn after would climb back up the same
rem ancestors and this loop would spin forever rather than answer. `hook.sh` gets
rem this from `dirname`, whose `/` is its own parent.
if "!parent:~1!"==":\" goto streammissing
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
