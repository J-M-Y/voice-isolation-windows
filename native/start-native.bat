@echo off
setlocal
if not exist "%~dp0target\release\voice-isolation-native.exe" (
  echo Rust executable not found. See native\README.md for build instructions.
  pause
  exit /b 1
)
start "" "%~dp0target\release\voice-isolation-native.exe"
