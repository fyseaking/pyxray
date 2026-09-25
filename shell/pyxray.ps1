# pyxray PowerShell integration — dot-source this from your $PROFILE:
#
#   . C:\path\to\pyxray\shell\pyxray.ps1
#
# This wraps python3/python for *interactive shells you type into*. It does
# NOT catch an agent's commands; for those use the harness hook — see
# integrations\README.md. On Windows the hook is the supported route.
#
#   $env:PYXRAY_DRAW = "never"   draw nothing; follow it with `pyx watch`
#   $env:PYXRAY_LAYOUT = "auto"  one row when dull, the card when not
#   $env:PYXRAY_GATE = "off"     the default; a number refuses above it
#   $env:PYXRAY_OFF = "1"        turn it off for this shell

$script:PyxrayRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if (-not $env:PYTHONPATH) { $env:PYTHONPATH = "$script:PyxrayRoot\python" }
elseif ($env:PYTHONPATH -notlike "*$script:PyxrayRoot\python*") { $env:PYTHONPATH = "$script:PyxrayRoot\python;$env:PYTHONPATH" }

if (-not $env:PYXRAY_BIN) {
    foreach ($candidate in @("$script:PyxrayRoot\target\release\pyx.exe", "$script:PyxrayRoot\target\debug\pyx.exe")) {
        if (Test-Path $candidate) { $env:PYXRAY_BIN = $candidate; break }
    }
}

# The real interpreter, resolved once so the wrapper cannot recurse into itself.
if (-not $env:PYXRAY_PYTHON) {
    $real = Get-Command python -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($real) { $env:PYXRAY_PYTHON = $real.Source }
}

function python3 {
    if ($env:PYXRAY_OFF -eq "1" -or $args.Count -eq 0) { & $env:PYXRAY_PYTHON @args }
    else { & $env:PYXRAY_PYTHON -m pyxray_agent.intercept -- $env:PYXRAY_PYTHON @args }
}
function python { python3 @args }
function pyx { if ($env:PYXRAY_BIN) { & $env:PYXRAY_BIN @args } else { & pyx.exe @args } }
function pyxwatch { pyx watch @args }
