# Author: Ray Zhang
[CmdletBinding()]
param(
    [string]$Go = 'go',
    [Parameter(Mandatory=$true)][string]$OpenSshArchive,
    [string]$OutputDirectory = 'D:\Tools\远程ssh'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$toolRoot = Split-Path $PSScriptRoot -Parent
$expected = 'A31831BC07C26B88C7D5BF2FDA108F14ADD3D90FAEA51002EC1A62DE56C1B234'
if ((Get-FileHash -LiteralPath $OpenSshArchive -Algorithm SHA256).Hash -ne $expected) {
    throw 'The OpenSSH archive does not match the reviewed offline payload. Review it before changing the pinned hash.'
}
[void](New-Item -ItemType Directory -Path $OutputDirectory -Force)
$outputRoot = (Resolve-Path -LiteralPath $OutputDirectory).Path
$stage = Join-Path $outputRoot ('build-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0,8))
[void](New-Item -ItemType Directory -Path $stage)
$binary = Join-Path $stage 'ry-ssh-tunnel.exe'
$linuxBinary = Join-Path $stage 'ry-ssh-tunnel-linux-amd64'
$previousGOOS = $env:GOOS; $previousGOARCH = $env:GOARCH; $previousCGO = $env:CGO_ENABLED
Push-Location $toolRoot
try {
    $env:GOOS='windows'; $env:GOARCH='amd64'; $env:CGO_ENABLED='0'
    & $Go build -buildvcs=false -trimpath -ldflags '-s -w -X main.version=1.1.2' -o $binary .
    if ($LASTEXITCODE -ne 0) { throw 'Windows Go build failed.' }
    $env:GOOS='linux'
    & $Go build -buildvcs=false -trimpath -ldflags '-s -w -X main.version=1.1.2' -o $linuxBinary .
    if ($LASTEXITCODE -ne 0) { throw 'Linux Go build failed.' }
    $revision = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read source revision.' }
    $sourceDirty = [bool](& git status --porcelain -- .)
} finally {
    Pop-Location
    $env:GOOS=$previousGOOS; $env:GOARCH=$previousGOARCH; $env:CGO_ENABLED=$previousCGO
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $OpenSshArchive).Path)
try {
    foreach ($package in @(@{Name='主控端'; Entry='Start-Controller.cmd'; Full=$false}, @{Name='被控端'; Entry='Start-Controlled.cmd'; Full=$true})) {
        $packageRoot = Join-Path $stage $package.Name
        $windowsScripts = Join-Path $packageRoot 'scripts\windows'
        [void](New-Item -ItemType Directory -Force -Path $windowsScripts)
        Copy-Item -LiteralPath $binary -Destination (Join-Path $packageRoot 'ry-ssh-tunnel.exe')
        Get-ChildItem -LiteralPath (Join-Path $toolRoot 'scripts\windows') -File | Where-Object { $_.Extension -eq '.ps1' } | Copy-Item -Destination $windowsScripts
        Copy-Item -LiteralPath (Join-Path $toolRoot 'scripts\windows\Install-Backend-Local.cmd') -Destination $windowsScripts
        Copy-Item -LiteralPath (Join-Path $toolRoot ('scripts\windows\' + $package.Entry)) -Destination (Join-Path $packageRoot '启动.cmd')
        Copy-Item -LiteralPath (Join-Path $toolRoot 'README.md') -Destination (Join-Path $packageRoot '使用说明.md')
        Copy-Item -LiteralPath (Join-Path $toolRoot 'THIRD-PARTY-NOTICES.md') -Destination $packageRoot
        Copy-Item -LiteralPath (Join-Path $toolRoot 'licenses') -Destination $packageRoot -Recurse
        Copy-Item -LiteralPath (Join-Path $toolRoot 'skills') -Destination $packageRoot -Recurse
        foreach ($entry in $archive.Entries) {
            $name = $entry.FullName.Replace('\','/')
            if (!$name.StartsWith('OpenSSH-Win64/', [StringComparison]::Ordinal)) { continue }
            $relative = $name.Substring('OpenSSH-Win64/'.Length)
            if (!$relative -or $name.EndsWith('/')) { continue }
            if ($relative.Split('/') -contains '..' -or $relative.Contains(':') -or $relative.StartsWith('/')) { throw "Unsafe payload path: $name" }
            if (!$package.Full -and $relative -notin @('ssh.exe','scp.exe','sftp.exe','ssh-keygen.exe','libcrypto.dll','LICENSE.txt','NOTICE.txt')) { continue }
            $destination = Join-Path $packageRoot ('payload\OpenSSH-Win64\' + $relative.Replace('/','\'))
            [void](New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent))
            $inputStream = $entry.Open()
            try {
                $outputStream = [IO.File]::Open($destination,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write)
                try { $inputStream.CopyTo($outputStream) } finally { $outputStream.Dispose() }
            } finally { $inputStream.Dispose() }
        }
        $files = @(Get-ChildItem -LiteralPath $packageRoot -Recurse -File | Sort-Object FullName | ForEach-Object {
            @{ path=$_.FullName.Substring($packageRoot.Length+1).Replace('\','/'); sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(); bytes=$_.Length }
        })
        $manifest = @{ author='Ray Zhang'; package=$package.Name; source_commit=$revision; source_dirty=$sourceDirty; created_utc=[DateTime]::UtcNow.ToString('o'); offline_openssh_sha256=$expected.ToLowerInvariant(); files=$files }
        [IO.File]::WriteAllText((Join-Path $packageRoot 'manifest.json'), ($manifest | ConvertTo-Json -Depth 5), [Text.UTF8Encoding]::new($false))
        $temporaryZip = Join-Path $stage ($package.Name + '.zip')
        [IO.Compression.ZipFile]::CreateFromDirectory($packageRoot, $temporaryZip, [IO.Compression.CompressionLevel]::Optimal, $true)
        $destinationZip = Join-Path $outputRoot ($package.Name + '.zip')
        if (Test-Path -LiteralPath $destinationZip) {
            Move-Item -LiteralPath $destinationZip -Destination (Join-Path $stage ($package.Name + '.previous.zip'))
        }
        Move-Item -LiteralPath $temporaryZip -Destination $destinationZip
        Get-FileHash -LiteralPath $destinationZip -Algorithm SHA256
    }
} finally { $archive.Dispose() }
Copy-Item -LiteralPath $linuxBinary -Destination (Join-Path $outputRoot 'ry-ssh-tunnel-linux-amd64')
$checksumLines = @('主控端.zip','被控端.zip','ry-ssh-tunnel-linux-amd64') | ForEach-Object {
    $hash = (Get-FileHash -LiteralPath (Join-Path $outputRoot $_) -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $_"
}
[IO.File]::WriteAllLines((Join-Path $outputRoot 'SHA256SUMS.txt'), $checksumLines, [Text.UTF8Encoding]::new($true))
Write-Output "Build staging retained for inspection: $stage"
