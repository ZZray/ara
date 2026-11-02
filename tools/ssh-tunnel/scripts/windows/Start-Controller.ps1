# Copyright (c) 2026 Ray Zhang.
[CmdletBinding()]
param([string]$Server)
. (Join-Path $PSScriptRoot 'Common.ps1')
$packageRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
try {
    Write-Host 'RY SSH 主控端：输入被控端显示的配对码后连接。'
    if (-not $Server) { $Server = Read-Host '云端地址（直接回车使用默认服务）' }
    $Server = Get-RYCloudURL $Server
    $exe = Join-Path $packageRoot 'ry-ssh-tunnel.exe'
    if (-not (Test-Path -LiteralPath $exe)) { throw '未找到 ry-ssh-tunnel.exe，请重新安装完整程序。' }
    if (-not (Get-Command ssh.exe -ErrorAction SilentlyContinue)) {
        $clientDir = Join-Path $packageRoot 'payload\OpenSSH-Win64'
        if (-not (Test-Path -LiteralPath (Join-Path $clientDir 'ssh.exe'))) { throw '缺少 SSH 客户端，请重新安装完整程序。' }
        $env:PATH = $clientDir + ';' + $env:PATH
    }
    $stateDir = Join-Path $env:LOCALAPPDATA ('RY\SSH-Tunnel\controller\' + [guid]::NewGuid().ToString('N'))
    Set-RYPrivateDirectory $stateDir
    Write-Host '同一窗口可添加多个被控端。stop 结束指定被控端；quit 退出并结束本窗口的连接。'
    & $exe controller -server $Server -state-dir $stateDir
    if ($LASTEXITCODE -ne 0) { throw ('主控端退出，状态码 ' + $LASTEXITCODE) }
} catch {
    Write-Host ('启动失败：' + $_.Exception.Message) -ForegroundColor Red
    exit 1
}
