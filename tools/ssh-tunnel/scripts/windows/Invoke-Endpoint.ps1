# Copyright (c) 2026 Ray Zhang. Called locally or through existing Windows management.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$RequestFile, [Parameter(Mandatory=$true)][string]$ReceiptFile)
. (Join-Path $PSScriptRoot 'Common.ps1')

$request = Get-Content -LiteralPath $RequestFile -Raw -Encoding UTF8 | ConvertFrom-Json
if ($request.session_id -notmatch '^[a-f0-9]{32}$') { throw 'Invalid session ID.' }
$sessionReceiptPath = Join-Path $PSScriptRoot ('session-' + $request.session_id + '.json')
$keyIntentPath = Join-Path $PSScriptRoot ('key-intent-' + $request.session_id + '.json')
$receipt = @{ ok = $false; state = 'started'; action = $request.action; session_id = $request.session_id; time = [DateTime]::UtcNow.ToString('o'); process_id = $PID }
Write-RYJson $ReceiptFile $receipt
try {
    Assert-RYAdmin
    switch ($request.action) {
        'prepare' {
            $command = Ensure-RYSshd (Join-Path $PSScriptRoot 'payload\OpenSSH-Win64')
            $identity = Get-RYEndpointIdentity -UserName $request.user -Command $command -SourceAddress $request.source_address
            $receipt.identity = $identity
        }
        'authorize' {
            if (Test-Path -LiteralPath $sessionReceiptPath) { throw 'An authorization receipt already exists; clean up or inspect it before retrying.' }
            $command = Get-RYSshdCommand
            if (-not $command) { throw 'sshd is missing.' }
            $identity = Get-RYEndpointIdentity -UserName $request.user -Command $command -SourceAddress $request.source_address
            Initialize-RYKeyFile $identity
            $sessionReceipt = @{ session_id = $request.session_id; state = 'authorizing'; key = $null; firewall_rule = $null; identity = $identity }
            Write-RYJson $sessionReceiptPath $sessionReceipt
            $sessionReceipt.key = Add-RYSessionKey -Path $identity.key_path -PublicKey $request.public_key -SessionID $request.session_id -IntentFile $keyIntentPath
            Write-RYJson $sessionReceiptPath $sessionReceipt
            if ($request.source_address -ne '127.0.0.1') {
                $ip = $null
                if (-not [Net.IPAddress]::TryParse($request.source_address, [ref]$ip) -or $ip.AddressFamily -ne 'InterNetwork') { throw 'Expected one explicit IPv4 firewall source.' }
                $ruleName = 'RY-SSH-Session-' + $request.session_id
                $sessionReceipt.firewall_rule = $ruleName
                Write-RYJson $sessionReceiptPath $sessionReceipt
                New-NetFirewallRule -Name $ruleName -DisplayName $ruleName -Direction Inbound -Action Allow -Protocol TCP -LocalPort 22 -RemoteAddress $ip.IPAddressToString -Profile Any | Out-Null
            }
            $sessionReceipt.state = 'authorized'
            Write-RYJson $sessionReceiptPath $sessionReceipt
            $receipt.identity = $identity
        }
        'cleanup' {
            if (Test-Path -LiteralPath $sessionReceiptPath) {
                $sessionReceipt = Get-Content -LiteralPath $sessionReceiptPath -Raw -Encoding UTF8 | ConvertFrom-Json
                if (-not $sessionReceipt.key -and (Test-Path -LiteralPath $keyIntentPath)) { $sessionReceipt.key = Get-Content -LiteralPath $keyIntentPath -Raw -Encoding UTF8 | ConvertFrom-Json }
                if ($sessionReceipt.key) { Remove-RYSessionKey $sessionReceipt.key }
                if ($sessionReceipt.firewall_rule) { Get-NetFirewallRule -Name $sessionReceipt.firewall_rule -ErrorAction SilentlyContinue | Remove-NetFirewallRule }
                $sessionReceipt.state = 'cleaned'
                Write-RYJson $sessionReceiptPath $sessionReceipt
            }
        }
        default { throw 'Unknown endpoint action.' }
    }
    $receipt.ok = $true
    $receipt.state = 'completed'
} catch {
    $receipt.state = 'failed'
    $receipt.error = $_.Exception.Message
} finally {
    $receipt.time = [DateTime]::UtcNow.ToString('o')
    Write-RYJson $ReceiptFile $receipt
}
if (-not $receipt.ok) { exit 1 }
