@echo off
setlocal
cd /d "%~dp0"
echo Preparing the local Python environment. Internet access is required.
set "BASE_PY="
py -3.12 -c "import sys" >nul 2>&1
if not errorlevel 1 set "BASE_PY=py -3.12"
if defined BASE_PY goto setup
set "BUNDLED_PY=%USERPROFILE%\.cache\codex-runtimes\codex-primary-runtime\dependencies\python\python.exe"
if exist "%BUNDLED_PY%" set "BASE_PY="%BUNDLED_PY%""
if defined BASE_PY goto setup
echo Python 3.12 was not found. Install Python 3.12 from python.org, then run this file again.
pause
exit /b 1
:setup
%BASE_PY% -m venv --upgrade .venv
if errorlevel 1 goto failed
".venv\Scripts\python.exe" -m pip install -r requirements.txt
if errorlevel 1 goto failed
".venv\Scripts\python.exe" -c "import tkinter, numpy, scipy, sounddevice, voxlock, sherpa_onnx"
if errorlevel 1 goto failed
echo Environment ready. Double-click start.bat to open the app.
pause
exit /b 0
:failed
echo Setup failed. Check your internet connection and Python installation.
pause
exit /b 1
