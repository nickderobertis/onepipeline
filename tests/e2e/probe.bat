@echo off
rem The Windows half of probe.sh. Both answer one line on stdout and exit 0; see
rem that file for what the two are for.
if exist "@VERSION_FILE@" type "@VERSION_FILE@"
exit /b 0
