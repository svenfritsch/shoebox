@echo off
rem Windows: double-click. Keep this file next to shoebox.exe (the downloaded folder).
set "HERE=%~dp0"
set "BIN=%HERE%shoebox.exe"
if not exist "%BIN%" set "BIN=%HERE%.shoebox\bin\shoebox.exe"
if not exist "%BIN%" (
    echo shoebox.exe was not found next to this file.
    pause
    exit /b 1
)
echo Starting shoebox. Leave this window open while you use it; close it to stop.
"%BIN%"
