# Author: Ray Zhang
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$PackageDirectory,
    [Parameter(Mandatory=$true)][string]$MakeNsis,
    [string]$OutputDirectory='D:\Tools\远程ssh'
)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$payload = (Resolve-Path -LiteralPath $PackageDirectory).Path
if (!(Test-Path -LiteralPath (Join-Path $payload 'payload\OpenSSH-Win64\sshd.exe'))) { throw 'Use the full controlled-end staging package.' }
$manifestPath = Join-Path $payload 'manifest.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
foreach ($file in $manifest.files) {
    if ((Get-FileHash -LiteralPath (Join-Path $payload $file.path) -Algorithm SHA256).Hash -ne $file.sha256) { throw "Payload hash mismatch: $($file.path)" }
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Start-Installed.cmd') -Destination (Join-Path $payload '启动.cmd')
$manifest.files = @(Get-ChildItem -LiteralPath $payload -Recurse -File | Where-Object Name -ne 'manifest.json' | Sort-Object FullName | ForEach-Object {
    @{path=$_.FullName.Substring($payload.Length+1).Replace('\','/');sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant();bytes=$_.Length}
})
$manifest.package='主控端与被控端安装程序'
[IO.File]::WriteAllText($manifestPath,($manifest | ConvertTo-Json -Depth 5),[Text.UTF8Encoding]::new($false))
$outRoot=(New-Item -ItemType Directory -Path $OutputDirectory -Force).FullName
$uninstall = Join-Path (Split-Path $payload -Parent) 'uninstall-files.nsh'
$lines = [Collections.Generic.List[string]]::new()
foreach ($file in Get-ChildItem -LiteralPath $payload -Recurse -File) {
    $relative=$file.FullName.Substring($payload.Length+1)
    if ($relative.Contains('$') -or $relative.Contains('"')) { throw "Unsupported installer path: $relative" }
    $lines.Add('Delete "$INSTDIR\' + $relative + '"')
}
foreach ($dir in Get-ChildItem -LiteralPath $payload -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending) {
    $relative=$dir.FullName.Substring($payload.Length+1)
    $lines.Add('RMDir "$INSTDIR\' + $relative + '"')
}
[IO.File]::WriteAllLines($uninstall,$lines,[Text.UTF8Encoding]::new($true))
$output = Join-Path $outRoot 'RY-SSH-远程支持-安装.exe'
& $MakeNsis '/V3' ('/DPAYLOAD_ROOT='+$payload) ('/DOUTPUT_EXE='+$output) ('/DUNINSTALL_FILES='+$uninstall) (Join-Path $PSScriptRoot 'installer.nsi')
if ($LASTEXITCODE -ne 0) { throw 'NSIS build failed.' }
$checksum=(Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText(($output+'.sha256'),($checksum+'  '+[IO.Path]::GetFileName($output)+[Environment]::NewLine),[Text.UTF8Encoding]::new($true))
Get-Item -LiteralPath $output | Select-Object FullName,Length
Write-Output "SHA256=$checksum"
