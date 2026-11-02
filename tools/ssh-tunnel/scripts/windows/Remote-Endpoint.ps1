# Copyright (c) 2026 Ray Zhang. Passwords stay in PSCredential and are not serialized.
. (Join-Path $PSScriptRoot 'Common.ps1')

function Connect-RYBackend {
    param([Management.Automation.PSCredential]$Credential, [string]$SessionID, [string]$Stage)
    $driveName = 'RY' + $SessionID.Substring(0, 8)
    $cim = $null
    $drive = $null
    try {
        $options = New-CimSessionOption -Protocol Dcom
        $cim = New-CimSession -ComputerName '192.168.3.199' -Credential $Credential -SessionOption $options -OperationTimeoutSec 15
        $machine = Get-CimInstance -CimSession $cim -ClassName Win32_ComputerSystem -OperationTimeoutSec 15
        $drive = New-PSDrive -Name $driveName -PSProvider FileSystem -Root '\\192.168.3.199\C$' -Credential $Credential -Scope Script
        $tcp = New-Object Net.Sockets.TcpClient
        try {
            $connect = $tcp.BeginConnect('192.168.3.199', 445, $null, $null)
            if (-not $connect.AsyncWaitHandle.WaitOne(5000)) { throw 'SMB connection timed out.' }
            $tcp.EndConnect($connect)
            $source = $tcp.Client.LocalEndPoint.Address.IPAddressToString
        } finally { $tcp.Dispose() }
        $relative = 'ProgramData\RY\SSH-Tunnel\sessions\' + $SessionID
        $root = "$driveName`:\$relative"
        if (Test-Path -LiteralPath $root) { throw 'Remote session directory already exists. Inspect its receipts before retrying.' }
        Set-RYPrivateDirectory -Path $root -AdminsOnly
        Copy-Item -LiteralPath (Join-Path $Stage 'Common.ps1'), (Join-Path $Stage 'Invoke-Endpoint.ps1'), (Join-Path $Stage 'payload') -Destination $root -Recurse
        return @{ cim = $cim; drive = $driveName; root = $root; windows_root = ('C:\' + $relative); source_address = $source; hostname = $machine.Name; last_operation = $null }
    } catch {
        if ($cim) { Remove-CimSession -CimSession $cim -ErrorAction SilentlyContinue }
        if ($drive) { Remove-PSDrive -Name $driveName -ErrorAction SilentlyContinue }
        throw
    }
}

function Disconnect-RYBackend {
    param($Backend)
    if ($Backend) {
        Remove-CimSession -CimSession $Backend.cim -ErrorAction SilentlyContinue
        Remove-PSDrive -Name $Backend.drive -ErrorAction SilentlyContinue
    }
}

function Read-RYOperationReceipt {
    param([string]$ReceiptFile)
    if (-not (Test-Path -LiteralPath $ReceiptFile)) { return $null }
    $receipt = Get-Content -LiteralPath $ReceiptFile -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($receipt.state -eq 'completed' -or $receipt.state -eq 'failed') { return $receipt }
    return $null
}

function Invoke-RYEndpointAction {
    param([string]$Stage, $Request, $Backend = $null, [int]$TimeoutSeconds = 90, [switch]$CleanupAfterExit)
    $root = if ($Backend) { $Backend.root } else { $Stage }
    $windowsRoot = if ($Backend) { $Backend.windows_root } else { $Stage }
    $operationFile = Join-Path $root 'last-operation.json'
    if (Test-Path -LiteralPath $operationFile) {
        $last = Get-Content -LiteralPath $operationFile -Raw -Encoding UTF8 | ConvertFrom-Json
        $previous = Read-RYOperationReceipt (Join-Path $root $last.receipt_name)
        if (-not $previous) {
            if (-not $CleanupAfterExit -or $Request.action -ne 'cleanup' -or -not $last.process_id) {
                throw ('Previous endpoint operation is UNKNOWN. No concurrent operation or cleanup was launched. Inspect ' + $operationFile)
            }
            if ($Backend) {
                $running = Get-CimInstance -CimSession $Backend.cim -ClassName Win32_Process -Filter ('ProcessId=' + [int]$last.process_id) -OperationTimeoutSec 15
                if ($running) { throw 'Previous remote process may still be running; cleanup refused.' }
            } else {
                $running = Get-Process -Id $last.process_id -ErrorAction SilentlyContinue
                if ($running -and $running.StartTime.ToUniversalTime().ToString('o') -eq $last.process_start_time) { throw 'Previous local process is still running; cleanup refused.' }
            }
        }
    }
    # Operation files are never reused. An unknown earlier result blocks replay.
    if ($Backend -and $Backend.last_operation -and -not $CleanupAfterExit) {
        $previous = Read-RYOperationReceipt $Backend.last_operation.receipt
        if (-not $previous) { throw 'Previous remote operation is UNKNOWN. Inspect its receipt/process before another operation; automatic replay was refused.' }
    }
    $operationID = [guid]::NewGuid().ToString('N')
    $requestName = "request-$operationID.json"
    $receiptName = "receipt-$operationID.json"
    $requestPath = Join-Path $root $requestName
    $receiptPath = Join-Path $root $receiptName
    Write-RYJson $requestPath $Request
    $scriptFile = Join-Path $windowsRoot 'Invoke-Endpoint.ps1'
    $remoteRequest = Join-Path $windowsRoot $requestName
    $remoteReceipt = Join-Path $windowsRoot $receiptName
    if (($scriptFile + $remoteRequest + $remoteReceipt) -match '"') { throw 'Unsupported quote in installation path.' }
    $arguments = '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $scriptFile + '" -RequestFile "' + $remoteRequest + '" -ReceiptFile "' + $remoteReceipt + '"'
    $process = $null
    $remotePID = $null
    $operation = @{ operation_id = $operationID; receipt_name = $receiptName; process_id = $null; process_start_time = $null; action = $Request.action }
    Write-RYJson $operationFile $operation
    if ($Backend) {
        $Backend.last_operation = @{ receipt = $receiptPath; process_id = $null; operation_id = $operationID }
        $launch = Invoke-CimMethod -CimSession $Backend.cim -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = ('powershell.exe ' + $arguments) } -OperationTimeoutSec 15
        if ($launch.ReturnValue -ne 0) { $Backend.last_operation = $null; throw ('Remote process was rejected, status ' + $launch.ReturnValue + '. No successful launch was reported.') }
        $remotePID = $launch.ProcessId
        $operation.process_id = $remotePID
        Write-RYJson $operationFile $operation
        $Backend.last_operation.process_id = $remotePID
    } else {
        $process = Start-Process -FilePath powershell.exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root "stdout-$operationID.log") -RedirectStandardError (Join-Path $root "stderr-$operationID.log")
        $operation.process_id = $process.Id
        $operation.process_start_time = $process.StartTime.ToUniversalTime().ToString('o')
        Write-RYJson $operationFile $operation
    }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $receipt = Read-RYOperationReceipt $receiptPath
        if ($receipt) {
            if (-not $receipt.ok) { throw ('Endpoint operation failed: ' + $receipt.error + ' Receipt: ' + $receiptPath) }
            return $receipt
        }
        if ($process -and $process.HasExited) { break }
        Start-Sleep -Milliseconds 250
    }
    # Check once more after the deadline; never relaunch an operation after unknown effects.
    $receipt = Read-RYOperationReceipt $receiptPath
    if ($receipt) {
        if (-not $receipt.ok) { throw ('Endpoint operation failed: ' + $receipt.error + ' Receipt: ' + $receiptPath) }
        return $receipt
    }
    throw ('Endpoint operation result is UNKNOWN; it was not replayed. Inspect ' + $receiptPath + ' and process ' + $(if ($process) {$process.Id} else {$remotePID}) + ' before retrying.')
}
