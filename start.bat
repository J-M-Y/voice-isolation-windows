@echo off
cd /d "%~dp0"
".venv\Scripts\python.exe" -c "import tkinter, numpy, scipy, sounddevice, voxlock, sherpa_onnx" >nul 2>&1
if errorlevel 1 goto missing
".venv\Scripts\python.exe" app.py
if errorlevel 1 pause
exit /b
:missing
echo Python environment is missing or incomplete. Run repair_environment.bat.
pause
exit /b 1
