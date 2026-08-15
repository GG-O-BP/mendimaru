$mendimaruRequestId = $env:MENDIMARU_REQUEST_ID
$mendimaruNonce = $env:MENDIMARU_OPERATION_NONCE
$mendimaruKeyText = $env:MENDIMARU_OPERATION_KEY
Remove-Item Env:MENDIMARU_REQUEST_ID -ErrorAction SilentlyContinue
Remove-Item Env:MENDIMARU_OPERATION_NONCE -ErrorAction SilentlyContinue
Remove-Item Env:MENDIMARU_OPERATION_KEY -ErrorAction SilentlyContinue

if ($mendimaruRequestId -notmatch '^[0-9a-f]{32}$' -or
    $mendimaruNonce -notmatch '^[0-9a-f]{32}$') {
    throw 'MENDIMARU_OPERATION_IDENTITY_INVALID'
}
try {
    $mendimaruKey = [Convert]::FromBase64String($mendimaruKeyText)
} catch {
    throw 'MENDIMARU_OPERATION_KEY_INVALID'
}
if ($mendimaruKey.Length -ne 32) {
    throw 'MENDIMARU_OPERATION_KEY_INVALID'
}
$mendimaruKeyText = $null
$script:MendimaruReportSequence = 0
$script:MendimaruHmac = New-Object Security.Cryptography.HMACSHA256
$script:MendimaruHmac.Key = $mendimaruKey

function Write-MendimaruReport {
    param([System.Collections.IDictionary]$Payload)

    $script:MendimaruReportSequence++
    $payloadJson = $Payload | ConvertTo-Json -Compress -Depth 8
    $payloadBytes = [Text.Encoding]::UTF8.GetBytes($payloadJson)
    $payloadBase64 = [Convert]::ToBase64String($payloadBytes)
    $authenticatedText = "{0}`n{1}`n{2}`n{3}" -f @(
        $mendimaruRequestId,
        $mendimaruNonce,
        $script:MendimaruReportSequence,
        $payloadBase64
    )
    $macBytes = $script:MendimaruHmac.ComputeHash(
        [Text.Encoding]::UTF8.GetBytes($authenticatedText)
    )
    $mac = -join @($macBytes | ForEach-Object { $_.ToString('x2') })
    $envelope = [ordered]@{
        schemaVersion = 1
        requestId = $mendimaruRequestId
        nonce = $mendimaruNonce
        sequence = $script:MendimaruReportSequence
        payload = $payloadBase64
        mac = $mac
    }
    $serialized = $envelope | ConvertTo-Json -Compress
    $temporaryPath = "$resultPath.tmp"
    $encoding = New-Object Text.UTF8Encoding($false)
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        try {
            [IO.File]::WriteAllText($temporaryPath, $serialized, $encoding)
            Move-Item -LiteralPath $temporaryPath -Destination $resultPath -Force
            return
        } catch {
            Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
            if ($attempt -eq 19) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

function Read-MendimaruAuthenticatedPayload {
    param([string]$Path)

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if (-not $item.PSIsContainer -and
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0 -and
        $item.Length -le 16384) {
        $serialized = [IO.File]::ReadAllText($item.FullName)
    } else {
        throw 'MENDIMARU_CONTROL_FILE_INVALID'
    }
    try {
        $envelope = $serialized | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw 'MENDIMARU_CONTROL_ENVELOPE_INVALID'
    }
    $propertyNames = @($envelope.PSObject.Properties.Name | Sort-Object)
    if (($propertyNames -join ',') -cne 'mac,nonce,payload,requestId,schemaVersion,sequence' -or
        [int]$envelope.schemaVersion -ne 1 -or
        [string]$envelope.requestId -cne $mendimaruRequestId -or
        [string]$envelope.nonce -cne $mendimaruNonce) {
        throw 'MENDIMARU_CONTROL_IDENTITY_INVALID'
    }
    try {
        $sequence = [long]$envelope.sequence
        $payloadBytes = [Convert]::FromBase64String([string]$envelope.payload)
    } catch {
        throw 'MENDIMARU_CONTROL_PAYLOAD_INVALID'
    }
    if ($sequence -le 0 -or $payloadBytes.Length -gt 8192 -or
        [string]$envelope.mac -cnotmatch '^[0-9a-f]{64}$') {
        throw 'MENDIMARU_CONTROL_PAYLOAD_INVALID'
    }
    $authenticatedText = "{0}`n{1}`n{2}`n{3}" -f @(
        $mendimaruRequestId,
        $mendimaruNonce,
        $sequence,
        [string]$envelope.payload
    )
    $expectedMac = $script:MendimaruHmac.ComputeHash(
        [Text.Encoding]::UTF8.GetBytes($authenticatedText)
    )
    $providedMac = New-Object byte[] 32
    for ($index = 0; $index -lt $providedMac.Length; $index++) {
        $providedMac[$index] = [Convert]::ToByte(
            ([string]$envelope.mac).Substring($index * 2, 2),
            16
        )
    }
    $difference = 0
    for ($index = 0; $index -lt $expectedMac.Length; $index++) {
        $difference = $difference -bor ($expectedMac[$index] -bxor $providedMac[$index])
    }
    if ($difference -ne 0) {
        throw 'MENDIMARU_CONTROL_AUTHENTICATION_FAILED'
    }
    try {
        $utf8 = New-Object Text.UTF8Encoding($false, $true)
        $payloadJson = $utf8.GetString($payloadBytes)
    } catch {
        throw 'MENDIMARU_CONTROL_PAYLOAD_INVALID'
    }
    return [pscustomobject]@{
        Sequence = $sequence
        Json = $payloadJson
    }
}

function Read-MendimaruStudioStopRequest {
    param(
        [string]$Path,
        [string]$ExpectedSessionId,
        [int]$ExpectedProcessId,
        [long]$ExpectedStartedTicks,
        [long]$PreviousSequence
    )

    $authenticated = Read-MendimaruAuthenticatedPayload -Path $Path
    if ($authenticated.Sequence -le $PreviousSequence) {
        throw 'MENDIMARU_CONTROL_REPLAY'
    }
    try {
        $request = $authenticated.Json | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw 'MENDIMARU_CONTROL_PAYLOAD_INVALID'
    }
    $propertyNames = @($request.PSObject.Properties.Name | Sort-Object)
    if (($propertyNames -join ',') -cne 'action,processId,sessionId,startedTicks' -or
        [string]$request.action -cne 'studio.stop' -or
        [string]$request.sessionId -cne $ExpectedSessionId -or
        [int]$request.processId -ne $ExpectedProcessId -or
        [long]$request.startedTicks -ne $ExpectedStartedTicks) {
        throw 'MENDIMARU_CONTROL_TARGET_INVALID'
    }
    return $authenticated.Sequence
}

function Assert-MendimaruDirectPath {
    param(
        [string]$Path,
        [string]$Root,
        [switch]$Leaf
    )

    if ([string]::IsNullOrWhiteSpace($Path) -or [string]::IsNullOrWhiteSpace($Root)) {
        throw 'MENDIMARU_PATH_INVALID'
    }
    if ($Leaf -and -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "MENDIMARU_EXECUTABLE_NOT_FOUND:$Path"
    }
    if (-not $Leaf -and -not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "MENDIMARU_DIRECTORY_NOT_FOUND:$Path"
    }
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $pathFull = [IO.Path]::GetFullPath($Path)
    $rootPrefix = "$rootFull\"
    if (-not $pathFull.Equals($rootFull, [StringComparison]::OrdinalIgnoreCase) -and
        -not $pathFull.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "MENDIMARU_PATH_OUTSIDE_TRUST_ROOT:$pathFull"
    }

    $item = Get-Item -LiteralPath $pathFull -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "MENDIMARU_REPARSE_POINT:$($item.FullName)"
    }
    $current = if ($Leaf) { $item.Directory } else { $item }
    while ($null -ne $current) {
        if (($current.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "MENDIMARU_REPARSE_POINT:$($current.FullName)"
        }
        if ($current.FullName.TrimEnd('\').Equals(
                $rootFull,
                [StringComparison]::OrdinalIgnoreCase
            )) {
            return $pathFull
        }
        $current = $current.Parent
    }
    throw "MENDIMARU_PATH_OUTSIDE_TRUST_ROOT:$pathFull"
}

function Get-MendimaruSha256 {
    param([string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function New-MendimaruDirectDirectory {
    param(
        [string]$Path,
        [string]$Root
    )

    if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
        throw "MENDIMARU_DIRECTORY_NOT_FOUND:$Root"
    }
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $pathFull = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    $rootPrefix = "$rootFull\"
    if (-not $pathFull.Equals($rootFull, [StringComparison]::OrdinalIgnoreCase) -and
        -not $pathFull.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "MENDIMARU_PATH_OUTSIDE_TRUST_ROOT:$pathFull"
    }
    $rootItem = Get-Item -LiteralPath $rootFull -Force
    if (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "MENDIMARU_REPARSE_POINT:$($rootItem.FullName)"
    }
    if ($pathFull.Equals($rootFull, [StringComparison]::OrdinalIgnoreCase)) {
        return $rootFull
    }

    $currentPath = $rootFull
    $relative = $pathFull.Substring($rootPrefix.Length)
    foreach ($segment in @($relative -split '\\' | Where-Object { $_.Length -gt 0 })) {
        $currentPath = Join-Path $currentPath $segment
        if (Test-Path -LiteralPath $currentPath) {
            $currentItem = Get-Item -LiteralPath $currentPath -Force
            if (-not $currentItem.PSIsContainer -or
                ($currentItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "MENDIMARU_REPARSE_POINT:$($currentItem.FullName)"
            }
        } else {
            New-Item -ItemType Directory -Path $currentPath | Out-Null
        }
    }
    return Assert-MendimaruDirectPath -Path $pathFull -Root $rootFull
}

function Assert-MendimaruTrustedExecutable {
    param(
        [string]$Path,
        [string]$Root,
        [string]$ExpectedSha256 = ''
    )

    $trustedPath = Assert-MendimaruDirectPath -Path $Path -Root $Root -Leaf
    if (-not [IO.Path]::GetExtension($trustedPath).Equals(
            '.exe',
            [StringComparison]::OrdinalIgnoreCase
        )) {
        throw "MENDIMARU_EXECUTABLE_INVALID:$trustedPath"
    }
    $before = Get-MendimaruSha256 $trustedPath
    if (-not [string]::IsNullOrWhiteSpace($ExpectedSha256) -and
        $before -ne $ExpectedSha256.ToLowerInvariant()) {
        throw "MENDIMARU_HASH_MISMATCH:$trustedPath"
    }

    $securityModule = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1'
    Import-Module -Name $securityModule -Force -ErrorAction Stop
    $signature = Get-AuthenticodeSignature -LiteralPath $trustedPath
    if ($signature.Status.ToString() -cne 'Valid') {
        throw "MENDIMARU_SIGNATURE_INVALID:$($signature.Status)"
    }
    $subject = if ($null -eq $signature.SignerCertificate) {
        ''
    } else {
        $signature.SignerCertificate.Subject
    }
    $trustedPublisher = @($subject -split ',' | ForEach-Object {
        $_.Trim().ToLowerInvariant()
    } | Where-Object {
        $_ -in @(
            'cn=mendix technology b.v.',
            'o=mendix technology b.v.',
            'cn=siemens ag',
            'o=siemens ag'
        )
    }).Count -gt 0
    if (-not $trustedPublisher) {
        throw "MENDIMARU_PUBLISHER_INVALID:$subject"
    }
    $after = Get-MendimaruSha256 $trustedPath
    if ($before -ne $after) {
        throw "MENDIMARU_EXECUTABLE_CHANGED:$trustedPath"
    }
    return $after
}
