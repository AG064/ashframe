# Ashframe setup.
#
# A fresh clone has the sources but not the binaries: Godot's import cache,
# the engine add-on's libraries and this project's own extension are all build
# outputs and none of them are in git. This puts them there.
#
# Run from anywhere:
#
#     pwsh -File scripts/setup.ps1
#
# It is safe to run again after pulling; it rebuilds only what moved.

[CmdletBinding()]
param(
    # Build a release extension as well as a debug one.
    [switch]$Release,
    # Skip building the engine's own libraries, for the case where they are
    # already there and the checkout is on another machine.
    [switch]$SkipEngine
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Write-Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }

function Find-GodotForImport {
    $onPath = Get-Command godot -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $beside = Split-Path -Parent $root
    $found = Get-ChildItem $beside -Recurse -Filter 'Godot_v4.7-stable_win64_console.exe' -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($found) { return $found.FullName }
    throw "Godot 4.7 was not found. Put it on PATH, or install one with: aurum godot --fetch <url> --sha256 <digest>"
}

# ── where the engine lives ───────────────────────────────────────────────────

$config = Get-Content 'aurum.toml' -Raw
$hint = [regex]::Match($config, 'path_hint\s*=\s*"([^"]+)"').Groups[1].Value
if ([string]::IsNullOrWhiteSpace($hint)) {
    throw "aurum.toml has no [engine] path_hint; set it to the Aurum checkout."
}
$engine = $hint
if (-not (Test-Path $engine)) {
    throw "the engine checkout named in aurum.toml is not there: $engine"
}
Write-Step "engine: $engine"

# ── the engine add-on ────────────────────────────────────────────────────────

if (-not $SkipEngine) {
    Write-Step 'building the engine libraries'
    Push-Location $engine
    try {
        & cargo build -p aurum-godot -p aurum-editor
        if ($LASTEXITCODE -ne 0) { throw "the engine did not build (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}

Write-Step 'installing the engine add-on'
$source = Join-Path $engine 'godot/addons'
foreach ($addon in @('aurum', 'aurum_editor')) {
    $from = Join-Path $source $addon
    if (-not (Test-Path $from)) { throw "the engine has no $addon add-on at $from" }
    $to = Join-Path $root "godot/addons/$addon"
    New-Item -ItemType Directory -Force -Path $to | Out-Null
    # The engine's add-on is source plus a compiled library. Both are copied:
    # the source because the project runs it, the library because building the
    # engine does not put it in the project.
    Copy-Item (Join-Path $from '*') $to -Recurse -Force
}

# ── this project's extension ─────────────────────────────────────────────────

Write-Step 'building the Ashframe extension'
& cargo build -p ashframe
if ($LASTEXITCODE -ne 0) { throw "ashframe did not build (exit $LASTEXITCODE)" }

if ($Release) {
    Write-Step 'building the release extension'
    & cargo build -p ashframe --release
    if ($LASTEXITCODE -ne 0) { throw "the release build failed (exit $LASTEXITCODE)" }
}

Write-Step 'installing it into the project'
$bin = Join-Path $root 'godot/addons/aurum/bin'
Copy-Item 'target/debug/ashframe.dll' (Join-Path $bin 'ashframe.debug.dll') -Force
if ($Release) {
    Copy-Item 'target/release/ashframe.dll' (Join-Path $bin 'ashframe.dll') -Force
} else {
    # Godot loads the release library when the project is exported or run
    # without the debugger; without one it refuses to open the extension at
    # all, so the debug build stands in.
    Copy-Item 'target/debug/ashframe.dll' (Join-Path $bin 'ashframe.dll') -Force
}

# ── let Godot find the extensions ────────────────────────────────────────────
#
# Godot discovers `.gdextension` files while importing the project and writes
# the list into `.godot/extension_list.cfg`, which is a build output and not in
# git. Without this pass a fresh clone opens with no native classes at all: the
# scene's `AshframeGame` node silently becomes a plain Node and every script
# that talks to it fails on the first frame.
Write-Step 'importing the Godot project'
$import = Find-GodotForImport
$project = Join-Path $root 'godot'

# Twice, and the second run is the one that counts.
#
# Godot 4.7 exits with an access violation at the end of the *first* headless
# import of any project that loads a GDExtension -- after the import has
# finished and the editor settings have been saved, so the work is done and
# only the shutdown is broken. It is not our extension: it reproduces with a
# fifteen-line GDExtension that registers a single empty Node class, and it
# does not happen on a second import or in a windowed one.
#
# Running it again is not a workaround for a broken import; the second run is a
# no-op that exits cleanly, which is what lets this script and a CI job read
# the exit code instead of parsing for errors.
$first = & $import --headless --path $project --import 2>&1
$first | Where-Object { $_ -match 'Parse Error|Cannot get class|ERROR: Failed' } | Select-Object -First 20
$second = & $import --headless --path $project --import 2>&1
$second | Where-Object { $_ -match 'Parse Error|Cannot get class|ERROR: Failed' } | Select-Object -First 20
if ($LASTEXITCODE -ne 0) {
    throw "Godot could not import the project (exit $LASTEXITCODE)"
}

$list = Join-Path $project '.godot/extension_list.cfg'
if (-not (Test-Path $list)) {
    throw "Godot did not write an extension list; the project cannot load native classes."
}
$registered = Get-Content $list -Raw
if ($registered -notmatch 'ashframe\.gdextension') {
    throw "the Ashframe extension is not registered. Godot saw: $registered"
}
Write-Host '    registered:' ($registered -split "`n" | Where-Object { $_ } | ForEach-Object { $_.Trim() }) -Separator ' '

Write-Host ''
Write-Host 'Ready. To play:' -ForegroundColor Green
Write-Host '    aurum run       # through the engine, supervised'
Write-Host '    pwsh -File scripts/play.ps1   # straight through Godot'
