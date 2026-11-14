@echo off
setlocal
cd /d "%~dp0"

rem Usage:  build.cmd        - x64 only (64-bit apps)
rem         build.cmd all    - x64 + x86 (also 32-bit apps)

set DIST=dist
set X64=target\x86_64-pc-windows-msvc\release
set X86=target\i686-pc-windows-msvc\release

echo === Building x64 ===
rustup target add x86_64-pc-windows-msvc || goto :fail
cargo build --release --target x86_64-pc-windows-msvc -p rerout -p rerout-inject -p rerout-hook || goto :fail

if not exist "%DIST%" mkdir "%DIST%"

rem dist\rerout.exe must not be running (closing the window only hides it in the tray)
taskkill /f /im rerout.exe >nul 2>&1
taskkill /f /im fbind.exe >nul 2>&1
timeout /t 1 /nobreak >nul

call :unlock rerout_hook_x64.dll
call :docopy "%X64%\rerout.exe"          "%DIST%\rerout.exe"              || goto :fail
call :docopy "%X64%\rerout-injector.exe" "%DIST%\rerout-injector-x64.exe" || goto :fail
call :docopy "%X64%\rerout_hook.dll"     "%DIST%\rerout_hook_x64.dll"     || goto :fail

if /i not "%~1"=="all" goto :done

echo === Building x86 ===
rustup target add i686-pc-windows-msvc || goto :fail
cargo build --release --target i686-pc-windows-msvc -p rerout-inject -p rerout-hook || goto :fail

call :unlock rerout_hook_x86.dll
call :docopy "%X86%\rerout-injector.exe" "%DIST%\rerout-injector-x86.exe" || goto :fail
call :docopy "%X86%\rerout_hook.dll"     "%DIST%\rerout_hook_x86.dll"     || goto :fail

:done
echo.
echo Done: %DIST%\rerout.exe
goto :end

:fail
echo.
echo BUILD FAILED
set RC=1

:end
rem Keep the window open when started by double-click
echo %cmdcmdline% | find /i "/c" >nul && pause
exit /b %RC%

:unlock
rem A DLL loaded into a running process can't be overwritten, but it CAN be renamed.
rem Every rename gets a unique name, so copies that are still loaded never block the next build.
if exist "%DIST%\%~1" ren "%DIST%\%~1" "%~1.%RANDOM%%RANDOM%.old" >nul 2>&1
rem Best-effort cleanup of old copies (silently skipped while they are still loaded)
del /q "%DIST%\%~1.*.old" >nul 2>&1
exit /b 0

:docopy
copy /y "%~1" "%~2" >nul
if errorlevel 1 (
    echo.
    echo Could not overwrite %~2
    echo It is probably in use: quit Rerout from the tray icon ^(right click, Quit^), then run build.cmd again.
    exit /b 1
)
exit /b 0