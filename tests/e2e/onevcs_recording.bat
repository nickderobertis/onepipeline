@echo off
rem The Windows half of the recording `onevcs` the journeys in `maintenance.rs`
rem point the engine at. `onevcs_recording.sh` is where what it records and why
rem are written down; this answers the same way.
setlocal

if not defined ONEPIPELINE_E2E_ONEVCS_CALLS goto unset
if not defined ONEPIPELINE_E2E_ONEVCS_REAL goto unset
echo %*>>"%ONEPIPELINE_E2E_ONEVCS_CALLS%"
if errorlevel 1 (
  echo onevcs_recording: cannot record the call in %ONEPIPELINE_E2E_ONEVCS_CALLS% 1>&2
  exit /b 1
)
"%ONEPIPELINE_E2E_ONEVCS_REAL%" %*
exit /b %errorlevel%

:unset
echo onevcs_recording: ONEPIPELINE_E2E_ONEVCS_CALLS and ONEPIPELINE_E2E_ONEVCS_REAL must both be set; maintenance.rs sets them on the world 1>&2
exit /b 64
