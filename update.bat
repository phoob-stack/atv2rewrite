@echo off
rem Updates the game to the latest version on GitHub (the main branch) and
rem builds it. Double-click it, or run "update.bat --no-pause" from a script.
setlocal
cd /d "%~dp0"
set "WAIT=pause"
if /i "%~1"=="--no-pause" set "WAIT=rem"

where git >nul 2>nul
if errorlevel 1 (
    echo Git was not found. Install it from https://git-scm.com and run this again.
    goto :fail
)
where cargo >nul 2>nul
if errorlevel 1 (
    echo Rust was not found. Install it from https://rustup.rs and run this again.
    goto :fail
)

rem A running game or launcher locks its .exe, so the build could not replace it.
for %%P in (atv2.exe atv2-launcher.exe) do (
    tasklist /fi "imagename eq %%P" | find /i "%%P" >nul
    if not errorlevel 1 (
        echo %%P is running. Close the game and the launcher, then run this again.
        goto :fail
    )
)

echo Downloading the latest version from GitHub...
git fetch origin main
if errorlevel 1 goto :fail

rem Local edits are kept safe in the stash, never thrown away.
git diff --quiet
if errorlevel 1 goto :stash
git diff --cached --quiet
if errorlevel 1 goto :stash
goto :switch
:stash
echo You have local changes; saving them with "git stash" (get them back with "git stash pop")...
git stash push -m "Saved by update.bat"
if errorlevel 1 goto :fail

:switch
git checkout main
if errorlevel 1 goto :fail
git merge --ff-only origin/main
if errorlevel 1 (
    echo Your main branch has commits that are not on GitHub, so it could not be updated.
    goto :fail
)

echo.
echo Building the game. The first build after an update can take a few minutes...
cargo build --release -p atv-game -p atv-launcher -p atv-tool
if errorlevel 1 goto :fail

echo.
for /f "delims=" %%L in ('git log -1 --format^="%%h %%s"') do echo Up to date: %%L
echo Start the game with target\release\atv2-launcher.exe
if /i "%~1"=="--no-pause" exit /b 0
choice /c YN /m "Start the game now"
if errorlevel 2 exit /b 0
start "" "target\release\atv2-launcher.exe"
exit /b 0

:fail
echo.
echo The update did not finish. See the messages above.
%WAIT%
exit /b 1
