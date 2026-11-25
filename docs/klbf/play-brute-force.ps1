# Plays with the settings of branch brute-force (run ED2, RX 7900 XTX 24 GB):
# run U's, and what the branch adds. Judge the second launch of a new
# executable: the first one compiles what it needs.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\klbf\play-brute-force.ps1 -Game <dump>\<title exe>
#
# Under 16 GB of card memory, lower -CardBuffersMB (2048 or less) and
# -HeapTexturesMB (1024 or 2048). Close the game's window to end it.
param(
    [Parameter(Mandatory)][string]$Game,
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\diag\victory.exe'),
    [string]$Config = (Join-Path $PSScriptRoot '..\..\victory.toml'),
    [int]$CardBuffersMB = 4096,
    [int]$HeapTexturesMB = 4096,
    [int]$TextureThreads = 8,
    # Descriptor records the game copies as bytes are mirrored. Off here: it
    # cost some 35 ms a frame and its gain is not measured yet.
    [switch]$HeapSync
)
$env:VICTORY_WIDE_REFRESH = '1'
$env:VICTORY_WIDE_MERGE = '1'
$env:VICTORY_INDIRECT_WINDOW = '1'
$env:VICTORY_WRITE_BEHIND = '1'
$env:VICTORY_INPLACE = '1'
$env:VICTORY_WIDE_THREADS = '8'
$env:VICTORY_CLEAN_STAMPS = '1'
$env:VICTORY_HEAP_TEXTURES_SMOOTH_MS = '60'
$env:VICTORY_HEAP_TEXTURES_MB = "$HeapTexturesMB"
$env:VICTORY_HEAP_TEXTURES_MS = '1500'
$env:VICTORY_TEXTURE_THREADS = "$TextureThreads"   # textures made ready on threads of their own
$env:VICTORY_HEAP_SYNC = if ($HeapSync) { '1' } else { '0' }
& (Join-Path $PSScriptRoot 'play-b.ps1') -Game $Game -Exe $Exe -Config $Config -CardBuffersMB $CardBuffersMB
