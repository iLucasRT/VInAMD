# Plays with run U: run H's settings, and more of the textures shaders pick by
# index served per frame (4 GB and 1.5 s at most). Judge the second launch of
# a new executable.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\klbf\play-u.ps1 -Game <dump>\<title exe>
param(
    [Parameter(Mandatory)][string]$Game,
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\diag\victory.exe'),
    [string]$Config = (Join-Path $PSScriptRoot '..\..\victory.toml'),
    [int]$CardBuffersMB = 4096
)
$env:VICTORY_HEAP_TEXTURES_MB = '4096'
$env:VICTORY_HEAP_TEXTURES_MS = '1500'
& (Join-Path $PSScriptRoot 'play-b.ps1') -Game $Game -Exe $Exe -Config $Config -CardBuffersMB $CardBuffersMB
