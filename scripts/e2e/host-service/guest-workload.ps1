param(
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-z0-9-]{1,50}$')][string]$TrialId,
    [Parameter(Mandatory=$true)][int]$RuntimePid,
    [ValidateRange(1,65535)][int]$GuestPort=3306
)
$ErrorActionPreference='Stop'
$output=Join-Path $OutputDirectory ($TrialId+'.json')
try {
    Add-Type -AssemblyName System.Net.Http
    if(-not ('RelayWorkload' -as [type])) {
        Add-Type -TypeDefinition ([IO.File]::ReadAllText((Join-Path $PSScriptRoot 'workload.cs'))) -ReferencedAssemblies 'System.dll','System.Core.dll','System.Net.Http.dll' -Language CSharp
    }
    # 매 trial의 수용 검사 직전에도 같은 10,000행을 준비한다. 첫 trial만
    # 빈 테이블에서 시작하면 삭제 객체와 JVM GC 상태가 달라지는 것을 막는다.
    if($TrialId -cnotlike 'acceptance*') {$null=[RelayWorkload]::Reset().GetAwaiter().GetResult()}
    $proof=[RelayWorkload]::Acceptance($GuestPort).GetAwaiter().GetResult()
    $owner=@(Get-NetTCPConnection -State Listen -LocalPort $GuestPort -ErrorAction Stop|Select-Object -ExpandProperty OwningProcess -Unique)
    if($owner.Count -ne 1){throw 'ambiguous fixture DB listener'}
    $report=[ordered]@{ok=$true;acceptance=$proof;runtimePid=$RuntimePid;boundary='Windows loopback HTTP -> exact Studio Run Locally -> managed TLS relay -> Linux loopback MySQL'}
    if($TrialId -cnotlike 'acceptance*') {
        # 기준선과 후보 모두 동일하게 10,000개의 1 KiB 행을 재생성한다.
        $report.seed=[RelayWorkload]::Reset().GetAwaiter().GetResult()
        $null=[RelayWorkload]::Run(200,0,0).GetAwaiter().GetResult()
        $report.startedAt=[DateTimeOffset]::UtcNow.ToString('o')
        # JVM GC 한 번의 시점 대신 전체 측정 구간의 100ms 표본을 보관한다.
        $report.workload=[RelayWorkload]::Run(1000,$RuntimePid,[int]$owner[0]).GetAwaiter().GetResult()
        $report.finishedAt=[DateTimeOffset]::UtcNow.ToString('o')
    }
    $report.runtimeWorkingSetBytes=(Get-Process -Id $RuntimePid).WorkingSet64
    $report.driverWorkingSetBytes=(Get-Process -Id $PID).WorkingSet64
    $report.guestRelayWorkingSetBytes=(Get-Process -Id $owner[0]).WorkingSet64
    $report|ConvertTo-Json -Depth 12|Set-Content -Encoding UTF8 -LiteralPath $output
}catch{
    # 이 fixture에는 자격증명이 없다. 응답/예외 원문은 증거 파일에 복사하지 않는다.
    $stage=if('RelayWorkload' -as [type]){[RelayWorkload]::Stage}else{'compile guest workload'}
    @{ok=$false;reason='app acceptance or workload assertion failed';stage=$stage}|ConvertTo-Json|Set-Content -Encoding UTF8 -LiteralPath $output
}
