# Timed test run for KLBF Lab / VIctory, XCL or Palmwave on Windows.
# Logs the window title (fps, draws per frame) and keeps a local screenshot every 15 s,
# then stops after N minutes or when the process ends.
# Screenshots stay on your PC: never publish them (game content).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-test.ps1 -Project victory -Exe <victory.exe> -Game <dump>\<title exe> -Config <victory.toml>
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-test.ps1 -Project xcl -Exe <xcl.exe> -Game <dump folder>
#
# XCL / Palmwave always run offline here (XCL_NETWORK=0). Set any other XCL_* / VICTORY_* variable before calling.
param(
    [ValidateSet('victory', 'xcl')][string]$Project = 'victory',  # Palmwave = xcl (same executable name)
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Game,
    [string]$Config = '',                                         # VIctory only: VICTORY_CONFIG
    [string]$Out = (Join-Path $env:TEMP 'klbf-runs'),
    [int]$Minutes = 4
)
$ErrorActionPreference = 'Continue'
if (Get-Process victory, xcl -ErrorAction SilentlyContinue) { throw "A run is already going." }
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace KV -Name W -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr h, out RECT r);
public struct RECT { public int L, T, R, B; }
'@
$dir = Join-Path $Out ("$Project-" + (Get-Date -Format 'yyyyMMdd-HHmmss')); New-Item -ItemType Directory -Force "$dir\screens" | Out-Null
if ($Project -eq 'victory') {
    if ($Config) { $env:VICTORY_CONFIG = $Config }
    $arg = @('run', $Game, '--execute')
} else {
    $env:XCL_NETWORK = '0'; if (-not $env:XCL_ADAPTER) { $env:XCL_ADAPTER = 'radeon' }
    $arg = @('run', $Game)
}
$t0 = Get-Date
$p = Start-Process $Exe -ArgumentList $arg -WorkingDirectory (Split-Path $Exe) `
     -RedirectStandardOutput "$dir\stdout.log" -RedirectStandardError "$dir\stderr.log" -PassThru -NoNewWindow
$null = $p.Handle
while (-not $p.WaitForExit(15000)) {
    $s = [int]((Get-Date) - $t0).TotalSeconds
    if ($s -ge $Minutes * 60) { Get-Process $Project -ErrorAction SilentlyContinue | Stop-Process -Force; "stopped after $Minutes min (no crash)" | Out-File "$dir\stopped.txt"; break }
    try {
        $w = Get-Process $Project -ErrorAction SilentlyContinue | ? { $_.MainWindowHandle -ne 0 } | select -First 1
        $r = New-Object KV.W+RECT
        if ($w -and [KV.W]::GetWindowRect($w.MainWindowHandle, [ref]$r) -and $r.R -gt $r.L) {
            $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T); $g = [System.Drawing.Graphics]::FromImage($bmp)
            $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size); $g.Dispose(); $bmp.Save("$dir\screens\$($s)s.png"); $bmp.Dispose()
            "$s s : $($w.MainWindowTitle)" | Out-File "$dir\titles.txt" -Append -Encoding utf8
        }
    } catch {}
}
"End: code {0}, {1:N0} s" -f $p.ExitCode, ((Get-Date) - $t0).TotalSeconds | Tee-Object "$dir\summary.txt"
