# WinBoat 세션 호스트 서비스 연결

`linux-winboat`의 `host-service.start`, `host-service.status`,
`host-service.stop` capability는 Studio Pro **Run Locally** 앱에서 Linux의
MySQL을 사용하는 연결을 관리한다. `host-service list`는 같은 비공개 캐시의
세션을 조회한다. Windows/macOS backend는 명시적으로 미지원이다.

```text
Studio Pro Run Locally
  → Windows 127.0.0.1:<guestPort>
  → 세션 인증 + 인증서 pinning TLS 스트림
  → 실행 중인 WinBoat의 검증된 사설 브리지 gateway:<임시 포트>
  → Linux 127.0.0.1:3306
```

이 기능은 MySQL 서버 설정, DB/계정/권한, 호스트 방화벽, Compose 포트 매핑을
변경하지 않는다. 중계의 사설 브리지 리스너는 세션 동안만 존재한다.
실제 환경에 이 노출을 추가할 권한이 있는 운영자가 시작해야 한다.
별도 SSH 서버, PuTTY, 게스트 설치 패키지나 관리자 권한은 필요하지 않다.

## 사용

먼저 기존 [Run Locally 절차](winboat-run-locally.md)에 따라 Studio를
시작하고 연결된 Runtime 세션을 확인한다. 관리되는 Studio keeper가 필요하다. Compose 재생성이 필요하면 Studio에서 앱을 실행하기 전에 준비한다.

```sh
mendimaru studio start --version 11.12.6 --project-id PROJECT_ID --json
mendimaru runtime list --json
mendimaru host-service start --service mysql --allow-host-port 3306 \
  --runtime-session-id RUNTIME_SESSION_ID --json
```

`guestPort`가 배정된 Windows loopback DB 포트다. Studio의 지원되는
**App Settings / Configurations** UI에서 MySQL 목적지를
`127.0.0.1:<guestPort>`로 설정하고 기존 DB와 최소 권한 계정을 사용한다.
고정 게스트 포트가 필요하면 `--guest-port PORT`를 명시한다. 기본값 `0`은
Windows 운영체제가 빈 포트를 원자적으로 배정하게 한다.
프로젝트의 `project-settings.user.json`을 일반 파일 도구로 읽거나 수정하지 않는다.
중계 CLI는 DB 자격증명을 받지 않는다. 자격증명은 Studio 구성 UI에만
입력하며, UI 자동화로 비밀번호 입력을 우회하거나 구성 파일을 편집하지 않는다.

Studio의 **Run Locally**로 앱을 실행한 다음:

```sh
mendimaru runtime wait --session-id RUNTIME_SESSION_ID --json
mendimaru host-service status --session-id HOST_SERVICE_SESSION_ID --json
mendimaru host-service list --json
```

시작의 `state=ready`는 인증된 게스트 helper가 loopback 리스너를 만들고,
그 리스너에서 실제 중계를 통해 MySQL 초기 greeting을 읽었다는 뜻이다.
`guestListenerVerified`, `targetTcpVerified`, `mysqlHandshakeVerified`가
각 근거를 표시한다. 초기 greeting은
[MySQL HandshakeV10](https://dev.mysql.com/doc/dev/mysql-server/8.0.46/page_protocol_connection_phase_packets_protocol_handshake_v10.html)의
프로토콜 번호만 검사한다. `databaseAuthenticationVerified=false`는 항상
유지한다. 로그인·질의·앱 기동의 성공은 소비 앱의 별도 수용 증거여야 한다.

같은 Runtime으로 다시 시작하면 살아 있는 연결을 재사용한다. 다른 게스트
포트를 요구하면 거부한다. 각 Runtime은 별도 게스트 포트를 가진다.
Windows 소켓의 exclusive bind가 캐시/작업 트리를 가로지르는 포트 충돌을
판정하며 기존 사용자 리스너는 정리하거나 대체하지 않는다.
고정 포트는 이전 TCP 연결의 OS 정리가 끝날 때까지 재사용이 거부될 수 있다.
이때 잠시 후 재시도하거나 기본 자동 포트 배정을 사용하고 Studio UI의 포트를
갱신한다. [Windows exclusive bind 동작](https://learn.microsoft.com/en-us/windows/win32/winsock/so-exclusiveaddruse)에
따라 임의 소켓을 덮어쓰는 reuse 옵션이나 응답을 잃을 수 있는 강제 reset을
사용하지 않는다.

## 보안과 스트리밍

- 허용된 목적지는 `mysql`과 명시적인 `--allow-host-port 3306`뿐이다.
  호스트명/DNS, 임의 원격 IP, 목적지 변경 메시지와 범용 TCP 프록시는 없다.
- 브리지 주소는 매번 컨테이너 검사로 찾는다. 한 개의 사설 IPv4 네트워크와
  동일 subnet의 gateway/컨테이너 주소가 필요하다. 공개·host-mode·모호한
  다중 네트워크는 거부한다. 현재 머신의 내부 IP를 기본값으로 저장하지 않는다.
- 매 세션의 인증서와 키는 메모리에만 존재한다. 연결된 Studio에서는 기존
  keeper/monitor의 인증 채널을 사용하므로 추가 RDP 연결이 기존 RemoteApp을
  끊지 않는다. hash 검증된 monitor에 helper 소스가 포함되며 시작 키는
  별도 파생 AES 키로 암호화하고 기존 HMAC으로 IV·암호문까지 인증한 후
  복호화한다. 공유 파일에는 평문 키나 DB 자격증명이 없다. Studio 연결이 없는 Runtime은 시작을 거부한다. 게스트는 인증서 SHA-256을 pin하고 TLS 1.2를 사용한다.
- TLS 안에서 연결별 임의 challenge에 대한 HMAC을 검증한다. 다른 세션의
  키, 이전 proof의 재전송과 제어/데이터 채널 바꿔치기를 거부한다. TLS
  0-RTT와 세션 재개를 사용하지 않는다.
- 게스트는 최대 64개 DB 연결을 처리한다. 호스트는 제어 연결을 포함해 최대
  65개 handshake/연결 작업을 허용한다. 프레임은 최대 16 KiB이고 방향별
  쓰기 완료를 기다린 뒤 다음 데이터를 읽는다. SQL 요청마다 프로세스나
  파일 mailbox, RDP 연결을 새로 만들지 않는다.
- 연결/TLS/인증 제한은 10초, 방향별 유휴 제한은 300초다. EOF를 길이 0인
  프레임으로 전송해 TCP half-close와 반대 방향의 마지막 응답을 보존한다.
  게스트는 한쪽 EOF 후 반대 방향 완료를 최대 300초 기다린다. 이 기한은
  로컬 peer가 닫지 않는 half-close의 슬롯 점유도 제한한다. 소켓 오류와 취소는
  양방향 연결을 닫는다.
- 세션별 제어 연결이 heartbeat를 주고받는다. 호스트/VM/helper가 중단되면
  게스트는 제어 단절 또는 10초 watchdog 후 자신이 소유한 리스너와 연결을
  닫는다. 새 시작은 새 키로 인증하며 이전 세션을 되살리지 않는다.

## 종료·복구·제거

```sh
mendimaru host-service stop --session-id HOST_SERVICE_SESSION_ID --json
```

연결된 Studio 세션이 있으면 기존 keeper IPC를 통해 그 정확한 Studio에 정상
종료를 요청하고 Runtime 종료를 마친 다음 중계를 닫는다. 저장 대화상자나
종료 실패가 있으면 연결을 유지하고 오류를 반환한다. Studio ID가 없는 앱이
이미 HTTP-ready이면 먼저 Studio UI에서 앱을 정상 종료해야 한다.
`stop`은 Studio 창도 종료할 수 있으므로 편집 내용을 저장한 후 실행한다.
다른 Runtime/Studio ID나 사용자 포트는 대상이 아니다.

Runtime이 별도로 종료되거나 helper가 끝나면 중계도 종료된다. keeper의
SIGINT/SIGTERM은 중계만 `interrupted`로 정리한다. 비정상 종료 후 `status`는
사라진 keeper를 `interrupted`로 보고하고 PID 추측으로 프로세스를 죽이지 않는다.
CLI 재실행은 중계를 유지하며, VM 재시작으로 끊어진 연결은 새 세션을 시작해
복구한다. 반환된 포트가 달라지면 Studio UI의 DB 포트도 확인한다.

비공개 상태는 앱 캐시의 `host-services/`에 있다. 디렉터리는 `0700`, 파일과
IPC 소켓은 `0600`이다. 버전 1 레코드는 VM identity와 세션 ID를 검증하며,
간접 파일·다중 hardlink·과대 파일·다른 VM·다른 UID·공개 권한을 거부한다.
레코드에는 비밀값이 없고 기존 Runtime 레코드 구조는 바뀌지 않는다.

캐시 손상 시 먼저 `list`/`status`로 관찰하고, 중계가 끝났음을 확인한 후
해당 세션의 JSON/소켓만 별도 위치에 격리한다. 같은 Runtime의 `.lock` 파일은
커널 flock으로 소유하므로 유지해도 된다. 제거하려면 모든 중계를 종료하고
`host-services/`를 제거한다. helper는 기존 Studio monitor에 포함된다.
시작 요청과 보고서는 기존 Studio 제어 경로의 `.host-service.request`와
`.host-service.report` 파일을 사용하며 시작 작업이 끝나면 제거한다.
강제 종료로 이 파일이 남으면 해당 Studio keeper가 종료됐음을 확인한 뒤
그 세션의 두 파일만 제거한다. 다른 Mendimaru 세션의 파일은 보존한다.

## 검증 범위

Rust 회귀는 인증/재전송, 제한된 바이너리 프레임, EOF/half-close, 대상 제한,
브리지 검사, 비공개 상태 신뢰와 Runtime 소유권 잠금을 검사한다.
실제 VM에서 MySQL greeting, 포트 충돌과 제어 단절 후 포트 재사용을 확인하는
비파괴 게이트는 다음과 같다. 기존 Runtime ID를 명시적으로 선택한다.

```sh
MENDIMARU_HOST_SERVICE_LIVE=1 \
MENDIMARU_HOST_SERVICE_RUNTIME_ID=RUNTIME_SESSION_ID \
cargo test --manifest-path src-tauri/Cargo.toml \
  live_mysql_greeting_ownership_and_control_loss_cleanup -- --ignored --nocapture
```

이 게이트는 DB 로그인이나 exact Studio 11.12.6 앱 수용·paired 성능 측정을
대신하지 않는다. 소비 앱의 읽기/쓰기/트랜잭션·연결별 isolation·시간대와
같은 데이터의 기준선/후보 latency·throughput·메모리 증거가 별도로 필요하다.
