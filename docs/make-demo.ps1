# Regenerates docs/demo.gif — with puppetty itself.
#
# A pwsh session is started under puppetty, the showcase commands are typed
# into it (char by char, so the recording reads like a human at a keyboard),
# and the session's own .cast recording is rendered to a GIF by
# `puppetty export`. The demo is made of the features it demonstrates.
#
# Usage: pwsh -File docs/make-demo.ps1   (run from the repo root; needs
#        a release engine build and python on PATH)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$pe = Join-Path $repo "engine-rs/target/release/puppetty-engine.exe"
if (-not (Test-Path $pe)) { throw "build the engine first: cargo build --release" }
$stage = Join-Path ([IO.Path]::GetTempPath()) "puppetty-demo-stage"
New-Item -ItemType Directory -Force $stage | Out-Null

# The shell being recorded: neutral prompt, `puppetty` resolving to the
# dev engine (both set invisibly, before recording of keystrokes begins).
$preamble = "function prompt {'PS> '}; function puppetty { & '$pe' @args }; Clear-Host"
foreach ($s in "demo", "py") { & $pe kill $s 2>$null | Out-Null }
# Start-Process, not a pipeline: the detached host inherits this process's
# stdout handle on Windows, so piping `run -d` never sees EOF and hangs.
# No -Wait either — it waits for descendants, i.e. the session host itself.
Start-Process -FilePath $pe -WindowStyle Hidden -ArgumentList @(
    'run', '-d', '--name', 'demo', '--cols', '80', '--rows', '20',
    '--cwd', $stage, '--', 'pwsh', '-NoLogo', '-NoExit', '-Command', $preamble
)
for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 300
    & $pe info demo *> $null
    if ($LASTEXITCODE -eq 0) { break }
}
if ($LASTEXITCODE -ne 0) { throw "demo session did not start" }
Start-Sleep 2

function TypeIn([string]$text, [int]$settleMs) {
    foreach ($ch in $text.ToCharArray()) {
        if ($ch -eq ' ') { & $pe keys demo space | Out-Null }
        else { & $pe send demo --no-enter -- "$ch" | Out-Null }
        Start-Sleep -Milliseconds 30
    }
    Start-Sleep -Milliseconds 400
    & $pe keys demo enter | Out-Null
    Start-Sleep -Milliseconds $settleMs
}

TypeIn 'puppetty run -d --name py -- python -q' 2000   # a detached REPL
TypeIn 'puppetty send py "6 * 7"' 1200
TypeIn 'puppetty read py' 1800                          # the rendered screen
TypeIn 'puppetty snap py -o py.png' 1800                # screenshot it
TypeIn 'puppetty send py "exit()"' 4500                 # REPL ends; session expires
TypeIn 'puppetty read py --last' 2500                   # back from the grave

& $pe kill demo | Out-Null
Start-Sleep 1
& $pe export demo -o (Join-Path $repo "docs/demo.gif") --fps 12
