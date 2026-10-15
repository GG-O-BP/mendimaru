# 실제 Studio MySQL 수용 하네스

이 하네스는 사용자가 승인한 **폐기 가능한 앱과 전용 DB**에서만 실행한다.
기존 DB/계정/권한과 방화벽을 바꾸지 않는다. 중계는 DB 자격증명을 받지 않는다.

1. `mxcli new RelayAcceptance --version 11.12.6 --theme none --layout none --skip-init --skip-build`
   로 별도 앱을 만들고 `mxcli exec fixture.mdl -p APP.mpr`를 실행한다.
   공식 11.12.6 `mx show-version APP.mpr`, `mx check APP.mpr`의 결과를 보관한다.
2. 증거 디렉터리의 `guest-start.ps1`에 아래 wrapper를 만든다. 두 경로는
   현재 WinBoat 공유 경로에 맞춘 **공개 스크립트 경로**다. 비밀값을 넣지 않는다.

   ```powershell
   param([int]$RuntimePid)
   & 'WINDOWS_SOURCE_PATH\guest-driver.ps1' -RuntimePid $RuntimePid -OutputDirectory $PSScriptRoot
   ```

3. Studio를 열기 전에 모델의 `MyFirstModule.GuestDriverPath` 상수 기본값을
   wrapper의 Windows 경로로 설정한다. 빈 기본값은 driver를 실행하지 않는다.
   `project-settings.user.json`을 일반 도구로 읽거나 수정하지 않는다.
4. 관리되는 Studio 11.12.6과 연결된 Runtime을 만들고 `host-service start`로
   게스트 포트를 배정받는다. Studio Configurations UI에서 전용 MySQL 구성의
   목적지를 `127.0.0.1:<guestPort>`로 설정하고 전용 계정의 값을 UI에 입력한다.
   고정 포트를 재사용하면 Windows의 이전 TCP 연결 정리가 끝날 때까지 bind가
   거부될 수 있으므로 기본값인 자동 포트 배정도 검증한다.
5. 각 trial 전에 Studio에서 앱을 정상 종료한다. 해당 중계 keeper만 종료하고
   다음 구현으로 같은 Runtime에 새 중계를 시작한다. 배정된 포트만 Studio의
   기존 전용 구성 UI에서 갱신한다. 다른 Studio나 사용자 리스너를 종료하지
   않는다. 세션의 새 ID·포트와 바이너리 hash를 보관한다.
6. 증거 디렉터리에 원자적으로 `guest-command.json`을 만든 다음 Studio의
   Run Locally로 앱을 시작한다. 예:

   ```json
   { "id": "baseline-steady-1", "action": "workload", "guestPort": 3306 }
   ```

`guest-driver.ps1`은 앱의 after-startup Java action에서 별도 소비 프로세스로
실행된다. 이미 열려 있는 Studio에 추가 RDP 연결을 만들지 않는다. 첫 명령을
최대 5분 기다리고 한 번만 실행한다. `acceptance`로 시작하는 ID는 수용 검사만
실행하고 그 밖의 ID는 전체 workload를 실행한다. 결과는 `<id>.json`이다.
`guestPort`는 해당 trial의 실제 배정값이며 잘못된 인증 검사와 listener 메모리
관찰에 사용한다. 생략 시 이전 고정 포트 하네스와 호환되는 `3306`이다.

앱 REST microflow로 쓰기·읽기·두 commit 후 balance, 예외 발생 후 rollback을
검사한다. 앱 pool의 연결로 MySQL 버전·격리 수준·ROW·InnoDB·named time zone을
읽는다. 없는 계정과 빈 인증 응답으로 MySQL 1045 거부도 확인한다. 실제 계정의
비밀번호를 driver에 전달하거나 진단 파일에 복사하지 않는다.

성능 trial마다 수용 검사 전후에 동일한 **10,000개, payload 1,024 byte**의
논리 데이터를 준비한다. 첫 trial만 빈 데이터에서 시작해 GC 조건이 달라지는
것을 방지한다. 4개 lane에서 각각 200회 워밍업한 뒤 1,000회씩
`read → update/commit → balance read`를 실행한다. 측정 요청은 총 12,000개다.
포트 greeting이나 직접 SQL 쓰기는 이 측정에 포함되지 않는다.

같은 머신·앱·DB·데이터·빌드 profile에서 기준선/후보를 ABBA 순서로 비교한다.
`uncoalesced-baseline.patch`는 Rust 호스트→게스트 방향의 프레임 header/payload
write만 나누며, 양쪽 arm은 같은 C# helper와 인증/전송 프로토콜을 사용한다.
경계는 Windows loopback HTTP → Studio Run Locally → 인증된 TLS 중계 → Linux
loopback MySQL이다. HTTP에 대한 게스트 방화벽 변경은 필요하지 않다.
latency와 처리량은 측정 구간만 사용한다. guest relay와 Java runtime의 working
set, Linux keeper의 RSS도 보관하며 trial driver 메모리는 제품 메모리와 구분한다.
앱/게스트 relay는 측정 구간에 목표 100ms 간격 working-set 시계열과 평균·peak를
수집한다. GC 직후의 종료 시점 한 번만으로 메모리 회귀를 판정하지 않으며,
종료 시점 값도 원본에 남긴다. 양쪽 arm에서 같은 관찰 방법을 사용한다.
평균/percentile 지연 증가, 처리량 감소와 메모리 증가는 각각 +10% 예산으로
검토한다. 결과가 넘으면 실패를 숨기거나 데이터 크기를 줄이지 않는다.
