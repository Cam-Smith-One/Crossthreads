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
rem Federation bind: 'auto' uses this device's Tailscale IP once a token is set.
if not defined CT_ADDR set "CT_ADDR=auto"

if not exist "%DAEMON%" (
  echo crossthreadsd.exe not found at "%DAEMON%"
  exit /b 1
)

rem Reclaim the ports: stop any crossthreadsd already running so this launch can
rem bind. Best effort; set CT_NO_KILL=1 to opt out.
if not "%CT_NO_KILL%"=="1" (
  taskkill /F /IM crossthreadsd.exe >nul 2>&1
)

echo Crossthreads is starting - it will open in your browser, signed in.
rem The web app needs a one-time sign-in link; crossthreads open gets it from
rem the daemon with this account's local key once the daemon is up.
start "" /b "%ROOT%\bin\crossthreads.exe" open --wait
"%DAEMON%" --addr "%CT_ADDR%" --http "%CT_HTTP%" --ui "%UI%"
endlocal
