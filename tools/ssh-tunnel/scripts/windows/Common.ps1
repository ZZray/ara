# Copyright (c) 2026 Ray Zhang. Windows PowerShell 5.1.
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Write-RYJson {
    param([string]$Path, $Value)
    $Path = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
    $tmp = "$Path.$([guid]::NewGuid().ToString('N')).tmp"
    [IO.File]::WriteAllText($tmp, ($Value | ConvertTo-Json -Depth 12), (New-Object Text.UTF8Encoding($false)))
    if (Test-Path -LiteralPath $Path) { [IO.File]::Replace($tmp, $Path, [NullString]::Value) }
    else { [IO.File]::Move($tmp, $Path) }
}

function Assert-RYAdmin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not (New-Object Security.Principal.WindowsPrincipal($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Administrator privileges are required on the controlled endpoint.'
    }
}

function Set-RYPrivateDirectory {
    param([string]$Path, [switch]$AdminsOnly)
    $cursor = $Path
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'State directory contains a reparse point; installation stopped.' }
        }
        $next = Split-Path -Parent $cursor
        if ($next -eq $cursor) { break }
        $cursor = $next
    }
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    $sids = @('S-1-5-18', 'S-1-5-32-544')
    if (-not $AdminsOnly) { $sids += [Security.Principal.WindowsIdentity]::GetCurrent().User.Value }
    foreach ($sid in $sids | Select-Object -Unique) {
        $rule = New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sid)), 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}

function Get-RYCloudURL {
    param([string]$InputValue)
    if ([string]::IsNullOrWhiteSpace($InputValue)) { return 'https://nmrkit.ciqtek.com/ssh-tunnel' }
    $uri = $null
    if (-not [Uri]::TryCreate($InputValue.Trim(), [UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -ne 'https' -or $uri.UserInfo -or $uri.Query -or $uri.Fragment) {
        throw 'Cloud address must be an HTTPS URL without credentials, query, or fragment.'
    }
    return $uri.AbsoluteUri.TrimEnd('/')
}

function Test-RYTape {
    param([byte[]]$Bytes, [int]$Offset, [byte[]]$Expected)
    if ($Offset -lt 0 -or $Offset + $Expected.Length -gt $Bytes.Length) { return $false }
    for ($i = 0; $i -lt $Expected.Length; $i++) { if ($Bytes[$Offset + $i] -ne $Expected[$i]) { return $false } }
    return $true
}

function Get-RYEd25519Key {
    param([string]$Text)
    $parts = $Text.Trim() -split '\s+'
    if ($parts.Count -lt 2 -or $parts.Count -gt 3 -or $parts[0] -ne 'ssh-ed25519') { throw 'Expected one Ed25519 public key without options.' }
    if ($Text -match '[\r\n]') { throw 'Public key must be a single line.' }
    try { $blob = [Convert]::FromBase64String($parts[1]) } catch { throw 'Invalid public key encoding.' }
    $prefix = [byte[]](0,0,0,11,115,115,104,45,101,100,50,53,53,49,57,0,0,0,32)
    if ($blob.Length -ne 51 -or -not (Test-RYTape $blob 0 $prefix)) { throw 'Invalid Ed25519 public key structure.' }
    return 'ssh-ed25519 ' + [Convert]::ToBase64String($blob)
}

function Get-RYHashBytes {
    param([byte[]]$Bytes)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '') }
    finally { $sha.Dispose() }
}

function Add-RYSessionKey {
    param([string]$Path, [string]$PublicKey, [string]$SessionID, [string]$IntentFile)
    if ($SessionID -notmatch '^[a-f0-9]{32}$') { throw 'Invalid session identifier.' }
    $key = Get-RYEd25519Key $PublicKey
    $old = [byte[]]@()
    if (Test-Path -LiteralPath $Path) { $old = [IO.File]::ReadAllBytes($Path) }
    # Strict UTF-8 detects unsupported files without rewriting their bytes or options.
    $text = (New-Object Text.UTF8Encoding($false, $true)).GetString($old)
    $line = "$key ry-session:$SessionID"
    if ($text -match ('(?m)^' + [regex]::Escape($line) + '\r?$')) { throw 'This session key is already present; inspect the earlier receipt before retrying.' }
    $separator = ''
    if ($old.Length -gt 0 -and $old[$old.Length - 1] -ne 10) { $separator = "`r`n" }
    $addition = [Text.Encoding]::ASCII.GetBytes($separator + $line + "`r`n")
    $intent = @{ path = $Path; line = $line; prefix_length = $old.Length; prefix_sha256 = Get-RYHashBytes $old; added_separator = $separator }
    if ($IntentFile) { Write-RYJson $IntentFile $intent }
    $stream = [IO.File]::Open($Path, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        if ($stream.Length -ne $old.Length) { throw 'authorized_keys changed concurrently; no key was appended.' }
        $current = New-Object byte[] $old.Length
        [void]$stream.Read($current, 0, $current.Length)
        if ((Get-RYHashBytes $current) -ne (Get-RYHashBytes $old)) { throw 'authorized_keys changed concurrently; no key was appended.' }
        $stream.Position = $stream.Length
        $stream.Write($addition, 0, $addition.Length)
        $stream.Flush()
    } finally { $stream.Dispose() }
    return $intent
}

function Remove-RYSessionKey {
    param($Receipt)
    if (-not (Test-Path -LiteralPath $Receipt.path)) { return }
    $stream = [IO.File]::Open($Receipt.path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $bytes = New-Object byte[] $stream.Length
        [void]$stream.Read($bytes, 0, $bytes.Length)
        $addition = [Text.Encoding]::ASCII.GetBytes($Receipt.added_separator + $Receipt.line + "`r`n")
        $offset = [int]$Receipt.prefix_length
        $prefix = New-Object byte[] ([Math]::Min($offset, $bytes.Length))
        [Array]::Copy($bytes, $prefix, $prefix.Length)
        $length = $addition.Length
        if ((Get-RYHashBytes $prefix) -ne $Receipt.prefix_sha256 -or -not (Test-RYTape $bytes $offset $addition)) {
            # Another administrator edited the file: remove only our exact marked line.
            $text = (New-Object Text.UTF8Encoding($false, $true)).GetString($bytes)
            $match = [regex]::Match($text, '(?m)^' + [regex]::Escape($Receipt.line) + '(?:\r?\n|$)')
            if (-not $match.Success) {
                if ($text.Contains($Receipt.line.Split(' ')[-1])) { throw 'Session key was edited; automatic removal was refused. Inspect the cleanup receipt.' }
                return
            }
            $offset = [Text.Encoding]::UTF8.GetByteCount($text.Substring(0, $match.Index))
            $length = [Text.Encoding]::UTF8.GetByteCount($match.Value)
        }
        $remaining = New-Object byte[] ($bytes.Length - $length)
        [Array]::Copy($bytes, 0, $remaining, 0, $offset)
        [Array]::Copy($bytes, $offset + $length, $remaining, $offset, $bytes.Length - $offset - $length)
        $stream.Position = 0
        $stream.Write($remaining, 0, $remaining.Length)
        $stream.SetLength($remaining.Length)
        $stream.Flush()
    } finally { $stream.Dispose() }
}

function Get-RYSshdCommand {
    $svc = Get-CimInstance -ClassName Win32_Service -Filter "Name='sshd'"
    if (-not $svc) { return $null }
    $image = $svc.PathName.Trim()
    if ($image -match '^"([^"]+\.exe)"(.*)$') { $exe = $Matches[1]; $tail = $Matches[2].Trim() }
    elseif ($image -match '^(.+?\.exe)(.*)$') { $exe = $Matches[1]; $tail = $Matches[2].Trim() }
    else { throw 'Cannot determine the existing sshd executable path.' }
    $configArgs = @()
    if ($tail) {
        if ($tail -match '^-f\s+"([^"]+)"$' -or $tail -match '^-f\s+([^\s"]+)$') { $configArgs = @('-f', $Matches[1]) }
        else { throw 'Existing sshd uses unsupported service arguments; preserve it and configure authorization manually.' }
    }
    return @{ exe = $exe; config_args = $configArgs; service = $svc }
}

function Ensure-RYSshd {
    param([string]$Payload)
    Assert-RYAdmin
    $command = Get-RYSshdCommand
    if (-not $command) {
        if (-not (Test-Path -LiteralPath (Join-Path $Payload 'sshd.exe'))) { throw 'Offline OpenSSH payload is missing.' }
        $destination = Join-Path $env:ProgramFiles 'RY\OpenSSH-10.0.0.0p2'
        if (Test-Path -LiteralPath $destination) {
            foreach ($file in Get-ChildItem -LiteralPath $Payload -File) {
                $targetFile = Join-Path $destination $file.Name
                if (-not (Test-Path -LiteralPath $targetFile) -or (Get-FileHash -LiteralPath $file.FullName).Hash -ne (Get-FileHash -LiteralPath $targetFile).Hash) {
                    throw 'Existing RY OpenSSH installation differs from this payload. Inspect it before retrying.'
                }
            }
        } else {
            New-Item -ItemType Directory -Path $destination -Force | Out-Null
            Copy-Item -Path (Join-Path $Payload '*') -Destination $destination -Recurse
        }
        $sshData = Join-Path $env:ProgramData 'ssh'
        if (-not (Test-Path -LiteralPath $sshData)) { Set-RYPrivateDirectory $sshData }
        $config = Join-Path $sshData 'sshd_config'
        if (-not (Test-Path -LiteralPath $config)) { Copy-Item -LiteralPath (Join-Path $destination 'sshd_config_default') -Destination $config }
        & (Join-Path $destination 'ssh-keygen.exe') -A | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'OpenSSH host key generation failed.' }
        $exe = Join-Path $destination 'sshd.exe'
        & $exe -t | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'OpenSSH configuration check failed.' }
        # Register only sshd; the vendor installer also deletes/replaces ssh-agent.
        New-Service -Name sshd -DisplayName 'OpenSSH SSH Server' -BinaryPathName ('"' + $exe + '"') -StartupType Automatic | Out-Null
        & sc.exe privs sshd 'SeAssignPrimaryTokenPrivilege/SeTcbPrivilege/SeBackupPrivilege/SeRestorePrivilege/SeImpersonatePrivilege' | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Failed to set sshd service privileges. Inspect the new service before retrying.' }
        $command = Get-RYSshdCommand
    }
    $configArgs = $command.config_args
    & $command.exe -t @configArgs | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Existing sshd configuration is invalid; it was not replaced.' }
    $svc = Get-Service -Name sshd
    if ($svc.Status -ne 'Running') { Start-Service -Name sshd }
    if ($svc.StartType -ne 'Automatic') { Set-Service -Name sshd -StartupType Automatic }
    $svc.WaitForStatus('Running', [TimeSpan]::FromSeconds(15))
    $command = Get-RYSshdCommand
    $listeners = @(Get-NetTCPConnection -State Listen -LocalPort 22 -ErrorAction SilentlyContinue | Where-Object { $_.OwningProcess -eq $command.service.ProcessId })
    if ($listeners.Count -eq 0) { throw 'sshd is not listening on port 22. Existing configuration was preserved.' }
    return $command
}

function Get-RYEndpointIdentity {
    param([string]$UserName, $Command, [string]$SourceAddress = '127.0.0.1')
    if ([string]::IsNullOrWhiteSpace($UserName) -or $UserName.Length -gt 128 -or $UserName -match '[,=\x00-\x1f"]') { throw 'Invalid SSH login user.' }
    $account = New-Object Security.Principal.NTAccount($UserName)
    $sid = $account.Translate([Security.Principal.SecurityIdentifier]).Value
    $profile = Get-ItemProperty -LiteralPath "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$sid" -ErrorAction SilentlyContinue
    $homePath = if ($profile) { [Environment]::ExpandEnvironmentVariables($profile.ProfileImagePath) } else { '' }
    $configArgs = $Command.config_args
    $localAddress = if ($SourceAddress -eq '127.0.0.1') { '127.0.0.1' } else { '192.168.3.199' }
    $settings = @(& $Command.exe -T @configArgs -C "user=$($UserName.ToLowerInvariant()),host=$SourceAddress,addr=$SourceAddress,laddr=$localAddress,lport=22" 2>$null)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot evaluate the existing sshd configuration for this login user.' }
    if ($settings -contains 'usedns yes') { throw 'UseDNS-dependent SSH matching requires manual authorization; the existing configuration was preserved.' }
    $keysLine = @($settings | Where-Object { $_ -like 'authorizedkeysfile *' })
    if ($keysLine.Count -ne 1) { throw 'Cannot determine AuthorizedKeysFile.' }
    $paths = @([regex]::Matches($keysLine[0].Substring(19), '"([^"]+)"|(\S+)') | ForEach-Object { if ($_.Groups[1].Success) { $_.Groups[1].Value } else { $_.Groups[2].Value } })
    if ($paths.Count -ne 1 -or $paths[0] -eq 'none') { throw 'Multiple/disabled AuthorizedKeysFile is not modified automatically.' }
    if ($paths[0].Contains('%h') -and -not $homePath) { throw 'Login user has no initialized Windows profile for AuthorizedKeysFile.' }
    $keyPath = $paths[0].Replace('__PROGRAMDATA__', $env:ProgramData).Replace('%u', $UserName).Replace('%h', $homePath).Replace('/', '\')
    if ($keyPath.Contains('%')) { throw 'Unsupported AuthorizedKeysFile expansion; manual authorization is required.' }
    if (-not [IO.Path]::IsPathRooted($keyPath)) {
        if (-not $homePath) { throw 'Login user has no initialized Windows profile; sign in locally once or choose an administrator account.' }
        $keyPath = Join-Path $homePath $keyPath
    }
    $keyPath = [IO.Path]::GetFullPath($keyPath)
    $hostFiles = @($settings | Where-Object { $_ -like 'hostkey *' } | ForEach-Object { $_.Substring(8).Trim('"').Replace('__PROGRAMDATA__', $env:ProgramData).Replace('/', '\') })
    $hostKey = $null
    foreach ($hostFile in $hostFiles) {
        if (Test-Path -LiteralPath ($hostFile + '.pub')) {
            $candidate = [IO.File]::ReadAllText($hostFile + '.pub').Trim()
            if ($candidate -like 'ssh-ed25519 *') { $hostKey = Get-RYEd25519Key $candidate; break }
        }
    }
    if (-not $hostKey) { throw 'No configured Ed25519 host public key file found. Existing host keys were preserved.' }
    return @{ user = $UserName; sid = $sid; key_path = $keyPath; host_key = $hostKey; shared_admin_keys = ($keyPath -ieq (Join-Path $env:ProgramData 'ssh\administrators_authorized_keys')) }
}

function Initialize-RYKeyFile {
    param($Identity)
    $path = $Identity.key_path
    $parent = Split-Path -Parent $path
    # Refuse redirects before elevated writes; do not alter existing ACLs.
    $cursor = $path
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Authorization path contains a reparse point; manual review required.' }
        }
        $next = Split-Path -Parent $cursor
        if ($next -eq $cursor) { break }
        $cursor = $next
    }
    $allowed = @('S-1-5-18', 'S-1-5-32-544')
    if (-not $Identity.shared_admin_keys) { $allowed += $Identity.sid }
    foreach ($entry in @(@{path=$parent;directory=$true}, @{path=$path;directory=$false})) {
        if (-not (Test-Path -LiteralPath $entry.path)) {
            if ($entry.directory) { New-Item -ItemType Directory -Path $entry.path -Force | Out-Null }
            else { [IO.File]::WriteAllBytes($entry.path, [byte[]]@()) }
            $acl = if ($entry.directory) { New-Object Security.AccessControl.DirectorySecurity } else { New-Object Security.AccessControl.FileSecurity }
            $acl.SetAccessRuleProtection($true, $false)
            $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
            foreach ($sid in $allowed) {
                $rule = if ($entry.directory) { New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sid)), 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow') }
                        else { New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sid)), 'FullControl', 'Allow') }
                $acl.AddAccessRule($rule)
            }
            Set-Acl -LiteralPath $entry.path -AclObject $acl
        }
    }
    $fileAcl = Get-Acl -LiteralPath $path
    foreach ($rule in $fileAcl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq 'Allow' -and $allowed -notcontains $rule.IdentityReference.Value) { throw 'Existing authorized_keys has broader ACLs; preserved for manual review.' }
    }
}
