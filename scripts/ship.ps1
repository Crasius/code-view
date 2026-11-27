# Step 3 of rule `rust`, Ship: stop the running instances, wait until they are gone, copy the
# release build to dist/ and start it without taking the focus. A copy right after a kill fails
# while Windows still holds the file, so each process is waited for before the copy.
# Usage: pwsh -NoProfile -File scripts/ship.ps1
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$built = Join-Path $root 'target\release\CodeView.exe'
$shipped = Join-Path $root 'dist\CodeView.exe'

$running = @(Get-Process -Name CodeView -ErrorAction SilentlyContinue)
foreach ($process in $running) {
    $process.Kill()
    if (-not $process.WaitForExit(15000)) { throw "CodeView $($process.Id) did not exit within 15 s" }
}
New-Item -ItemType Directory -Force (Split-Path $shipped) | Out-Null
Copy-Item -LiteralPath $built -Destination $shipped -Force
$same = (Get-FileHash $built).Hash -eq (Get-FileHash $shipped).Hash
& pwsh -NoProfile -File (Join-Path $root 'scripts\start-background.ps1') -Exe $shipped
"stopped=$($running.Count) copied=$same built=$((Get-Item $built).LastWriteTime.ToString('HH:mm:ss'))"
