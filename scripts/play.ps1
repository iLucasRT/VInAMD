# Starts the title with the settings this fork is played with (README.md, "How to run it").
#   scripts\play.ps1 -Exe <path of your own copy's executable>
# -Safe         without the newer, faster ways (the first thing to try when something is wrong)
# -RDNA2        optional 16 GB VRAM / 32 GB RAM budget tuning (leaves default invocation unchanged)
# -SeriesS      run with the Xbox Series S console profile (--console series-s)
# -RayTracing   the title's own ray-traced indirect light: lit interiors, about a third slower
# -Build        target\diag (the default, the build all measurements are of) or target\release
# -More         other settings: -More @{VICTORY_PANEL='1'}
# Run it from a normal window, not as administrator: the driver then keeps its shader cache elsewhere.
param(
    [Parameter(Mandatory)][string]$Exe,
    [switch]$Safe,
    [switch]$RDNA2,
    [switch]$SeriesS,
    [switch]$RayTracing,
    [ValidateSet('diag', 'release')][string]$Build = 'diag',
    [hashtable]$More = @{}
)
$settings = @{
    VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_RELAY = '2'; VICTORY_WIDE_BACK = '1'; VICTORY_CARD_REHASH_RUNS = '1'
    VICTORY_JOIN_COPIES = '1'; VICTORY_INDIRECT_WINDOW = '1'; VICTORY_RELAY_COPY = '1'; VICTORY_CARD_STEPS = '2'
    VICTORY_INDIRECT_BATCH = '2'; VICTORY_MAP_THREADS = '1'; VICTORY_TITLE_PRIORITIES = '0'; VICTORY_CARD_AWAKE = '0'
    VICTORY_SHOW_AFTER = '1'; VICTORY_AMBIENT = '0.8'; VICTORY_D3D12_DRED = 'light'
    VICTORY_WINDOW_BUDGET_MB = '2048'
    # Tested in this fork (runs YP19-YP23, RX 7900 XTX): renderer ports 1, 2, 3, 6 and 7 on (port 2
    # with its pages of missed records looked at whole, port 3 reading from a copy: with port 3 in
    # place or port 2's round alone, the indirect light went black); textures kept below 80 % of the
    # card's budget, the card's empty blocks not kept (it filled and spilled into the PC's memory:
    # 7 fps); card-awake off (the driver reset twice with it); windows of title memory in the PC's
    # memory. Reads of the game's files stay buffered: direct reads halved streaming (YP20).
    VICTORY_PORT_CONSTANTS = '1'; VICTORY_PORT_GUARD = '1'; VICTORY_PORT_JOURNAL = '1'
    VICTORY_PORT_DESCRIPTORS = '1'; VICTORY_PORT_DESCRIPTORS_EVERY = '2'; VICTORY_PORT_DESCRIPTORS_CHECK = '32'
    VICTORY_PORT_CBV = '1'; VICTORY_PORT_CBV_PLACE = '0'
    VICTORY_TEXTURE_ROOM_PERCENT = '80'; VICTORY_KEEP_FREED = '0'; VICTORY_WINDOWS_VRAM = '0'
}
if ($Safe) {
    $settings.VICTORY_UNDER_WIDE = '0'; $settings.VICTORY_LAND_ECHO = '0'
} else {
    $settings.VICTORY_THIN = '1'; $settings.VICTORY_JOIN_PARTS = '8'; $settings.VICTORY_UPLOAD_AHEAD = '1'
}
if ($RDNA2) {
    # Optional 16 GB VRAM / 32 GB RAM profile on top of the tested defaults:
    # enables residency admission gating, caps card/window buffers for 13.5 GB WDDM budget,
    # and balances copy/dstorage worker counts on 8C/16T hosts.
    $settings.VICTORY_RESIDENCY = '1'
    $settings.VICTORY_UPLOAD_AHEAD = '0'
    $settings.VICTORY_AMBIENT_AIR = '1.5'
    $settings.VICTORY_CARD_BUFFERS_MIN_MB = '1'
    $settings.VICTORY_CARD_BUFFERS_MB = if ($SeriesS) { '768' } else { '1280' }
    $settings.VICTORY_WINDOW_BUDGET_MB = if ($SeriesS) { '512' } else { '1024' }
    $settings.VICTORY_WIDE_THREADS = '8'
    $settings.VICTORY_DSTORAGE_WORKERS = '8'
}
if ($RayTracing) { $settings.VICTORY_BVH_BUILDER = '1' }
foreach ($k in $More.Keys) { $settings[$k] = $More[$k] }

$victory = Join-Path $PSScriptRoot "..\target\$Build\victory.exe"
if (-not (Test-Path $victory)) { Write-Host "No $victory. Build it first (README.md)."; exit 1 }
if (-not (Test-Path $Exe)) { Write-Host "No $Exe."; exit 1 }
# (Other settings left over in this window go; where your files are stays.)
$paths = 'VICTORY_CONFIG', 'VICTORY_GAME_DIR', 'VICTORY_CACHE_DIR', 'VICTORY_DATA_DIR'
$inherited = @(Get-ChildItem Env:VICTORY_* | Where-Object { $_.Name -notin $paths })
foreach ($entry in $inherited) { Remove-Item "Env:$($entry.Name)" }
foreach ($k in $settings.Keys) { Set-Item "Env:$k" $settings[$k] }
$discarded = $inherited | Where-Object {
    $_.Name -notin $More.Keys -and -not $_.Value.Equals([Environment]::GetEnvironmentVariable($_.Name))
} | Sort-Object Name
if ($discarded) {
    Write-Host "Discarded inherited values for $($discarded.Name -join ', '). Use -More to pass them explicitly."
}
$Exe = (Resolve-Path $Exe).Path
$runArgs = @('run', $Exe)
if ($SeriesS) { $runArgs += @('--console', 'series-s') }
$runArgs += '--execute'
Push-Location (Join-Path $PSScriptRoot '..')   # victory.toml is looked for here
try { & $victory @runArgs } finally { Pop-Location }
