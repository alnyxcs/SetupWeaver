@echo off
REM ============================================================================
REM  SetupWeaver Packager GUI  —  dev launcher
REM  Build (debug) + run.  Works from any directory (desktop shortcut, etc.)
REM ============================================================================

setlocal
pushd "%~dp0"

echo [1/2] Building setupweaver-packager-gui (debug)...
cargo build -p setupweaver-packager-gui
if errorlevel 1 (
    echo.
    echo *** BUILD FAILED — see errors above ***
    popd
    pause
    exit /b 1
)

if not exist "target\debug\setupweaver-packager-gui.exe" (
    echo *** Binary not found after build ***
    popd
    pause
    exit /b 1
)

echo [2/2] Launching setupweaver-packager-gui...
cd /d "%~dp0target\debug"
start "" "setupweaver-packager-gui.exe"

popd
endlocal
