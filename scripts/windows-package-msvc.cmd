@echo off
setlocal

set "SCRIPT_DIR=%~dp0"
set "SUPPLIED_ARGS=%*"

pwsh.exe -NoProfile -File "%SCRIPT_DIR%windows-package-msvc.ps1" %SUPPLIED_ARGS%
exit /b %ERRORLEVEL%
