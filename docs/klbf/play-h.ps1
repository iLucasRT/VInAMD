# Plays with run H: run B's settings, and the pipelines of earlier runs waited
# for before the world (VICTORY_PREWARM=0 turns that off). The first launch of
# a new executable takes about 50 s more to load and looks worse: judge the second.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\klbf\play-h.ps1 -Game <dump>\<title exe>
param(
    [Parameter(Mandatory)][string]$Game,
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\diag\victory.exe'),
    [string]$Config = (Join-Path $PSScriptRoot '..\..\victory.toml'),
    [int]$CardBuffersMB = 4096
)
& (Join-Path $PSScriptRoot 'play-b.ps1') -Game $Game -Exe $Exe -Config $Config -CardBuffersMB $CardBuffersMB
