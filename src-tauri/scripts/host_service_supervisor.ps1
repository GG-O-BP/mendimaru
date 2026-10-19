# Studio monitor의 인증된 채널로 시작한다. 별도 RDP 연결은 기존 RemoteApp을
# 끊을 수 있으므로 연결된 Studio에서는 소켓 작업을 이 프로세스가 소유한다.
$script:HostService = $null
$script:HostServiceTask = $null
$script:HostServiceSequence = [long]0
function Close-MendimaruHostService {
    if($null -ne $script:HostService){$script:HostService.Dispose();$script:HostService=$null}
    $script:HostServiceTask=$null
}
function Service-MendimaruHostService {
    if($null -eq $script:UiIdentity){return}
    if($null -ne $script:HostServiceTask -and ($script:HostServiceTask.IsCompleted -or $script:HostService.IsClosed)){Close-MendimaruHostService}
    $path=$controlPath+'.host-service.request'
    if(-not (Test-Path -LiteralPath $path)){return}
    $packet=$null;$accepted=$false
    try {
        $auth=Read-MendimaruAuthenticatedPayload -Path $path
        $packet=$auth.Json|ConvertFrom-Json
        if($auth.Sequence -le $script:HostServiceSequence -or
            (@($packet.PSObject.Properties.Name|Sort-Object) -join ',') -cne 'expiresAt,guestPort,host,id,pin,port,sealedKey,sealedM2eePassword,studioSessionId' -or
            $packet.id -cnotmatch '^hostsvc_[0-9a-f]{32}$' -or $packet.studioSessionId -cne $script:UiIdentity.sessionId -or
            [long]$packet.expiresAt -lt [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()-5000 -or
            [long]$packet.expiresAt -gt [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+35000 -or
            $packet.pin -cnotmatch '^[0-9a-f]{64}$' -or [int]$packet.port -lt 1 -or [int]$packet.port -gt 65535 -or
            [int]$packet.guestPort -lt 0 -or [int]$packet.guestPort -gt 65535){throw 'invalid-host-service'}
        $ip=[Net.IPAddress]::Parse([string]$packet.host);$b=$ip.GetAddressBytes()
        if($b.Length -ne 4 -or -not ($b[0]-eq 10 -or ($b[0]-eq 172 -and $b[1]-ge 16 -and $b[1]-le 31) -or ($b[0]-eq 192 -and $b[1]-eq 168))){throw 'invalid-host-service'}
        $script:HostServiceSequence=$auth.Sequence;$accepted=$true
        Remove-Item -LiteralPath $path -Force
        if($null -ne $script:HostService){throw 'host-service-owned'}
        $sealed=$packet.sealedKey
        if($sealed.version -ne 1 -or (@($sealed.PSObject.Properties.Name|Sort-Object) -join ',') -cne 'ciphertext,iv,version'){throw 'invalid-host-service'}
        $iv=[Convert]::FromBase64String([string]$sealed.iv);$cipher=[Convert]::FromBase64String([string]$sealed.ciphertext)
        if($iv.Length -ne 16 -or $cipher.Length -ne 48){throw 'invalid-host-service'}
        $aes=[Security.Cryptography.Aes]::Create();$derived=$null;$plain=$null;$key=$null;$decryptor=$null;$passwordBytes=$null;$password=$null
        try {
            $derived=$script:MendimaruHmac.ComputeHash([Text.Encoding]::ASCII.GetBytes('mendimaru-ui-value-v1'))
            $aes.Key=$derived;$aes.IV=$iv;$aes.Mode=[Security.Cryptography.CipherMode]::CBC;$aes.Padding=[Security.Cryptography.PaddingMode]::PKCS7
            $decryptor=$aes.CreateDecryptor();$plain=$decryptor.TransformFinalBlock($cipher,0,$cipher.Length)
            $key=[Convert]::FromBase64String((New-Object Text.UTF8Encoding($false,$true)).GetString($plain))
            if($key.Length -ne 32){throw 'invalid-host-service'}
            if(-not ('MendimaruHostService' -as [type])){
                $source=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__HOST_SERVICE_SOURCE_BASE64__'))
                Add-Type -TypeDefinition $source -Language CSharp -ErrorAction Stop
            }
            if($null -ne $packet.sealedM2eePassword){
                $sealedPassword=$packet.sealedM2eePassword
                if($sealedPassword.version -ne 1 -or (@($sealedPassword.PSObject.Properties.Name|Sort-Object) -join ',') -cne 'ciphertext,iv,version'){throw 'invalid-host-service'}
                $passwordIv=[Convert]::FromBase64String([string]$sealedPassword.iv)
                $passwordCipher=[Convert]::FromBase64String([string]$sealedPassword.ciphertext)
                if($passwordIv.Length -ne 16 -or $passwordCipher.Length -lt 16 -or $passwordCipher.Length -gt 272 -or $passwordCipher.Length % 16 -ne 0){throw 'invalid-host-service'}
                $aes.IV=$passwordIv
                $passwordDecryptor=$aes.CreateDecryptor()
                try {$passwordBytes=$passwordDecryptor.TransformFinalBlock($passwordCipher,0,$passwordCipher.Length)}finally{$passwordDecryptor.Dispose()}
                $password=(New-Object Text.UTF8Encoding($false,$true)).GetString($passwordBytes)
                if($script:UiIdentity.fileVersion -cne '11.12.6.0' -or [Text.Encoding]::UTF8.GetByteCount($password) -gt 256 -or [string]::IsNullOrEmpty($password) -or $password -match '[\x00-\x1f\x7f]'){throw 'invalid-host-service'}
                $script:HostService=New-Object MendimaruHostService([string]$packet.host,[int]$packet.port,[string]$packet.pin,[int]$packet.guestPort,$key,[int]$script:UiIdentity.processId,[long]$script:UiIdentity.startedTicks,[string]$script:UiIdentity.executable,$password)
            }else{
                $script:HostService=New-Object MendimaruHostService([string]$packet.host,[int]$packet.port,[string]$packet.pin,[int]$packet.guestPort,$key)
            }
            $script:HostServiceTask=$script:HostService.Run()
        }finally{
            foreach($bytes in @($derived,$plain,$key,$passwordBytes)){if($null -ne $bytes){[Array]::Clear($bytes,0,$bytes.Length)}}
            $password=$null
            if($null -ne $decryptor){$decryptor.Dispose()};$aes.Dispose()
        }
        $resultPath=$controlPath+'.host-service.report'
        Write-MendimaruReport ([ordered]@{id=$packet.id;ok=$true})
    }catch{
        # 인증 실패는 응답하지 않는다. 유효한 요청도 예외 값/키는 내보내지 않는다.
        if($accepted){
            $resultPath=$controlPath+'.host-service.report'
            Write-MendimaruReport ([ordered]@{id=$packet.id;ok=$false;reason=$(if($_.Exception.Message -ceq 'host-service-owned'){'owned'}else{'startup-refused'})})
        }
    }
}
