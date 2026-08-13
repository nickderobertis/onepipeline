@echo off
rem The Windows half of the repository gate command: the same three verbs
rem `gate.sh` answers, for the same journeys. See that file for what each is for.
setlocal enabledelayedexpansion

if "%~1"=="wait-for" goto waitfor
if "%~1"=="break-streams" goto breakstreams
if "%~1"=="append-future-event" goto appendfuture
echo unknown gate command %~1 1>&2
exit /b 64

:waitfor
if exist "%~2" exit /b 0
rem A short sleep with no dependency on a timeout/powershell that a runner may
rem not have: one ping to the loopback, waited out.
ping -n 1 -w 50 127.0.0.1 >nul 2>&1
goto waitfor

:breakstreams
rmdir /s /q "%ONEVCS_HOME%\streams"
type nul >"%ONEVCS_HOME%\streams"
exit /b 0

:appendfuture
pushd ..
for %%i in ("%CD%") do set "token=%%~nxi"
popd
echo {"from":"a newer onevcs"}>>"%ONEVCS_HOME%\streams\!token!.ndjson"
exit /b 0
