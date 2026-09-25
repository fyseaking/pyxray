@echo off
rem pyxray shim for cmd.exe and PowerShell users on Windows.
rem
rem Put this directory on PATH ahead of your Python and `python3` / `python`
rem typed at a prompt go through the interceptor. Note the limit: a program
rem that spawns "python3" with CreateProcess does not resolve .cmd files, so
rem this catches what *you* type, not what an agent's harness spawns — for
rem agents, install the harness hook (integrations\README.md).
rem
rem PYXRAY_PYTHON names the real interpreter; otherwise the `py` launcher.
setlocal
if "%PYXRAY_OFF%"=="1" goto passthrough
if defined PYXRAY_PYTHON (
  "%PYXRAY_PYTHON%" -m pyxray_agent.intercept -- "%PYXRAY_PYTHON%" %*
) else (
  py -3 -m pyxray_agent.intercept -- py -3 %*
)
exit /b %ERRORLEVEL%
:passthrough
if defined PYXRAY_PYTHON (
  "%PYXRAY_PYTHON%" %*
) else (
  py -3 %*
)
exit /b %ERRORLEVEL%
