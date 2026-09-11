# Play Ashframe.
#
#     pwsh -File scripts/play.ps1
#     pwsh -File scripts/play.ps1 -Screenshot shots/combat.png -AtFrame 620 -Autoplay
#
# Finds Godot the same way the engine does: the `godot` command if it is on
# PATH, then a Godot 4.7 console build beside the checkout. Failing both, it
# says so rather than opening the editor.

[CmdletBinding()]
param(
    # Where to write a PNG of the game and quit. Omit to just play.
    [string]$Screenshot,
    # Which frame to capture, counted from the first. Sixty is about a second.
    [int]$AtFrame = 300,
    # Drive a canned demonstration instead of reading the keyboard, so a
    # capture shows a fight rather than a mech standing still.
    [switch]$Autoplay,
    # Hold a fixed three-quarter view of the player, for looking at the rig.
    [switch]$DebugCam,
    [int]$Width = 1600,
    [int]$Height = 900
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$project = Join-Path $root 'godot'

function Find-Godot {
    $onPath = Get-Command godot -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    $beside = Split-Path -Parent $root
    $found = Get-ChildItem $beside -Recurse -Filter 'Godot_v4.7-stable_win64_console.exe' -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($found) { return $found.FullName }

    throw "Godot 4.7 was not found. Put it on PATH, or install one with: aurum godot --fetch <url> --sha256 <digest>"
}

$godot = Find-Godot

# Godot finds `.gdextension` files while importing, and the list it writes lives
# in `.godot/`, which is not in git. A clone that has never been imported opens
# with no native classes, so import first rather than failing in a way that
# looks like a broken scene.
if (-not (Test-Path (Join-Path $project '.godot/extension_list.cfg'))) {
    Write-Host 'first run: importing the project'
    & $godot --headless --path $project --import | Out-Null
    if (-not (Test-Path (Join-Path $project '.godot/extension_list.cfg'))) {
        throw 'the project could not be imported. Run scripts/setup.ps1 first.'
    }
}

$arguments = @('--path', $project, '--resolution', "${Width}x${Height}")

if ($Screenshot) {
    $full = [System.IO.Path]::GetFullPath((Join-Path $root $Screenshot))
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $full) | Out-Null
    # Godot wants forward slashes in a res://-adjacent path, and a Windows
    # backslash in a command-line argument is an escape.
    $arguments += @('--', '--shot', ($full -replace '\\', '/'), '--shot-frame', $AtFrame, '--quit-after-shot')
    if ($Autoplay) { $arguments += '--autoplay' }
    if ($DebugCam) { $arguments += '--debug-cam' }
    Write-Host "capturing frame $AtFrame to $full"
} else {
    if ($Autoplay) { $arguments += @('--', '--play', '--autoplay') }
}

& $godot @arguments
exit $LASTEXITCODE
