# Copyright (c) 2026 Ray Zhang. No service, firewall, network, or account changes.
[CmdletBinding()]
param([string]$OpenSshPayload)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$scripts = Join-Path $root 'scripts\windows'
. (Join-Path $scripts 'Common.ps1')
. (Join-Path $scripts 'Remote-Endpoint.ps1')
$temp = Join-Path ([IO.Path]::GetTempPath()) ('ry-ssh-script-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temp | Out-Null
$passed = New-Object 'Collections.Generic.List[string]'
function Assert-RYTest { param([bool]$Condition,[string]$Name) if (-not $Condition) { throw "FAIL: $Name" }; $passed.Add($Name) }
function Assert-RYThrows { param([scriptblock]$Action,[string]$Name) $threw=$false; try { & $Action } catch {$threw=$true}; Assert-RYTest $threw $Name }

try {
    foreach ($file in Get-ChildItem -LiteralPath $scripts -Filter '*.ps1') {
        $tokens=$null; $errors=$null
        [void][Management.Automation.Language.Parser]::ParseFile($file.FullName,[ref]$tokens,[ref]$errors)
        Assert-RYTest ($errors.Count -eq 0) ('PS5 syntax: ' + $file.Name)
    }
    Assert-RYTest ((Get-RYCloudURL '') -eq 'https://nmrkit.ciqtek.com/ssh-tunnel') 'default independent cloud URL'
    Assert-RYThrows {Get-RYCloudURL 'http://example.com'} 'refuse plaintext cloud URL'
    Assert-RYThrows {Get-RYCloudURL 'https://secret@example.com'} 'refuse URL credentials'
    Assert-RYThrows {Get-RYCloudURL 'https://example.com/?token=secret'} 'refuse URL query'
    $blob = [byte[]](0,0,0,11,115,115,104,45,101,100,50,53,53,49,57,0,0,0,32) + (1..32 | ForEach-Object {[byte]$_})
    $key = 'ssh-ed25519 ' + [Convert]::ToBase64String($blob)
    Assert-RYTest ((Get-RYEd25519Key $key) -eq $key) 'valid Ed25519 key'
    Assert-RYThrows {Get-RYEd25519Key ('command="whoami" '+$key)} 'reject key options'
    Assert-RYThrows {Get-RYEd25519Key ($key+"`n"+$key)} 'reject extra key line'
    Assert-RYThrows {Get-RYEd25519Key 'ssh-ed25519 AQID'} 'reject short key blob'
    $session = '0123456789abcdef0123456789abcdef'
    $path = Join-Path $temp 'authorized_keys'
    $old = [Text.Encoding]::UTF8.GetBytes('# 保留中文、选项及无尾换行' + "`r`n" + 'restrict ssh-rsa AAAA existing')
    [IO.File]::WriteAllBytes($path,$old)
    $intent = Join-Path $temp 'intent.json'
    $receipt = Add-RYSessionKey -Path $path -PublicKey $key -SessionID $session -IntentFile $intent
    Assert-RYTest (Test-Path -LiteralPath $intent) 'authorization intent persisted'
    Assert-RYTest (Test-RYTape ([IO.File]::ReadAllBytes($path)) 0 $old) 'append preserves all original bytes'
    Assert-RYThrows {Add-RYSessionKey -Path $path -PublicKey $key -SessionID $session} 'duplicate session refuses replay'
    Remove-RYSessionKey $receipt
    Assert-RYTest ((Get-RYHashBytes ([IO.File]::ReadAllBytes($path))) -eq (Get-RYHashBytes $old)) 'cleanup restores original bytes'
    $receipt = Add-RYSessionKey -Path $path -PublicKey $key -SessionID $session
    [IO.File]::AppendAllText($path,'ssh-rsa OTHER another-admin' + "`n",(New-Object Text.UTF8Encoding($false)))
    Remove-RYSessionKey $receipt
    Assert-RYTest ([IO.File]::ReadAllText($path).Contains('ssh-rsa OTHER another-admin')) 'cleanup preserves concurrent appended key'
    Assert-RYTest (-not [IO.File]::ReadAllText($path).Contains('ry-session:')) 'cleanup removes only current marker'
    $receipt = Add-RYSessionKey -Path $path -PublicKey $key -SessionID $session
    [IO.File]::WriteAllText($path,'# prepended by another administrator' + "`n" + [IO.File]::ReadAllText($path),(New-Object Text.UTF8Encoding($false)))
    Remove-RYSessionKey $receipt
    Assert-RYTest ([IO.File]::ReadAllText($path).StartsWith('# prepended')) 'cleanup handles shifted line without restoring old file'
    $receipt = Add-RYSessionKey -Path $path -PublicKey $key -SessionID $session
    [IO.File]::WriteAllText($path,[IO.File]::ReadAllText($path).Replace($receipt.line,'restrict ' + $receipt.line),(New-Object Text.UTF8Encoding($false)))
    Assert-RYThrows {Remove-RYSessionKey $receipt} 'edited session key requires manual cleanup'
    $json = Join-Path $temp 'atomic.json'
    Write-RYJson $json @{ok=$true; note='中文'}
    Write-RYJson $json @{ok=$false; note='第二次'}
    Assert-RYTest ((Get-Content -LiteralPath $json -Raw -Encoding UTF8 | ConvertFrom-Json).note -eq '第二次') 'atomic UTF8 JSON replace'
    $jsonBytes=[IO.File]::ReadAllBytes($json)
    Assert-RYTest (-not (Test-RYTape $jsonBytes 0 ([byte[]](239,187,191)))) 'JSON has no BOM for Go reader'
    $drive=New-PSDrive -Name RYTestJson -PSProvider FileSystem -Root $temp
    try {
        Write-RYJson 'RYTestJson:\drive.json' @{ok=$true}
        Assert-RYTest (Get-Content 'RYTestJson:\drive.json' -Raw | ConvertFrom-Json).ok 'atomic JSON resolves PowerShell drive paths for SMB requests'
    } finally {Remove-PSDrive -Name RYTestJson}
    $privateDir = Join-Path $temp 'private-state'
    Set-RYPrivateDirectory $privateDir
    $privateAcl = Get-Acl -LiteralPath $privateDir
    $allowed = @('S-1-5-18','S-1-5-32-544',[Security.Principal.WindowsIdentity]::GetCurrent().User.Value)
    $unexpected = @($privateAcl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]) | Where-Object {$_.AccessControlType -eq 'Allow' -and $allowed -notcontains $_.IdentityReference.Value})
    Assert-RYTest ($privateAcl.AreAccessRulesProtected -and $unexpected.Count -eq 0) 'state directory excludes inherited untrusted access'

    if ($OpenSshPayload) {
        # The real vendor sshd parses Match rules. It only reads this temporary config/key.
        $hostPath=Join-Path $temp 'fixture_host'
        $generated=Start-Process -FilePath (Join-Path $OpenSshPayload 'ssh-keygen.exe') -ArgumentList ('-q -t ed25519 -f "'+$hostPath+'" -N ""') -WindowStyle Hidden -Wait -PassThru
        Assert-RYTest ($generated.ExitCode -eq 0) 'real OpenSSH fixture host key generated'
        $configPath=Join-Path $temp 'sshd_config'
        $configText='Port 22'+"`n"+'HostKey '+$hostPath.Replace('\','/')+"`n"+'AuthorizedKeysFile __PROGRAMDATA__/ssh/administrators_authorized_keys'+"`n"+'Match LocalAddress 192.168.3.199'+"`n"+'    AuthorizedKeysFile __PROGRAMDATA__/ssh/backend_fixture_keys'+"`n"
        [IO.File]::WriteAllText($configPath,$configText,(New-Object Text.UTF8Encoding($false)))
        $command=@{exe=(Join-Path $OpenSshPayload 'sshd.exe');config_args=@('-f',$configPath)}
        $jump=Get-RYEndpointIdentity -UserName $env:USERNAME -Command $command
        Assert-RYTest $jump.shared_admin_keys 'real sshd effective administrator key file'
        Assert-RYTest ($jump.host_key -eq (Get-RYEd25519Key ([IO.File]::ReadAllText($hostPath+'.pub').Trim()))) 'real configured host public key is pinned'
        $backendIdentity=Get-RYEndpointIdentity -UserName $env:USERNAME -Command $command -SourceAddress '192.168.3.1'
        Assert-RYTest ($backendIdentity.key_path -eq (Join-Path $env:ProgramData 'ssh\backend_fixture_keys')) 'real backend Match LocalAddress selects correct authorization path'
    }

    # Existing healthy service: no service install, restart, startup change or payload copy.
    function Assert-RYAdmin {}
    function FakeSshd { $global:LASTEXITCODE=0 }
    function Get-RYSshdCommand { return @{exe='FakeSshd';config_args=@();service=@{ProcessId=42}} }
    function Get-Service {
        $svc=[pscustomobject]@{Status='Running';StartType='Automatic'}
        $svc | Add-Member -MemberType ScriptMethod -Name WaitForStatus -Value {param($status,$timeout)}
        return $svc
    }
    function Get-NetTCPConnection { return [pscustomobject]@{OwningProcess=42} }
    function Start-Service {throw 'Unexpected service start'}
    function Set-Service {throw 'Unexpected service configuration change'}
    function New-Service {throw 'Unexpected service installation'}
    function Stop-Service {throw 'Unexpected service stop'}
    $command = Ensure-RYSshd 'missing-payload-is-not-used'
    Assert-RYTest ($command.service.ProcessId -eq 42) 'healthy sshd is reused without installation or restart'

    # Unknown remote effects must stop before writing a request or launching another process.
    $unknownBackend = @{last_operation=@{receipt=(Join-Path $temp 'missing-receipt.json')};root=$temp;windows_root=$temp}
    Assert-RYThrows {Invoke-RYEndpointAction -Stage $temp -Request @{action='prepare'} -Backend $unknownBackend -TimeoutSeconds 0} 'unknown remote result blocks replay'
    $started = Join-Path $temp 'started.json'
    Write-RYJson $started @{state='started';ok=$false;process_id=123}
    Assert-RYTest ($null -eq (Read-RYOperationReceipt $started)) 'started receipt is not success'
    Write-RYJson $started @{state='failed';ok=$false;error='fixture failure'}
    Assert-RYTest (-not (Read-RYOperationReceipt $started).ok) 'failed receipt is not success'
    Write-RYJson $started @{state='failed';ok=$false;error='中文账号与目录：C:\Users\张三\.ssh'}
    $childResult = Join-Path $temp 'child-read.txt'
    $commandText = ". '" + (Join-Path $scripts 'Remote-Endpoint.ps1').Replace("'","''") + "'; `$receipt=Read-RYOperationReceipt '" + $started.Replace("'","''") + "'; [IO.File]::WriteAllText('" + $childResult.Replace("'","''") + "',`$receipt.error,[Text.Encoding]::UTF8)"
    $child = Start-Process powershell.exe -ArgumentList ('-NoProfile -NonInteractive -EncodedCommand ' + [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($commandText))) -WindowStyle Hidden -Wait -PassThru
    Assert-RYTest ($child.ExitCode -eq 0 -and [IO.File]::ReadAllText($childResult) -eq '中文账号与目录：C:\Users\张三\.ssh') 'actual PS5 child reader preserves non-BOM UTF8 username/path'

    # A slow local authorization outlives the caller timeout. Cleanup must not race it.
    $slowStage=Join-Path $temp 'slow-endpoint'
    New-Item -ItemType Directory -Path $slowStage | Out-Null
    $slowCode='param($RequestFile,$ReceiptFile); Start-Sleep -Seconds 3; [IO.File]::WriteAllText($ReceiptFile,''{"ok":true,"state":"completed"}'',[Text.Encoding]::UTF8)'
    [IO.File]::WriteAllText((Join-Path $slowStage 'Invoke-Endpoint.ps1'),$slowCode,(New-Object Text.UTF8Encoding($true)))
    Assert-RYThrows {Invoke-RYEndpointAction -Stage $slowStage -Request @{action='authorize'} -TimeoutSeconds 0} 'slow local authorization reports UNKNOWN'
    Assert-RYThrows {Invoke-RYEndpointAction -Stage $slowStage -Request @{action='cleanup'} -TimeoutSeconds 0} 'automatic cleanup does not race timed-out authorization'
    Assert-RYThrows {Invoke-RYEndpointAction -Stage $slowStage -Request @{action='cleanup'} -CleanupAfterExit -TimeoutSeconds 0} 'manual cleanup also refuses the still-running process'
    $active=Get-Content -LiteralPath (Join-Path $slowStage 'last-operation.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $slowProcess=Get-Process -Id $active.process_id -ErrorAction SilentlyContinue
    if ($slowProcess) {[void]$slowProcess.WaitForExit(10000)}
    Assert-RYTest ((Read-RYOperationReceipt (Join-Path $slowStage $active.receipt_name)).ok) 'slow endpoint eventually finishes without duplicate launch'

    # Credential retry code must not serialize its secret or mutate global auth policy.
    $remoteText = Get-Content -LiteralPath (Join-Path $scripts 'Remote-Endpoint.ps1') -Raw
    $launcher = Get-Content -LiteralPath (Join-Path $scripts 'Start-Controlled.ps1') -Raw
    Assert-RYTest ($remoteText -notmatch 'GetNetworkCredential|SecureStringToBSTR|ConvertFrom-SecureString|TrustedHosts\s*=|Enable-PSRemoting|LocalAccountTokenFilterPolicy') 'remote installation does not persist passwords or weaken global policy'
    Assert-RYTest ($launcher.Contains("if (`$choice -ine 'R')")) 'credential failure exposes explicit retry/exit decision'
    Assert-RYTest ($remoteText.Contains("OperationTimeoutSec 15")) 'remote management operations are bounded'
    [pscustomobject]@{passed=$passed.Count;tests=@($passed);scope='syntax + filesystem behavior + controlled service/receipt fixtures; no live SSH install'} | ConvertTo-Json -Depth 4
} finally {
    # Only this test-created exact temporary directory, after resolved containment check.
    $resolved=[IO.Path]::GetFullPath($temp)
    $tempRoot=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')+'\'
    if ($resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -like 'ry-ssh-script-tests-*') { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
