param(
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$TargetDir = 'D:\WorkSpace\target\handwave-node-release'
)
$ErrorActionPreference = 'Stop'
$taskProject = Split-Path $PSScriptRoot -Parent
$taskOriginalTarget = $env:CARGO_TARGET_DIR
$taskOriginalJobs = $env:CARGO_BUILD_JOBS
$taskOriginalFlags = $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS
$taskStageParent = $null
$taskStageCreated = $false
try {
    $env:CARGO_TARGET_DIR = $TargetDir
    $env:CARGO_BUILD_JOBS = '2'
    if ($Target -eq 'x86_64-pc-windows-msvc') {
        # Ship a standalone executable without requiring the VC runtime installer.
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
    }
    Push-Location $taskProject
    try {
        & cargo build --release --locked --target $Target
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    } finally { Pop-Location }
    $taskSuffix = if ($Target -match 'windows') { '.exe' } else { '' }
    $taskBinary = Join-Path $TargetDir "$Target\release\handwave-node$taskSuffix"
    $taskPackage = Join-Path $taskProject "dist\handwave-node-$Target"
    $taskTemporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $taskStageParent = Join-Path $taskTemporaryRoot ("handwave-package-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $taskStageParent | Out-Null
    $taskStageCreated = $true
    $taskStage = Join-Path $taskStageParent "handwave-node-$Target"
    New-Item -ItemType Directory -Path $taskStage | Out-Null
    New-Item -ItemType Directory -Path $taskPackage -Force | Out-Null
    Copy-Item -LiteralPath $taskBinary -Destination $taskStage
    foreach ($taskFile in @('README.md','node.conf.example','LICENSE','THIRD-PARTY-NOTICES.md')) {
        Copy-Item -LiteralPath (Join-Path $taskProject $taskFile) -Destination $taskStage
    }
    Copy-Item -LiteralPath (Join-Path $taskProject 'licenses') -Destination $taskStage -Recurse
    if ($Target -match 'windows') { Copy-Item -LiteralPath (Join-Path $taskProject 'start-node.cmd') -Destination $taskStage }
    $taskDigest = (Get-FileHash -LiteralPath $taskBinary -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $taskStage 'SHA256SUMS') -Value "$taskDigest  handwave-node$taskSuffix" -Encoding ascii
    # Update shipped files without removing the user's configuration in the extracted folder.
    Get-ChildItem -LiteralPath $taskStage | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $taskPackage -Recurse -Force }
    $taskZip = "$taskPackage.zip"
    # Archive a fresh allowlist, never an extracted folder that may now contain real node.conf/history.
    Compress-Archive -LiteralPath $taskStage -DestinationPath $taskZip -Force
    [pscustomobject]@{Binary=$taskBinary;Bytes=(Get-Item -LiteralPath $taskBinary).Length;Zip=$taskZip;ZipBytes=(Get-Item -LiteralPath $taskZip).Length;SHA256=$taskDigest}|ConvertTo-Json
} finally {
    $env:CARGO_TARGET_DIR = $taskOriginalTarget
    $env:CARGO_BUILD_JOBS = $taskOriginalJobs
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = $taskOriginalFlags
    if ($taskStageCreated -and $taskStageParent -and (Test-Path -LiteralPath $taskStageParent)) {
        $taskResolvedStage = [IO.Path]::GetFullPath($taskStageParent)
        $taskResolvedTemp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        if (-not $taskResolvedStage.StartsWith($taskResolvedTemp, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $taskResolvedStage -Leaf) -notmatch '^handwave-package-[0-9a-f]{32}$') {
            throw 'Refusing cleanup outside the task-created package staging directory'
        }
        Remove-Item -LiteralPath $taskResolvedStage -Recurse -Force
    }
}

