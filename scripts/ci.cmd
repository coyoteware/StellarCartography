@echo off
setlocal
rem Local build pipeline. Replaces the GitHub Actions workflow the repo used
rem to have (.github/workflows/ci.yml) and runs the same steps:
rem
rem   1. Frontend: frozen-lockfile install, typecheck + build (tsc && vite build)
rem   2. cargo clippy --workspace --all-targets --locked -- -D warnings
rem   3. cargo test --workspace --locked
rem   4. (only with --installer) pnpm tauri build -> NSIS setup exe in
rem      target\release\bundle\nsis\
rem
rem Usage, from any directory, in cmd or PowerShell:
rem   scripts\ci.cmd              tests only
rem   scripts\ci.cmd --installer  tests, then build the installer

set "INSTALLER=0"
if /i "%~1"=="--installer" set "INSTALLER=1"

pushd "%~dp0.." || exit /b 1

echo === [1/4] Frontend: install + typecheck + build ===
call pnpm --dir app install --frozen-lockfile || goto :fail
call pnpm --dir app build || goto :fail

echo === [2/4] Clippy ===
cargo clippy --workspace --all-targets --locked -- -D warnings || goto :fail

echo === [3/4] Rust tests ===
cargo test --workspace --locked || goto :fail

if "%INSTALLER%"=="1" (
    echo === [4/4] Installer ===
    call pnpm --dir app tauri build || goto :fail
    echo Installer: see target\release\bundle\nsis\
) else (
    echo === [4/4] Installer: skipped - pass --installer to build it ===
)

popd
echo.
echo CI passed.
exit /b 0

:fail
set "CODE=%errorlevel%"
popd
echo.
echo CI FAILED (exit code %CODE%).
exit /b %CODE%
