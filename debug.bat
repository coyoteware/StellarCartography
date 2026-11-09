@echo off
rem Starts StellarCartography for manual debugging:
rem   - the Vite dev server (the frontend, with hot reload) in a window of its own;
rem   - the app, a debug build of the Rust backend, in this window, so its
rem     output and any panic backtrace show here.
rem When the app closes, press R to build and start it again (say, after a
rem Rust change) without restarting the dev server, or Q to stop both.
rem
rem In the app, Ctrl+Shift+I opens the WebView2 dev tools (so does right-click
rem and Inspect on a panel; the map keeps right-click for panning). To debug the
rem Rust side, attach a debugger to target\debug\stellar-cartography.exe once
rem it's running.

setlocal
set "ROOT=%~dp0"
cd /d "%ROOT%"
title StellarCartography app
set "VITE_TITLE=StellarCartography dev server"
set "VITE_URL=http://localhost:1420/"
set "STARTED_VITE="
set "RUST_BACKTRACE=1"

where pnpm.cmd >nul 2>&1
if errorlevel 1 (
  echo pnpm.cmd isn't on PATH. Install it with "npm install -g pnpm", then open a new window.
  goto fail
)
where cargo >nul 2>&1
if errorlevel 1 (
  echo cargo isn't on PATH. Install Rust from https://rustup.rs, then open a new window.
  goto fail
)

if exist "%ROOT%app\node_modules\" goto run_app
echo Installing the frontend packages...
pushd "%ROOT%app"
call pnpm.cmd install
if errorlevel 1 (
  popd
  goto fail
)
popd

:run_app
call :ensure_server
if errorlevel 1 goto fail
echo.
echo Building and starting the app (a debug build; the first one takes a few minutes)...
cargo run -p stellar-cartography
echo.
echo The app exited with code %errorlevel%.
choice /c RQ /n /m "Press R to run it again, or Q to quit: "
if errorlevel 2 goto quit
goto run_app

:quit
if not defined STARTED_VITE goto end
echo Stopping the dev server...
taskkill /fi "WINDOWTITLE eq %VITE_TITLE%*" /t /f >nul 2>&1
:end
endlocal
exit /b 0

:fail
echo.
pause
endlocal
exit /b 1

rem Starts the dev server in its own window unless it's already answering,
rem and waits up to a minute for it.
:ensure_server
call :listening
if not errorlevel 1 exit /b 0
echo Starting the dev server in its own window...
start "%VITE_TITLE%" /d "%ROOT%app" cmd /k pnpm.cmd dev
set "STARTED_VITE=1"
set /a TRIES=0
:wait_server
call :listening
if not errorlevel 1 exit /b 0
set /a TRIES+=1
if %TRIES% geq 60 goto server_timeout
ping -n 2 127.0.0.1 >nul
goto wait_server
:server_timeout
echo The dev server didn't answer within a minute; see its window for errors.
exit /b 1

rem Succeeds when something answers on the dev server's port.
:listening
curl.exe -s -o nul --max-time 2 %VITE_URL%
exit /b
