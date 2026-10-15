param([Parameter(Mandatory=$true)][int]$RuntimePid,[Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
if(-not (Test-Path -LiteralPath (Join-Path $OutputDirectory 'guest-network.json'))) {
    $profiles=@(Get-NetConnectionProfile -ErrorAction SilentlyContinue|Select-Object NetworkCategory,IPv4Connectivity)
    $rules=@(Get-NetFirewallRule -ErrorAction SilentlyContinue|Where-Object {$_.DisplayName -like '*Mendix*'}|Select-Object DisplayName,Enabled,Direction,Action,Profile)
    @{profiles=$profiles;mendixRules=$rules}|ConvertTo-Json -Depth 6|Set-Content -Encoding UTF8 -LiteralPath (Join-Path $OutputDirectory 'guest-network.json')
}
# 앱 after-startup action이 비동기로 실행하는 일회성 검증 소비자다.
# command 파일은 trial 제어에만 쓰며 SQL/중계 바이트를 운반하지 않는다.
$until=[DateTime]::UtcNow.AddMinutes(5)
$request=Join-Path $OutputDirectory 'guest-command.json'
while([DateTime]::UtcNow -lt $until) {
    if(Test-Path -LiteralPath $request) {
        $packet=Get-Content -Raw -LiteralPath $request|ConvertFrom-Json
        Remove-Item -LiteralPath $request
        if($packet.id -cmatch '^[a-z0-9-]{1,50}$' -and $packet.action -ceq 'workload') {
            $guestPort=if($null -eq $packet.guestPort){3306}else{[int]$packet.guestPort}
            if($guestPort -lt 1 -or $guestPort -gt 65535){throw 'invalid fixture guest port'}
            & (Join-Path $PSScriptRoot 'guest-workload.ps1') -OutputDirectory $OutputDirectory -TrialId $packet.id -RuntimePid $RuntimePid -GuestPort $guestPort
            $proofPath=Join-Path $OutputDirectory 'guest-helper-proof.json'
            if(-not (Test-Path -LiteralPath $proofPath)) {
                try {
                    $proof=(& (Join-Path $PSScriptRoot '../../test-host-service.ps1')|Out-String).Trim()
                    @{ok=$true;proof=$proof}|ConvertTo-Json|Set-Content -Encoding UTF8 -LiteralPath $proofPath
                }catch{
                    @{ok=$false;reason='production Windows helper contract failed';detail=$_.Exception.Message}|ConvertTo-Json|Set-Content -Encoding UTF8 -LiteralPath $proofPath
                }
            }
        }
        break
    }
    Start-Sleep -Milliseconds 250
}
