# Copyright (c) 2026 Ray Zhang. Foreground support session, no scheduled task/service for the tunnel.
[CmdletBinding()]
param([string]$Server, [switch]$BackendOnly)
. (Join-Path $PSScriptRoot 'Common.ps1')
. (Join-Path $PSScriptRoot 'Remote-Endpoint.ps1')

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not (New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    # This is the requested interactive installation window, not a background helper.
    $arguments = '-NoLogo -NoProfile -ExecutionPolicy Bypass -File "' + $PSCommandPath + '"'
    if ($BackendOnly) { $arguments += ' -BackendOnly' }
    if ($Server) { $arguments += ' -Server "' + (Get-RYCloudURL $Server) + '"' }
    Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList $arguments -Wait
    exit
}

$packageRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$sessionID = [guid]::NewGuid().ToString('N')
$stateDir = Join-Path $env:ProgramData ('RY\SSH-Tunnel\sessions\' + $sessionID)
$stage = Join-Path $stateDir 'endpoint'
$backend = $null
$agent = $null
$keyHandled = $false
$jumpPrepared = $false
$backendPrepared = $false
$exitCode = 0
$cleanupErrors = New-Object 'Collections.Generic.List[string]'
try {
    Write-Host 'RY SSH 被控端（一次性支持会话）' -ForegroundColor Cyan
    Write-Host '首次安装 SSH 需要管理员权限；已有服务、配置和主机密钥会保留。'
    Write-Host '按 Ctrl+C 或由主控执行 stop 正常结束后会撤销本次公钥；SSH 服务本身保留。'
    Write-Host '强杀、关机或直接关闭窗口可能中断清理，请用 Cleanup-Session.ps1 根据会话回执补做撤销。'
    if (-not $BackendOnly) {
        if (-not $Server) { $Server = Read-Host '云端地址（直接回车使用默认服务）' }
        $Server = Get-RYCloudURL $Server
    }
    $jumpUser = Read-Host "本机 SSH 登录用户名（默认 $env:USERNAME）"
    if ([string]::IsNullOrWhiteSpace($jumpUser)) { $jumpUser = $env:USERNAME }
    $exe = Join-Path $packageRoot 'ry-ssh-tunnel.exe'
    $payload = Join-Path $packageRoot 'payload\OpenSSH-Win64'
    if (-not (Test-Path -LiteralPath $exe) -or -not (Test-Path -LiteralPath (Join-Path $payload 'sshd.exe'))) { throw '程序文件不完整，请重新安装完整程序。' }
    Set-RYPrivateDirectory $stateDir
    Set-RYPrivateDirectory $stage
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Common.ps1'), (Join-Path $PSScriptRoot 'Invoke-Endpoint.ps1') -Destination $stage
    New-Item -ItemType Directory -Path (Join-Path $stage 'payload') | Out-Null
    Copy-Item -LiteralPath $payload -Destination (Join-Path $stage 'payload') -Recurse
    $jumpRequest = @{ action = 'prepare'; session_id = $sessionID; user = $jumpUser; source_address = '127.0.0.1' }
    $jumpReceipt = Invoke-RYEndpointAction -Stage $stage -Request $jumpRequest
    $jumpPrepared = $true
    Write-Host ('本机 SSH 就绪：' + $env:COMPUTERNAME + ' / ' + $jumpUser)
    if ($jumpReceipt.identity.shared_admin_keys) { Write-Warning '此账户使用 Windows 管理员共享密钥文件。本次主控公钥可用于读取该文件的管理员账户，结束时会撤销。' }
    if ($BackendOnly) {
        Write-Host '后端本机 SSH 安装已完成；本次没有启动云端隧道，也没有授权主控公钥。'
        Write-Host '原有 Windows 管理通道若不可用，仍需现场管理员处理，或在可联网的后端直接运行被控端。'
    } else {
        Write-Host '接下来通过现有 Windows 管理通道安装 192.168.3.199 的 SSH。不会开启 WinRM、关闭 UAC 或更改 TrustedHosts。'
        while (-not $backendPrepared) {
            $backendUser = Read-Host '192.168.3.199 管理员用户名（默认 ciqtek-NMR；输入 S 跳过后端）'
            if ($backendUser -ieq 'S') { break }
            if ([string]::IsNullOrWhiteSpace($backendUser)) { $backendUser = 'ciqtek-NMR' }
            $password = Read-Host '192.168.3.199 密码（仅在本窗口内存中使用，不保存）' -AsSecureString
            $credential = New-Object Management.Automation.PSCredential($backendUser, $password)
            try {
                $backend = Connect-RYBackend -Credential $credential -SessionID $sessionID -Stage $stage
                Write-RYJson (Join-Path $stateDir 'backend.json') @{ hostname = $backend.hostname; user = $backendUser; source_address = $backend.source_address; session_id = $sessionID }
                Write-Host ('Windows 管理通道已连接：' + $backend.hostname)
                $backendRequest = @{ action = 'prepare'; session_id = $sessionID; user = $backendUser; source_address = $backend.source_address }
                $backendReceipt = Invoke-RYEndpointAction -Stage $stage -Request $backendRequest -Backend $backend
                $backendPrepared = $true
                if ($backendReceipt.identity.shared_admin_keys) { Write-Warning '后端账户也使用管理员共享密钥文件，本次会话结束时撤销新增公钥。' }
            } catch {
                Write-Host ('后端准备未完成：' + $_.Exception.Message) -ForegroundColor Yellow
                if ($backend -and $backend.last_operation) {
                    # A launched remote install is not an authentication retry.
                    Write-Host '已提交过远程操作，请先检查上面的回执/服务状态。本次不重复安装。'
                    Write-Host '可以只连接跳板；如需要现场安装，在后端运行同一个安装程序，选择被控端，再运行安装目录中的 scripts\windows\Install-Backend-Local.cmd。'
                    $choice = Read-Host '输入 S 仅连接跳板；其他输入退出并检查回执'
                    if ($choice -ieq 'S') { break }
                    throw
                }
                Disconnect-RYBackend $backend
                $backend = $null
                Write-Host '凭据错误可重试。若 SMB 管理共享或 WMI/DCOM 不可达，单靠密码无法远程安装。'
                Write-Host '现场兜底：在后端运行同一个安装程序，选择被控端，再运行安装目录中的 scripts\windows\Install-Backend-Local.cmd。'
                $choice = Read-Host 'R 重新输入凭据；S 只连接跳板；其他输入退出'
                if ($choice -ieq 'S') { break }
                if ($choice -ine 'R') { throw '用户结束后端准备。' }
            } finally {
                $credential = $null
                if ($password) { $password.Dispose(); $password = $null }
            }
        }
        $arguments = 'device -server "' + $Server + '" -state-dir "' + $stateDir + '" -name "' + $env:COMPUTERNAME + '"'
        $agent = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $stateDir 'device.stdout.log') -RedirectStandardError (Join-Path $stateDir 'device.stderr.log')
        Write-RYJson (Join-Path $stateDir 'device-process.json') @{ process_id = $agent.Id; start_time = $agent.StartTime.ToUniversalTime().ToString('o'); executable = $exe }
        $pairShown = $false
        Write-Host '正在连接独立云端隧道服务……'
        while (-not $agent.HasExited) {
            $pairFile = Join-Path $stateDir 'pairing.json'
            if (-not $pairShown -and (Test-Path -LiteralPath $pairFile)) {
                $pair = Get-Content -LiteralPath $pairFile -Raw -Encoding UTF8 | ConvertFrom-Json
                Write-Host ('配对码：' + $pair.code) -ForegroundColor Green
                Write-Host ('请仅发给本次支持人员。失效时间（UTC）：' + $pair.expires_at)
                $pairShown = $true
            }
            $publicKeyFile = Join-Path $stateDir 'controller.pub'
            if (-not $keyHandled -and (Test-Path -LiteralPath $publicKeyFile)) {
                $keyHandled = $true
                try {
                    $publicKey = Get-RYEd25519Key ([IO.File]::ReadAllText($publicKeyFile).Trim())
                    $jumpRequest.action = 'authorize'
                    $jumpRequest.public_key = $publicKey
                    $jumpAuth = Invoke-RYEndpointAction -Stage $stage -Request $jumpRequest
                    $targets = @{ jump = @{ address = '127.0.0.1:22'; user = $jumpUser; host_key = $jumpAuth.identity.host_key } }
                    if ($backendPrepared) {
                        $backendRequest.action = 'authorize'
                        $backendRequest.public_key = $publicKey
                        $backendAuth = Invoke-RYEndpointAction -Stage $stage -Request $backendRequest -Backend $backend
                        $targets.backend = @{ address = '192.168.3.199:22'; user = $backendUser; host_key = $backendAuth.identity.host_key }
                    }
                    Write-RYJson (Join-Path $stateDir 'authorized.json') @{ ok = $true; targets = $targets }
                    Write-Host '配对授权完成；主控现在可连接。按 Ctrl+C 或主控 stop 结束本次会话。' -ForegroundColor Green
                } catch {
                    Write-RYJson (Join-Path $stateDir 'authorized.json') @{ ok = $false; error = 'Endpoint authorization failed; inspect the controlled-side receipt.' }
                    throw
                }
            }
            Start-Sleep -Milliseconds 250
            $agent.Refresh()
        }
        if ($agent.ExitCode -ne 0) {
            $detail = @(Get-Content -LiteralPath (Join-Path $stateDir 'device.stdout.log') -Tail 8 -Encoding UTF8 -ErrorAction SilentlyContinue) + @(Get-Content -LiteralPath (Join-Path $stateDir 'device.stderr.log') -Tail 8 -Encoding UTF8 -ErrorAction SilentlyContinue)
            throw ('隧道已退出，状态码 ' + $agent.ExitCode + '。' + ($detail -join ' '))
        }
    }
} catch {
    $exitCode = 1
    Write-Host ('未完成：' + $_.Exception.Message) -ForegroundColor Red
} finally {
    if ($agent -and -not $agent.HasExited) { Stop-Process -Id $agent.Id -ErrorAction SilentlyContinue; $agent.WaitForExit(5000) | Out-Null }
    if ($keyHandled) {
        if ($jumpPrepared) {
            try { Invoke-RYEndpointAction -Stage $stage -Request @{ action = 'cleanup'; session_id = $sessionID } | Out-Null }
            catch { $cleanupErrors.Add('本机撤销未确认：' + $_.Exception.Message) }
        }
        if ($backendPrepared) {
            try { Invoke-RYEndpointAction -Stage $stage -Backend $backend -Request @{ action = 'cleanup'; session_id = $sessionID } | Out-Null }
            catch { $cleanupErrors.Add('后端撤销未确认：' + $_.Exception.Message) }
        }
    }
    Disconnect-RYBackend $backend
    if (Test-Path -LiteralPath $stateDir) {
        Write-RYJson (Join-Path $stateDir 'cleanup.json') @{ ok = ($cleanupErrors.Count -eq 0); errors = @($cleanupErrors); session_id = $sessionID; time = [DateTime]::UtcNow.ToString('o') }
    }
    if ($cleanupErrors.Count -gt 0) {
        $exitCode = 1
        foreach ($errorText in $cleanupErrors) { Write-Host $errorText -ForegroundColor Red }
        Write-Host '本次授权可能仍在，请保留回执并人工完成撤销；不会把清理失败显示为成功。' -ForegroundColor Red
    } elseif ($keyHandled) { Write-Host '本次公钥及本次防火墙规则已撤销。已有 SSH 服务和原配置保留。' }
    Write-Host ('诊断与回执目录：' + $stateDir)
}
if ($Host.Name -eq 'ConsoleHost') { [void](Read-Host '按回车关闭窗口') }
exit $exitCode
