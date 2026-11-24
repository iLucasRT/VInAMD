# Plays with the settings of run B (Lab 25 candidate): the fastest measured on
# an RX 7900 XTX 24 GB. Every switch is described where the code reads it.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File docs\klbf\play-b.ps1 -Game <dump>\<title exe>
#
# Build first: cargo build --profile diag -p victory --features diagnostics
# Under 16 GB of card memory, lower -CardBuffersMB (the fork's default is 512).
param(
    [Parameter(Mandatory)][string]$Game,
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\diag\victory.exe'),
    [string]$Config = (Join-Path $PSScriptRoot '..\..\victory.toml'),
    [int]$CardBuffersMB = 4096
)
$env:VICTORY_CONFIG = (Resolve-Path $Config).Path
$env:VICTORY_RELAY = '2'                             # submissions sent ahead of the wait, results landed as they finish
$env:VICTORY_WIDE_BACK = '1'                         # snapshots and compares on several threads
$env:VICTORY_CARD_BUFFERS_MIN_MB = '1'               # title buffers from 1 MiB up live in card memory...
$env:VICTORY_CARD_BUFFERS_MB = "$CardBuffersMB"      # ...up to this much
$env:VICTORY_JOIN_COPIES = '1'
$env:VICTORY_CARD_REHASH_RUNS = '1'
$env:VICTORY_PIPELINE_WAIT_MS = '100'                # per frame, instead of 500
& (Resolve-Path $Exe).Path run $Game --execute
