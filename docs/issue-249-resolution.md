# #249: Studio Run Locally의 Linux MySQL 연결

`host-service start/status/stop/list`가 관리되는 Studio 세션에 Windows loopback
리스너를 연결한다. 목적지는 명시적으로 허용한 Linux `127.0.0.1:3306`이다.
기존 Studio monitor가 helper를 실행하므로 DB 연결마다 프로세스나 RDP 연결을
추가하지 않는다. 사용·보안·복구 절차는 [운영 문서](winboat-host-services.md),
재현 가능한 앱은 [수용 하네스](../scripts/e2e/host-service/README.md)에 있다.

## 실제 앱 수용

2026-10-15 Debian 13 / WinBoat Windows / MySQL Community 8.4.11에서 별도
`RelayAcceptance` 앱을 생성했다. 공식 Studio 11.12.6의 `mx show-version`과
`mx check`로 exact 버전 및 **0 errors**를 확인했다. Studio가 프로젝트를 열고
Run Locally로 빌드·기동했다. 초기 F5는 OS 입력을 보조했고, 반복 측정에서는
같은 Studio의 Run 버튼을 UI Automation Invoke로 실행했다. portable runtime,
직접 Java 기동이나 hot-swap을 provenance로 사용하지 않았다.

전용 DB 구성은 Studio Configurations UI에서 입력했다. 프로젝트 사용자
설정 파일을 일반 파일 도구로 읽거나 편집하지 않았다. DB를 재생성하거나
계정 권한을 확장하지 않았다. 앱의 REST microflow로 다음을 확인했다.

- 쓰기 → commit → 값 변경 → commit → 다시 읽기: balance `101.25`.
- commit 후 예외를 발생시킨 요청: 전체 요청 rollback, 해당 row 없음.
- **앱 pool 연결**: `8.4.11 | READ-COMMITTED | ROW | InnoDB`.
- 같은 연결의 named time zone 변환: UTC → Asia/Seoul, `09:00:00`.
- 중계를 통한 없는 DB 계정/빈 인증 응답: MySQL **1045** 거부.
  실제 계정의 틀린 비밀번호를 수집하는 검사는 사용하지 않았다.

이 수용 결과는 [각 측정 원본](evidence/issue-249/baseline-steady-1.json)의
`acceptance`에도 포함된다. 시작 API의 `databaseAuthenticationVerified`는
계속 `false`다. greeting 확인을 DB 로그인 성공으로 표시하지 않는다.

## 성능

최종 [시계열 ABBA 결과](evidence/issue-249/paired-sampled-summary.json)는
latency·처리량·메모리의 **+10% 회귀 예산을 모두 통과**했다.
같은 머신·앱·DB·debug profile·전송 경계에서 ABBA 순서로 측정했다.
기준선은 이 기능의 초기 스트림 구현으로 Rust 호스트→게스트 프레임의
header/payload를 각각 TLS write하고, 후보는 한 번에 쓴다. C# helper는
양쪽 arm에서 같다. 기능이 없는 main을 기준선으로 삼지 않았다.
재현 patch, 바이너리·소스 hash와 전체 원본을 보관했으며 release profile의
절대 성능을 주장하지 않는다.

trial마다 수용 검사 전후 10,000 rows × payload 1,024 bytes로 초기화하고,
4 lanes × 200회 워밍업 후 1,000회씩 read → update/commit → balance read를
실행했다. 각 trial의 측정 요청은 **12,000개**, 각 arm은 **24,000개**다.
데이터 크기·요청 수·JVM heap 설정을 회귀 회피 목적으로 바꾸지 않았다.

| 항목                        |  기준선 |    후보 | 회귀율 |
| --------------------------- | ------: | ------: | -----: |
| 평균 latency (ms)           |  15.763 |  15.026 | -4.67% |
| p50 (ms)                    |  13.637 |  12.835 | -5.88% |
| p95 (ms)                    |  30.947 |  30.534 | -1.33% |
| p99 (ms)                    |  48.034 |  46.625 | -2.93% |
| 처리량 (requests/s)         | 231.788 | 242.414 | -4.58% |
| Java working set 평균 (MiB) | 446.575 | 451.240 | +1.04% |
| Java working set peak (MiB) | 477.828 | 478.379 | +0.12% |
| 게스트 monitor 평균 (MiB)   | 176.609 | 172.483 | -2.34% |
| 게스트 monitor peak (MiB)   | 184.508 | 181.016 | -1.89% |
| Linux keeper 평균 RSS (MiB) |  85.519 |  85.625 | +0.12% |
| Linux keeper peak RSS (MiB) |  86.102 |  86.734 | +0.73% |

양수 회귀율은 latency/메모리 증가 또는 처리량 감소다. latency는 두 trial
요약의 산술 평균, 처리량은 총 요청/총 시간이다. Java/게스트 monitor의
working set은 **전체 측정 구간**에서 목표 100ms 간격으로 수집한다.
평균은 trial 평균들의 평균, peak는 arm 내 두 trial 중 최댓값이다.
Linux RSS는 앱 시작·워밍업·측정 동안의 관찰값이며 같은 집계 규칙을 쓴다.
게스트 monitor 수치는 helper를 포함한 기존 Studio monitor 프로세스다.
별도 workload driver 메모리는 제품 메모리에서 제외하고 원본에 따로 남겼다.
종료 시점 working set도 보존했다. 새 실행의 종료 시점 Java 값은 +1.73%였다.

앞선 결과도 삭제하거나 통과로 바꾸지 않았다.

- 짧은 pilot은 첫 arm만 빈 테이블에서 시작해 비교 조건이 달랐다. 같은
  pre-seed와 긴 워밍업으로 [전체 ABBA](evidence/issue-249/paired-summary.json)를
  다시 실행했다. 이 비교는 통과했다.
- 새 VM의 [단일 후보 확인](evidence/issue-249/cold-candidate-guard.json)에서
  p99가 앞선 기준선보다 약 +10.7% 높았다. 이 결과를 통과 근거로 사용하지
  않고 같은 helper·포트 정책을 양쪽에 적용한 전체 ABBA를 실행했다.
- 그 [후속 ABBA](evidence/issue-249/paired-final-summary.json)는 종료 시점
  Java working set **+12.79%**로 실패했다. 이 실패를 보존하고, 단일 시점과
  측정 구간을 구분하도록 시계열 관찰을 추가했다. 위 최종 비교는 같은
  데이터·워밍업·요청 수로 평균과 peak를 모두 검증한 결과다.

경계는 Windows loopback HTTP → Studio Run Locally → 관리 TLS 중계 → Linux
loopback MySQL이다. 이 환경에서는 Linux→게스트 앱 HTTP가 timeout이어서
Linux Chrome 접근 성공으로 보고하지 않는다. 기존 Compose/HTTP 포워딩을
수정하거나 게스트 방화벽을 열지 않았다. 기존 HTTP 및 다중 Runtime 소유권은
Rust 회귀와 별도의 실제 Studio F5 릴리스 게이트로 검증한다.

## 회귀와 업그레이드

계약은 닫힌 capability enum 확장에 따라 schema **7.0.0**이다. 기존 Runtime,
browser, artifact의 schema 4/5/6 레코드와 operation history 1–6은 기존
호환 규칙으로 읽으며 새 host-service 레코드는 별도 version 1 저장소를 쓴다.
레코드 신뢰·active port scan·forget·중단/손상/identity mismatch·Compose 복원·
keeper socket·CLI 오류 회귀를 기존 업그레이드 matrix와 함께 실행했다.

Rust 전체 회귀는 library **515 passed, 22 ignored**, CLI contract **9**,
CLI E2E **19**, lifecycle matrix **1**이며 clippy `-D warnings`가 통과했다.
일반 회귀는 TLS pin/HMAC/재전송/채널 바꿔치기, 잘못된 목적지, framing/크기 제한,
동시 연결, half-close, 취소와 포트 정리를 포함한다. 별도 실제 WinBoat 게이트
`live_mysql_greeting_ownership_and_control_loss_cleanup`도 통과했다.

실제 keeper 강제 종료 후 `interrupted`와 검증 flag 초기화를 확인했고,
기존 Studio를 보존한 채 새 키/리스너로 다시 시작하여 앱 수용을 반복했다.
정상 Studio 종료에 따른 VM 재생성 이후에도 새 Studio/중계/앱이 복구됐다.
정상 앱 중지 후 host-service stop은 정확한 Studio와 Runtime을 종료하고
`stopped`를 반환했으며 별도 CLI의 재종료도 성공했다. 앱이 실행 중인 종료가
Studio의 정상 종료 기한을 넘으면 실패하고 중계를 보존한다.

별도 실제 CLI 재실행은 같은 세션을 재사용했다. 다른 guest port 요구,
허용하지 않은 host port 및 임의 host 인자는 거부됐다. 실제 사설 TLS
리스너에 인증 키 없이 접속해 임의 proof를 보낸 경우 연결이 닫혔고,
기존 인증된 소유자는 계속 `ready`였다. [결과](evidence/issue-249/negative.json).

## 설치 패키지 재검증

0.7.0 Debian 패키지를 빌드하고 사용자 디렉터리에 추출해 설치했다. 기존
시스템 설치본은 보존했다. [패키지와 바이너리 hash](evidence/issue-249/installed-package.json),
[설치본 help](evidence/issue-249/installed-help.txt)와
[capabilities](evidence/issue-249/installed-capabilities.json)를 기록하고
schema 7 / 25 capabilities 계약 검사를 통과했다.

이 설치본으로 새 중계를 시작하고 같은 Studio 앱의 실제 읽기·쓰기·commit·
rollback·앱 pool 설정과 잘못된 DB 인증 거부를 다시 확인했다. 앱을 정상 중지한
뒤 설치본의 공개 stop 명령으로 연결된 Studio/Runtime을 정상 종료했다.
재종료는 성공했고 소유한 사설 리스너가 닫혔다.
[설치본 실제 실행 결과](evidence/issue-249/installed-live-proof.json).

최종 소스로 [실제 WinBoat transport 게이트](evidence/issue-249/live-winboat-gate.txt)와
[Windows 생산 helper의 포트·half-close 계약](evidence/issue-249/guest-helper-proof.json)도
다시 통과했다. 일반 회귀에서 제외한 실제 환경 검사는 별도로 실행한 결과다.
