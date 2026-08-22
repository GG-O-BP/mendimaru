<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="public/mendimaru.png" alt="Mendimaru 로고" width="180">
</p>

<h1 align="center">Mendimaru</h1>

Mendix Studio Pro 버전을 탐지·설치·실행·제거하는 Tauri GUI 앱입니다. Windows에서는 네이티브로 실행하고 Linux에서는 WinBoat를 사용합니다.

## 화면 구성

- **Studio Pro**: 현재 Windows 환경의 Studio Pro 버전 탐지·실행·설치·안전한 제거
- **프로젝트**: 설정한 워크스페이스 안의 `.mpr` 프로젝트 탐지·실행
- **작업 센터**: 설치·제거·실행의 영속 진행 상태, 실패 원인과 재시도 가능 여부 확인
- **설정**: Windows 네이티브 워크스페이스와 포터블 Studio 경로 또는 Linux WinBoat 환경 지정

대시보드, VM 자원 정보, 고급 다운로드 URL, 수동 빌드 번호 입력은 제공하지 않습니다.

### 안전한 프로젝트 실행

프로젝트가 요구하는 Studio Pro 정확 버전이 설치되어 있으면 바로 엽니다. 버전이 없거나 알 수 없거나 명시적으로 선택한 버전과 다르면 실행 도우미가 Marketplace의 정확한 릴리스를 확인하고, 필요하면 설치한 뒤 동일 버전이 실제로 탐지된 경우에만 원래 `.mpr`를 엽니다. 설치된 다른 버전으로 암묵적으로 대체하지 않습니다. 불일치 버전 또는 버전을 알 수 없는 프로젝트를 열 때는 사용자가 버전을 직접 선택하고 백업 안내를 확인해야 합니다.

선택한 버전과 완료되지 않은 실행 의도는 취소, 설치 실패 및 앱 재시작 뒤에도 유지되어 이어서 진행할 수 있습니다. 이 설정은 호스트 전용 앱 설정 디렉터리에 저장하며, 프로젝트는 정규화된 경로의 SHA-256 digest로만 식별하고 실제 프로젝트 경로는 저장하지 않습니다.

### 환경 진단

설정 화면은 WinBoat 실행 파일, Compose 구조, 컨테이너 런타임 daemon, FreeRDP, 공유 워크스페이스와 마운트, 컨테이너 상태, Guest API, loopback RDP 포트 및 Marketplace 브라우저를 독립적으로 검사합니다. 실패한 항목에는 재탐지, Windows 시작, WinBoat 열기 또는 관련 설정 이동처럼 명시적으로 안전한 다음 행동만 제공합니다. 진단 보고서는 JSON으로 복사하거나 내보낼 수 있으며, 허용된 상태 필드만 포함하고 설정 경로, 자격 증명, token 및 명령 payload는 제외합니다.

### 영속 작업 이력

설치·제거·실행 작업은 신뢰할 수 없는 공유 워크스페이스 밖의 호스트 전용 앱 설정 디렉터리에 원자적으로 기록됩니다. 작업 센터는 화면 새로고침이나 앱 재시작 뒤에도 이력을 복원하고, 실패 단계와 안전한 원인, 확인 가능한 Windows 종료 코드를 표시하며, 재시도 가능한 작업과 프로젝트를 다시 선택해야 하는 보호된 실행을 구분합니다. 이전 앱 프로세스의 실행 중 기록은 시도별 HMAC 키가 더 이상 없으므로 오래된 결과를 신뢰하지 않고 중단됨으로 조정합니다. 기존 Windows 보고서는 파일명만 한 번 가져와 인증 불가한 중단 기록으로 표시하며 payload로 성공 여부를 추론하지 않습니다.

완료 이력 정리는 종결된 호스트 기록만 제거합니다. 실행 중인 작업, 다운로드한 설치 파일, 명령 스크립트와 Windows 보고서는 삭제하지 않습니다. 이력 스키마에는 프로젝트 경로, 명령 payload, URL, 자격 증명 또는 HMAC 키를 저장하지 않습니다.

## Windows 설치

GitHub Release 자산에서 MSI 또는 NSIS 설치 파일을 받습니다. Windows 빌드는 WinBoat, Docker, Guest API, RDP, FreeRDP와 경로 변환을 사용하지 않습니다.

네이티브 Windows 모드에서는 다음을 수행합니다.

- 32/64비트 제거 레지스트리, Mendix 표준 폴더, Version Selector 정보와 설정한 사용자/포터블 경로에서 Studio Pro 탐지
- `StudioPro.exe`를 직접 실행하고 선택한 `.mpr` 경로를 프로세스 인자로 전달
- 프로젝트와 설치 파일에 Windows 네이티브 경로만 사용
- UAC 요청 전에 다운로드 파일의 SHA-256 안정성과 Mendix/Siemens의 신뢰된 Authenticode 서명 검증
- 관리자 권한 설치 프로그램 또는 공식 등록 제거 프로그램의 실제 종료 코드와 설치 결과 확인
- 해당 `StudioPro.exe`가 실행 중이면 제거 거부, 공식 제거 정보가 없는 포터블 버전의 제거 버튼 비활성화

기존 설정은 자동 마이그레이션되며 새 `windowsStudioPaths` 목록은 기본적으로 비어 있으므로 Linux 설정도 그대로 유효합니다.

## Arch Linux 설치

AUR에서 `mendimaru` 패키지를 설치합니다.

```bash
paru -S mendimaru
```

WinBoat는 필수 의존성입니다. `winboat` 의존성을 충족하는 패키지가 없다면
`paru`가 AUR의 `winboat`를 자동으로 설치합니다. `winboat`, `winboat-bin`,
`winboat-electron`, `winboat-git` 중 하나가 이미 설치되어 있다면 그대로
사용하고 다시 설치하지 않습니다. 새 시스템에서 다른 패키지를 선택하려면
`paru -S winboat-bin mendimaru`처럼 같은 명령에 함께 지정할 수 있습니다.

Mendix Marketplace에서 설치 가능한 Studio Pro 버전을 조회하려면 Chromium 또는 Google Chrome도 필요하며, 두 브라우저는 선택 의존성으로 선언되어 있습니다.

## WinBoat 초기 설정

WinBoat가 설치되어 있지만 아직 Windows VM이 구성되지 않은 경우, Mendimaru의 **WinBoat 설정 시작** 버튼이 공식 WinBoat 설정 마법사를 엽니다. Windows 계정, VM 자원, Windows 이미지와 Guest Server 설치는 WinBoat가 담당합니다.

Mendimaru는 마법사가 완료될 때까지 상태를 확인한 뒤 다음 작업을 자동으로 마무리합니다.

- AUR `winboat-bin`의 `/opt/winboat/winboat`를 포함한 실행 파일 탐지
- `~/.winboat/docker-compose.yml` 또는 `podman-compose.yml` 탐지
- 실행 중인 컨테이너에서 Guest API와 RDP의 실제 동적 호스트 포트 탐지
- 설정한 Linux 워크스페이스를 Compose의 `/shared`에 적용
- Compose 원본을 `*.mendimaru.bak`으로 백업하고, 가상 디스크를 유지한 채 컨테이너 한 번 재생성

초기 설정을 취소하거나 창을 닫은 경우 **설정 계속**을 누르면 공식 마법사를 다시 열 수 있습니다. Mendimaru는 Windows 사용자명이나 암호를 별도 설정에 복사하지 않습니다.

## 다국어 지원

영어(`en-US`), 한국어(`ko-KR`), 일본어(`ja-JP`)를 지원합니다. 기본값은 시스템 언어이며, 헤더의 언어 선택 메뉴에서 바꾸면 앱 설정에 저장되어 다음 실행에도 유지됩니다. 지원하지 않는 시스템 언어는 영어로 대체됩니다.

번역과 로케일 처리는 Rust 백엔드가 담당합니다.

- 화면 문구와 백엔드 오류 문구는 `src-tauri/i18n/<locale>/mendimaru.ftl`의 Fluent 리소스에서 함께 관리합니다.
- `i18n-embed`가 번역 리소스를 실행 파일에 포함하고 시스템 언어 선택과 영어 폴백을 처리합니다.
- 날짜, 숫자, 다운로드 용량은 ICU4X로 형식화한 값만 프런트엔드에 전달합니다.
- 프런트엔드는 백엔드가 전달한 번역 번들을 표시하며, 문구를 기준으로 상태를 판별하지 않습니다. 다운로드 취소처럼 동작에 영향을 주는 값은 별도 코드와 상태로 전달합니다.
- 테스트는 모든 언어의 번역 키·변수 구성이 같은지, React에서 사용하는 정적 번역 키가 백엔드 번들에 포함됐는지 확인합니다.

언어를 추가할 때는 `src-tauri/src/i18n.rs`의 지원 로케일 목록에 BCP 47 언어 태그와 표시 이름을 등록하고, 기존 영어 파일과 키·변수 구성이 같은 Fluent 파일을 `src-tauri/i18n/<locale>/mendimaru.ftl`에 추가합니다. 새 화면 문구는 Fluent 파일 세 곳과 `src/shared/contracts/uiMessages.json`에 추가합니다. 이 레지스트리가 TypeScript 번역 키 타입과 Rust UI 번들에 함께 사용되며, `cargo test`가 빠진 번역이나 변수 불일치를 검출합니다.

## Studio Pro 버전 조회와 설치

`kirakiraichigo-mendix-manager`와 같은 방식으로 [Mendix Marketplace Studio Pro 페이지](https://marketplace.mendix.com/link/studiopro)의 데이터그리드를 Chromium으로 읽습니다.

- 첫 10개 버전을 자동으로 갱신하고 **이전 버전 더 불러오기**로 다음 페이지를 가져옵니다.
- 목록은 앱 캐시 디렉터리의 `studio-version-catalog.json`에 저장해 다음 실행 때 먼저 표시합니다.
- 최신, LTS, MTS, Beta 표시와 출시일을 함께 가져옵니다.
- Studio Pro 11 이상은 `Mendix-<version>-Setup.exe` 공식 아티팩트를 사용합니다.
- Studio Pro 10 이하는 버전 상세 페이지에서 `Build <number>`를 자동 추출해 `Mendix-<version>.<build>-Setup.exe`를 사용합니다.
- 사용자는 목록에서 버전을 고르기만 하면 되며 URL이나 빌드 번호를 입력하지 않습니다.
- 다운로드를 완료한 설치 파일은 기록된 출처, 예상 크기, Windows PE 구조와 SHA-256이 모두 일치할 때만 재사용합니다. 기존 버전의 메타데이터 없는 캐시나 변경된 캐시는 제거하고 다시 다운로드합니다.
- 설치되지 않은 각 카탈로그 버전에는 기존 캐시를 재사용하지 않고 설치 실패를 복구할 수 있는 강제 재다운로드 동작이 있습니다.

Windows에서는 시스템 및 사용자 표준 경로의 Microsoft Edge와 Chrome을 탐지합니다. Linux에서는 `MENDIMARU_CHROME_PATH`, `google-chrome-stable`, `google-chrome`, `chromium`, `chromium-browser` 순서로 찾습니다.

## Windows 경로

레지스트리와 Version Selector 정보 외에 다음 기본 위치도 탐지합니다.

| 용도 | Windows 경로 |
| --- | --- |
| Studio Pro 설치 루트 | `C:\Program Files\Mendix` |
| Studio Pro 실행 파일 | `C:\Program Files\Mendix\<version>\modeler\studiopro.exe` |
| Studio Pro 제거 정보 | `C:\ProgramData\Mendix` |
| 네이티브 기본 워크스페이스 | 존재하면 `%USERPROFILE%\Mendix`, 아니면 `%USERPROFILE%` |
| Linux WinBoat 공유 경로 | `\\host.lan\Data` |

설치 파일은 설정한 워크스페이스의 `.mendimaru/installers`에 저장합니다. 네이티브 모드에서는 서명을 검증한 뒤 명령 셸 없이 Windows 권한 상승 API로 실행합니다. Linux 모드에서는 따옴표 영향을 받지 않는 UTF-16LE 인코딩 PowerShell 명령을 WinBoat RemoteApp에 전달합니다. 설치 프로세스가 성공하고 해당 버전의 `StudioPro.exe`가 탐지된 뒤에만 완료로 처리합니다.

제거할 때도 Windows 제거 프로세스가 끝나고 해당 버전의 `StudioPro.exe`가 사라진 것을 확인한 뒤 설치된 버전 목록을 자동으로 갱신합니다.

앱을 시작하면 호스트의 비공개 캐시에서 마지막으로 검증된 설치 버전 목록을 복원하므로 알려진 Studio Pro 릴리스가 즉시 표시됩니다. 현재 Windows 목록은 백그라운드에서 다시 검증하며, 검증이 성공할 때까지 설치·제거·실행·프로젝트 열기 동작을 잠급니다. 검증에 실패해도 설치 목록을 빈 상태로 오인하지 않고 마지막 목록과 명시적인 재시도를 유지합니다.

Linux WinBoat 모드에서는 Studio Pro 실행 버튼이 Windows 프로세스의 실제 창이 생성되고 FreeRDP가 표시할 준비를 마칠 때까지 비활성화됩니다. 실행 준비 중에는 다른 버전과 프로젝트의 실행 버튼도 잠겨 중복 실행을 방지합니다. Windows는 공유 작업 스크립트의 해시를 고정하고 고유한 전용 경로에 복사해 그 사본만 실행합니다. 설치·제거는 이미 관리자 권한인 WinBoat 세션의 토큰을 상속하므로 별도의 UAC 창을 표시하지 않습니다.

## Linux 공유 워크스페이스

Linux 공유 디렉터리는 WinBoat Compose의 `<host path>:/shared` 마운트와 연결됩니다. 프로젝트 목록은 이 디렉터리만 탐색하며 `.git`, `node_modules`, `deployment`, `.mendix-cache`, `.mendimaru` 같은 생성·캐시 디렉터리는 제외합니다.

공유 디렉터리를 바꾸면 기존 Compose 파일을 `*.mendimaru.bak`으로 백업합니다. 설정에서 즉시 적용을 선택하면 WinBoat 컨테이너를 다시 만들지만 `/storage` 가상 디스크와 설치된 Windows 앱은 유지됩니다.

## Backend capability 계약

에이전트와 CI는 GUI를 시작하지 않고 플랫폼 중립 backend 계약을 조회할 수 있습니다.

```bash
mendimaru capabilities --json
```

응답은 host, Studio와 선택적 Runtime platform을 구분하고 Studio, Runtime, UI 자동화 및 browser의 모든 동작을 지원/미지원으로 명시합니다. `--backend`를 지정하면 현재 host와 정확히 일치해야 하며 다른 backend로 자동 fallback하지 않습니다. 자세한 내용은 [Platform backend와 capability 계약](docs/backend-contract.md) 및 기계 판독용 [JSON Schema](schemas/)를 참고하세요.

### Headless CLI

설치된 `mendimaru` 실행 파일은 Tauri나 대화상자를 시작하지 않고 환경 확인/준비, 정확한 Studio Pro 버전 목록·설치·제거·실행, Studio 세션 조회·종료, opaque 프로젝트 ID 조회, 영속 작업 조회·재시도를 수행합니다. 결과 JSON은 stdout, 오류 JSON은 stderr로 분리되며 `--ndjson`은 구조화된 진행 이벤트를 추가합니다. `--timeout-seconds`와 `Ctrl+C`는 공유 작업 경계에서 취소하고, 중단된 작업은 operation ID로 다시 조회할 수 있습니다. 전체 명령, 종료 코드, 스키마와 안전 규칙은 [Headless CLI 계약](docs/headless-cli.md)을 참고하세요.

Linux의 `browser test`는 명시적 URL, Portable Runtime 세션 또는 WinBoat Run Locally 세션에 동일한 선언형 Playwright/Chromium suite를 실행합니다. 브라우저 다운로드는 명시적으로만 수행하며 실패 시 마스킹된 HTML, DOM/accessibility, screenshot, trace, console, network 증거를 제한된 보존 정책으로 남깁니다. 자세한 내용은 [브라우저 테스트 가이드](docs/browser-testing.md)를 참고하세요.

## 개발

필수 환경은 Node.js 22.22.2 이상, Rust와 호스트 플랫폼용 Tauri 시스템 의존성입니다. Linux 통합에는 WinBoat, Docker 또는 Podman, FreeRDP 3, Chrome/Chromium이 추가로 필요하며 Windows 목록 조회에는 Edge 또는 Chrome을 사용합니다.

```bash
npm install
npm run tauri dev
```

검증과 번들 생성:

```bash
npm run check
npm run test:browser
npm run test:e2e
npm run tauri build
```

Linux에서 `npm run test:e2e`는 고정 버전의 `tauri-driver`와 `WebKitWebDriver`를 통해 debug 실행 파일을 Vite 개발 URL에 연결합니다. 격리된 WinBoat/API/프로젝트 fixture로 실제 WebView, Tauri IPC, 온라인 앱 상태, 프레임을 샘플링한 제한된 경로·작업 중 애니메이션, 유휴 상태에서 지속 애니메이션이 없다는 점과 주요 화면 전환을 검증합니다. 드라이버 브리지는 `cargo install tauri-driver --version 2.0.6 --locked`로 설치하고 호스트에는 `WebKitWebDriver`도 있어야 합니다. `npm run test:app-flow`는 OS 경계를 모킹한 빠른 React 앱 흐름 테스트이고, `npm run test:browser`는 Mendimaru 데스크톱 셸이 아니라 Mendix Runtime 페이지를 테스트합니다. CI는 세 계층을 모두 통과시킨 뒤 Windows/Linux 테스트와 Windows MSI/NSIS 스모크 빌드를 수행합니다.

실행 중인 실제 WinBoat에 대한 비파괴 RemoteApp 검증은 `npm run test:winboat-smoke`로 실행합니다. 인증된 세션 조회와 만료된 세션 거부를 검증합니다. 실제 상태를 바꾸는 수명주기 검증은 별도로 다음과 같이 실행하며, 이미 설치된 버전은 안전을 위해 거부합니다.

```bash
MENDIMARU_E2E_ALLOW_MUTATION=1 \
MENDIMARU_E2E_VERSION=11.13.0 \
npm run test:winboat-e2e
```

정확히 지정한 폐기 가능한 버전의 공식 설치 파일이 공유 캐시에 있어야 합니다. 이 테스트는 미설치 → 설치 → 실제 Studio 창 → 실행 중 삭제 거부 → 정상 종료 → 삭제 전 과정을 수행하며, 진행 단계 순서, 정확한 프로세스 식별자, 기존 설치본과 설치 캐시 불변성, 만료·반복 작업 거부, 잔류 프로세스와 예상 밖 RemoteApp/PowerShell 창이 없다는 점까지 검증합니다. 두 실제 VM 테스트 모두 격리된 Xvfb와 `xvfb-run`, `xfwm4`, `wmctrl`이 필요합니다. Arch Linux에서는 `xorg-server-xvfb` 패키지가 `xvfb-run`을 제공합니다. CI에는 실제 WinBoat VM이 없으므로 파괴적 수명주기 검증은 로컬/수동 릴리스 게이트이며 CI에서 통과했다고 주장하지 않습니다.

전체 Rust 테스트는 레지스트리 파싱, 경로 격리, 파일 무결성, Windows 인자 인코딩, UAC/종료 코드 실패와 설치부터 제거까지의 fixture 수명주기를 검증합니다.

Rust와 TypeScript가 공유하는 직렬화 enum 값은 `src/shared/contracts/enumValues.json`에서 관리합니다. TypeScript는 여기서 유니언 타입을 만들고, Rust 테스트는 계약 불일치를 차단합니다.

실제 Marketplace 연동 테스트는 기본 테스트에서 제외되며 다음처럼 실행할 수 있습니다.

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## 보안

Windows 네이티브 명령은 경로를 명령 셸에 삽입하지 않습니다. 설치 파일, 설치된 Studio 실행 파일과 등록된 Mendix 제거 프로그램은 Mendix 또는 Siemens가 발행한 유효한 신뢰 Authenticode 서명이 있어야 하며 검증 전후 해시로 파일 교체도 탐지합니다. Windows Installer 제거는 제품 코드에 대한 `/x` 작업과 알려진 비대화형 플래그로 제한하고, 등록 제거 프로그램은 선택한 설치본에 속하면서 허용 목록의 플래그만 사용해야 합니다. UAC 취소나 실패 종료 코드는 성공으로 처리하지 않습니다.

Linux에서는 Windows 사용자명과 암호를 앱 설정에 저장하지 않습니다. RemoteApp 실행 시 실행 중인 WinBoat 컨테이너에서 자격 증명을 읽어 FreeRDP 3의 표준 입력으로 전달합니다. FreeRDP는 앱 전용 TOFU 인증서 핀을 사용하고, 관리자 권한 작업은 Guest API와 RDP가 loopback에만 바인딩된 경우에만 허용합니다. 공유 작업 결과와 유지 중인 Studio 세션 제어 요청은 시도별 HMAC 키와 재전송 방지 sequence로 인증합니다.

위협 모델, 실행 파일 신뢰 체인, 컨테이너 권한과 잔여 위험 및 신고 방법은 [보안 정책과 WinBoat 신뢰 경계](SECURITY.md)를 참고하세요.

## 라이선스

Mendimaru는 [MIT 라이선스](LICENSE)로 제공됩니다.
