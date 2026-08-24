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
- **설정**: Windows 네이티브 워크스페이스와 포터블 Studio 경로 또는 Linux WinBoat 환경 지정

대시보드, VM 자원 정보, 고급 다운로드 URL, 수동 빌드 번호, 강제 재다운로드 옵션은 제공하지 않습니다.

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

Windows에서는 시스템 및 사용자 표준 경로의 Microsoft Edge와 Chrome을 탐지합니다. Linux에서는 `MENDIMARU_CHROME_PATH`, `google-chrome-stable`, `google-chrome`, `chromium`, `chromium-browser` 순서로 찾습니다.

## Windows 경로

레지스트리와 Version Selector 정보 외에 다음 기본 위치도 탐지합니다.

| 용도 | Windows 경로 |
| --- | --- |
| Studio Pro 설치 루트 | `C:\Program Files\Mendix` |
| Studio Pro 실행 파일 | `C:\Program Files\Mendix\<version>\modeler\studiopro.exe` |
| Studio Pro 제거 정보 | `C:\ProgramData\Mendix` |
| 네이티브 기본 워크스페이스 | `%USERPROFILE%\Mendix` (없으면 다른 디렉터리를 지정하도록 안내) |
| Linux WinBoat 공유 경로 | `\\host.lan\Data` |

설치 파일은 설정한 워크스페이스의 `.mendimaru/installers`에 저장합니다. 네이티브 모드에서는 서명을 검증한 뒤 명령 셸 없이 Windows 권한 상승 API로 실행합니다. Linux 모드에서는 따옴표 영향을 받지 않는 UTF-16LE 인코딩 PowerShell 명령을 WinBoat RemoteApp에 전달합니다. 설치 프로세스가 성공하고 해당 버전의 `StudioPro.exe`가 탐지된 뒤에만 완료로 처리합니다.

Marketplace 카탈로그 캐시는 최대 6시간 재사용하므로 일반 시작 시 백그라운드 브라우저를 다시 띄우지 않습니다. 즉시 갱신하려면 카탈로그 새로고침을 사용하면 됩니다.

제거할 때도 Windows 제거 프로세스가 끝나고 해당 버전의 `StudioPro.exe`가 사라진 것을 확인한 뒤 설치된 버전 목록을 자동으로 갱신합니다.

Linux WinBoat 모드에서는 Studio Pro 실행 버튼이 Windows 프로세스의 실제 창이 생성되고 FreeRDP가 표시할 준비를 마칠 때까지 비활성화됩니다. 실행 준비 중에는 다른 버전과 프로젝트의 실행 버튼도 잠겨 중복 실행을 방지합니다. 실행 스크립트는 공유 폴더에 저장하고 짧은 호출 명령만 RemoteApp으로 전달해 FreeRDP RAIL의 명령 길이 제한을 넘지 않습니다. Windows Script Host가 PowerShell을 숨김 모드로 실행하며, 설치·제거는 이미 관리자 권한인 WinBoat 세션의 토큰을 상속하므로 PowerShell 콘솔이나 별도의 UAC 창을 표시하지 않습니다.

## Linux 공유 워크스페이스

Linux 공유 디렉터리는 WinBoat Compose의 `<host path>:/shared` 마운트와 연결됩니다. 프로젝트 목록은 이 디렉터리만 탐색하며 `.git`, `node_modules`, `deployment`, `.mendix-cache`, `.mendimaru` 같은 생성·캐시 디렉터리는 제외합니다.

공유 디렉터리를 바꾸면 기존 Compose 파일을 `*.mendimaru.bak`으로 백업합니다. 설정에서 즉시 적용을 선택하면 WinBoat 컨테이너를 다시 만들지만 `/storage` 가상 디스크와 설치된 Windows 앱은 유지됩니다.

## 개발

필수 환경은 Node.js 22.22.2 이상, Rust와 호스트 플랫폼용 Tauri 시스템 의존성입니다. Linux 통합에는 WinBoat, Docker 또는 Podman, FreeRDP 3, Chrome/Chromium이 추가로 필요하며 Windows 목록 조회에는 Edge 또는 Chrome을 사용합니다.

```bash
npm install
npm run tauri dev
```

검증과 번들 생성:

```bash
npm run check
npm run check:windows
npm run tauri build
```

`npm run test:ui:integration`은 OS 경계를 모킹한 빠른 React 흐름을 실행합니다. Windows의 `npm run test:e2e`는 테스트 전용 Cargo feature로 실제 앱을 `tauri dev`로 시작하고, 네이티브 WebView2 창을 임베디드 WebDriver로 조작합니다. 실제 IPC, 레지스트리 탐지, 프로젝트 스캔, 설정 저장, Edge 기반 Marketplace 갱신, CSP 차단, 잘못된 입력 거부와 성능 예산을 검증합니다. 설정과 캐시는 안전 마커가 있는 임시 디렉터리로 격리하고 실행 후 제거합니다. WebDriver feature·권한·전역 Tauri 브리지는 일반 개발 및 릴리스 빌드에 포함되지 않습니다.

CI는 Windows와 Linux에서 프런트엔드/Rust 테스트와 npm/Cargo 잠금 파일 보안 감사를 실행하고, Windows에서 실제 네이티브 E2E를 수행합니다. 이후 표시된 일회성 Windows VM에서 MSI와 NSIS를 각각 빌드·설치·실행·제거합니다. 번들 설치 테스트는 일반 작업 PC에서 실행을 거부합니다.

Windows 릴리스에는 Authenticode 서명이 필수입니다. 저장소 secret `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD`와 저장소 변수 `WINDOWS_TIMESTAMP_URL`을 설정해야 합니다. 릴리스를 만들기 전에 전체 Windows 검사, Rust 감사와 실제 네이티브 E2E를 다시 통과해야 합니다. 이후 PFX를 임시로 가져와 앱과 두 설치 파일을 서명·검증하고, 표시된 일회성 VM에서 서명된 MSI/NSIS를 각각 설치·실행·제거한 결과까지 통과한 파일만 업로드합니다.

Rust와 TypeScript가 공유하는 직렬화 enum 값은 `src/shared/contracts/enumValues.json`에서 관리합니다. TypeScript는 여기서 유니언 타입을 만들고, Rust 테스트는 계약 불일치를 차단합니다.

실제 Marketplace 연동 테스트는 기본 테스트에서 제외되며 다음처럼 실행할 수 있습니다.

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## 보안

Windows 네이티브 명령은 경로를 명령 셸에 삽입하지 않습니다. 설치 파일, 설치된 Studio 실행 파일과 등록된 Mendix 제거 프로그램은 Mendix 또는 Siemens가 발행한 유효한 신뢰 Authenticode 서명이 있어야 합니다. 검증한 파일은 Windows가 실행을 시작할 때까지 쓰기·삭제 공유를 막은 상태로 열어 두므로 서명 검증과 실행 사이의 파일 교체 구간도 닫습니다. Windows Installer 제거는 제품 코드에 대한 `/x` 작업과 알려진 비대화형 플래그로 제한하고, 등록 제거 프로그램은 선택한 설치본에 속하면서 허용 목록의 플래그만 사용해야 합니다. UAC 취소나 실패 종료 코드는 성공으로 처리하지 않습니다.

Linux에서는 Windows 사용자명과 암호를 앱 설정에 저장하지 않습니다. RemoteApp 실행 시 실행 중인 WinBoat 컨테이너에서 자격 증명을 읽어 FreeRDP 3의 표준 입력으로 전달합니다.

## 라이선스

Mendimaru는 [MIT 라이선스](LICENSE)로 제공됩니다.
