@echo off
cd /d "%~dp0"
".venv\Scripts\python.exe" -c "import voxlock" >nul 2>&1
if errorlevel 1 goto missing
".venv\Scripts\python.exe" download_model.py
pause
exit /b
:missing
echo Python environment is missing. Run repair_environment.bat first.
pause
exit /b 1
