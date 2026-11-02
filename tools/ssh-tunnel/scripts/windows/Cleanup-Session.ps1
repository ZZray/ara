# Copyright (c) 2026 Ray Zhang. Run on the controlled jump host after interrupted cleanup.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$SessionDir)
. (Join-Path $PSScriptRoot 'Common.ps1')
. (Join-Path $PSScriptRoot 'Remote-Endpoint.ps1')
Assert-RYAdmin
$root = [IO.Path]::GetFullPath((Join-Path $env:ProgramData 'RY\SSH-Tunnel\sessions')).TrimEnd('\') + '\'
$resolved = [IO.Path]::GetFullPath($SessionDir).TrimEnd('\')
$sessionID = Split-Path -Leaf $resolved
if (-not $resolved.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) -or $sessionID -notmatch '^[a-f0-9]{32}$' -or (Split-Path -Parent $resolved).TrimEnd('\') -ine $root.TrimEnd('\')) {
    throw 'SessionDir must be one direct session directory under ProgramData\RY\SSH-Tunnel\sessions.'
}
$processFile = Join-Path $resolved 'device-process.json'
if (Test-Path -LiteralPath $processFile) {
    $record = Get-Content -LiteralPath $processFile -Raw -Encoding UTF8 | ConvertFrom-Json
    $active = Get-Process -Id $record.process_id -ErrorAction SilentlyContinue
    if ($active -and $active.StartTime.ToUniversalTime().ToString('o') -eq $record.start_time) {
        throw 'The controlled process is still running. End it from the controller or its original window before cleanup.'
    }
}
$stage = Join-Path $resolved 'endpoint'
foreach ($receiptFile in Get-ChildItem -LiteralPath $stage -Filter 'receipt-*.json') {
    $receipt = Get-Content -LiteralPath $receiptFile.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($receipt.state -eq 'started' -and (Get-Process -Id $receipt.process_id -ErrorAction SilentlyContinue)) {
        throw 'An earlier local endpoint operation may still be running; inspect its receipt before cleanup.'
    }
}
$errors = New-Object 'Collections.Generic.List[string]'
try { Invoke-RYEndpointAction -Stage $stage -Request @{ action = 'cleanup'; session_id = $sessionID } -CleanupAfterExit | Out-Null }
catch { $errors.Add('Local cleanup: ' + $_.Exception.Message) }
$backend = $null
$backendFile = Join-Path $resolved 'backend.json'
if (Test-Path -LiteralPath $backendFile) {
    $info = Get-Content -LiteralPath $backendFile -Raw -Encoding UTF8 | ConvertFrom-Json
    while (-not $backend) {
        $user = Read-Host ('后端管理员用户名（默认 ' + $info.user + '；S 暂不清理后端）')
        if ($user -ieq 'S') { $errors.Add('Backend cleanup was skipped; its temporary key or firewall rule may remain.'); break }
        if (-not $user) { $user = $info.user }
        $password = Read-Host '后端密码（不保存）' -AsSecureString
        $cim = $null
        $drive = $null
        try {
            $credential = New-Object Management.Automation.PSCredential($user, $password)
            $cim = New-CimSession -ComputerName '192.168.3.199' -Credential $credential -SessionOption (New-CimSessionOption -Protocol Dcom) -OperationTimeoutSec 15
            $machine = Get-CimInstance -CimSession $cim -ClassName Win32_ComputerSystem -OperationTimeoutSec 15
            if ($machine.Name -ine $info.hostname) { throw 'Backend computer identity differs from this session receipt; cleanup refused.' }
            $driveName = 'RY' + $sessionID.Substring(0,8)
            $drive = New-PSDrive -Name $driveName -PSProvider FileSystem -Root '\\192.168.3.199\C$' -Credential $credential
            $relative = 'ProgramData\RY\SSH-Tunnel\sessions\' + $sessionID
            $backend = @{ cim = $cim; drive = $driveName; root = "$driveName`:\$relative"; windows_root = ('C:\' + $relative); last_operation = $null }
            if (-not (Test-Path -LiteralPath $backend.root)) { throw 'Backend session receipt directory is missing.' }
            foreach ($receiptFile in Get-ChildItem -LiteralPath $backend.root -Filter 'receipt-*.json') {
                $receipt = Get-Content -LiteralPath $receiptFile.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
                if ($receipt.state -eq 'started') {
                    $running = Get-CimInstance -CimSession $cim -ClassName Win32_Process -Filter ('ProcessId=' + [int]$receipt.process_id) -OperationTimeoutSec 15
                    if ($running) { throw 'An earlier backend operation may still be running; inspect its receipt before cleanup.' }
                }
            }
            Invoke-RYEndpointAction -Stage $stage -Backend $backend -Request @{ action = 'cleanup'; session_id = $sessionID } -CleanupAfterExit | Out-Null
        } catch {
            if ($backend) { $errors.Add('Backend cleanup: ' + $_.Exception.Message); break }
            Write-Host ('无法连接后端：' + $_.Exception.Message) -ForegroundColor Yellow
            if ($cim) { Remove-CimSession -CimSession $cim -ErrorAction SilentlyContinue }
            if ($drive) { Remove-PSDrive -Name $driveName -ErrorAction SilentlyContinue }
            if ((Read-Host '输入 R 重试；其他输入保留待处理回执') -ine 'R') { $errors.Add('Backend management connection unavailable.'); break }
        } finally {
            $credential = $null
            $password.Dispose()
        }
    }
}
Disconnect-RYBackend $backend
Write-RYJson (Join-Path $resolved 'cleanup.json') @{ ok = ($errors.Count -eq 0); session_id = $sessionID; errors = @($errors); time = [DateTime]::UtcNow.ToString('o') }
if ($errors.Count -gt 0) { $errors | ForEach-Object { Write-Warning $_ }; exit 1 }
Write-Host '本次授权及规则已撤销，已有 SSH 配置与服务保留。' -ForegroundColor Green
