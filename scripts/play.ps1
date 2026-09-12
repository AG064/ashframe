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
    # The windowed build first, then the console one. They run the same game,
    # but the console build opens a console window alongside it, and that is a
    # second window on somebody's desktop taking their focus for no reason.
    foreach ($name in @('Godot_v4.7-stable_win64.exe', 'Godot_v4.7-stable_win64_console.exe')) {
        $found = Get-ChildItem $beside -Recurse -Filter $name -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -notmatch '\\_console\.exe$' } |
            Select-Object -First 1
        if ($found) { return $found.FullName }
    }

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

# Put the window somewhere it is not in the way.
#
# A capture needs a real rendering context, so it cannot be headless: the dummy
# display driver has nothing to draw into and every frame comes back blank. It
# does not need to be *seen*, though, and for a long time this script opened a
# window in the middle of whatever the person at the keyboard was doing and took
# their focus with it.
#
# So: the secondary display if there is one, and the primary only if there is
# not. Then, once it exists, it is sent to the bottom of the z-order without
# being activated -- Godot creates its window before we can say anything, and a
# window that appears behind everything for four seconds is a great deal less
# rude than one that appears in front.
function Get-CaptureArea {
    try {
        Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop
    } catch {
        # No WinForms, no opinion: Godot will centre itself and that is that.
        return $null
    }
    $screens = [System.Windows.Forms.Screen]::AllScreens
    $target = $screens | Where-Object { -not $_.Primary } | Select-Object -First 1
    if (-not $target) { $target = $screens | Select-Object -First 1 }
    if (-not $target) { return $null }
    return $target.WorkingArea
}

$captureArea = Get-CaptureArea
if ($captureArea) {
    $arguments += @('--position', "$($captureArea.X),$($captureArea.Y)")
}

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

# Send it to the bottom of the z-order once it exists.
#
# The game does not capture the pointer in a capture run and sets a no-focus
# flag on its own window, but it cannot do either until it is running, and Godot
# shows the window before then. This is the outside half of that: the window is
# pushed behind everything else without being activated.
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class ZOrder {
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    public static readonly IntPtr Bottom = new IntPtr(1);
    public const uint NoSize = 0x0001, NoMove = 0x0002, NoZOrder = 0x0004, NoActivate = 0x0010;
}
"@

function Move-Behind {
    param($Process)
    if (-not $Process) { return }
    for ($i = 0; $i -lt 100; $i++) {
        $Process.Refresh()
        if ($Process.HasExited) { return }
        if ($Process.MainWindowHandle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 50
    }
    $Process.Refresh()
    if ($Process.MainWindowHandle -eq [IntPtr]::Zero) { return }
    [void][ZOrder]::SetWindowPos($Process.MainWindowHandle, [ZOrder]::Bottom, 0, 0, 0, 0,
        ([ZOrder]::NoSize -bor [ZOrder]::NoMove -bor [ZOrder]::NoZOrder -bor [ZOrder]::NoActivate))
}

$process = Start-Process -FilePath $godot -ArgumentList $arguments -PassThru
Move-Behind -Process $process
$process.WaitForExit()
exit $process.ExitCode
