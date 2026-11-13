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

rem dist\rerout.exe must not be running; hook DLLs may be loaded in other apps
taskkill /f /im rerout.exe >nul 2>&1
taskkill /f /im fbind.exe >nul 2>&1
call :unlock rerout_hook_x64.dll

copy /y "%X64%\rerout.exe"          "%DIST%\rerout.exe"                >nul || goto :fail
copy /y "%X64%\rerout-injector.exe" "%DIST%\rerout-injector-x64.exe"   >nul || goto :fail
copy /y "%X64%\rerout_hook.dll"     "%DIST%\rerout_hook_x64.dll"       >nul || goto :fail

if /i not "%~1"=="all" goto :done

echo === Building x86 ===
rustup target add i686-pc-windows-msvc || goto :fail
cargo build --release --target i686-pc-windows-msvc -p rerout-inject -p rerout-hook || goto :fail

call :unlock rerout_hook_x86.dll
copy /y "%X86%\rerout-injector.exe" "%DIST%\rerout-injector-x86.exe"   >nul || goto :fail
copy /y "%X86%\rerout_hook.dll"     "%DIST%\rerout_hook_x86.dll"       >nul || goto :fail

:done
echo.
echo Done: %DIST%\rerout.exe
goto :end

:fail
echo.
echo BUILD FAILED (error code %errorlevel%)
set RC=1

:end
rem Keep the window open when started by double-click
echo %cmdcmdline% | find /i "/c" >nul && pause
exit /b %RC%

:unlock
rem A DLL loaded into a running process can't be overwritten, but it CAN be renamed.
if exist "%DIST%\%~1" (
    del /q "%DIST%\%~1.old" >nul 2>&1
    ren "%DIST%\%~1" "%~1.old" >nul 2>&1
)
exit /b 0
