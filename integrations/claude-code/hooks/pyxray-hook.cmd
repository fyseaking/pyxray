@echo off
rem pyxray's hook launcher for Windows shells that are not Git Bash.
rem
rem   pyxray-hook.cmd [HARNESS [PHASE]]   see pyxray-hook (the sh launcher)
rem
rem Buffers the payload, then tries %PYXRAY_PYTHON%, the py launcher, python
rem and python3 until one can import pyxray_agent (pyxray_hook.py exits 75
rem when it cannot). Always exits 0; a Cursor pre hook that found nothing to
rem run still prints an explicit allow, since Cursor reads silence as "deny".
setlocal EnableExtensions
set "HERE=%~dp0"
set "HARNESS=%~1"
set "PHASE=%~2"
if "%HARNESS%"=="" set "HARNESS=claude"
if "%PHASE%"=="" set "PHASE=pre"
set "ERRSINK=2>nul"
if "%PYXRAY_DEBUG%"=="1" set "ERRSINK="
set "IN=%TEMP%\pyxray-hook-%RANDOM%%RANDOM%.json"
findstr "^" > "%IN%"

if defined PYXRAY_PYTHON (
  "%PYXRAY_PYTHON%" "%HERE%pyxray_hook.py" %HARNESS% %PHASE% < "%IN%" %ERRSINK% && goto :done
)
where py >nul 2>nul && (py -3 "%HERE%pyxray_hook.py" %HARNESS% %PHASE% < "%IN%" %ERRSINK% && goto :done)
where python >nul 2>nul && (python "%HERE%pyxray_hook.py" %HARNESS% %PHASE% < "%IN%" %ERRSINK% && goto :done)
where python3 >nul 2>nul && (python3 "%HERE%pyxray_hook.py" %HARNESS% %PHASE% < "%IN%" %ERRSINK% && goto :done)

if /I "%HARNESS%"=="cursor" if /I "%PHASE%"=="pre" echo {"permission":"allow"}

:done
del "%IN%" >nul 2>nul
exit /b 0
