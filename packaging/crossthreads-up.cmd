@echo off
rem crossthreads-up.cmd - start Crossthreads (daemon + web UI) and open the app.
rem Resolves the UI relative to this script, so it works from an unpacked zip or
rem after install.ps1 has placed it under %USERPROFILE%\.crossthreads.
setlocal
set "HERE=%~dp0"
set "ROOT=%HERE%.."
set "DAEMON=%ROOT%\bin\crossthreadsd.exe"
set "UI=%ROOT%\ui"
if not defined CT_HTTP set "CT_HTTP=127.0.0.1:47101"

if not exist "%DAEMON%" (
  echo crossthreadsd.exe not found at "%DAEMON%"
  exit /b 1
)

echo Crossthreads is starting - it will open at http://%CT_HTTP%
start "" "http://%CT_HTTP%"
"%DAEMON%" --http "%CT_HTTP%" --ui "%UI%"
endlocal
