# mendimaru

WinBoat를 통해 Linux에서 Mendix Studio Pro를 설치하고, 버전을 선택해 실행하며, 공유 워크스페이스의 프로젝트를 여는 Tauri GUI 앱입니다.

## 화면 구성

- **Studio Pro**: WinBoat Windows에 설치된 버전 실행·제거, Mendix Marketplace의 실제 설치 가능 버전 조회·설치
- **프로젝트**: 설정한 Linux 공유 디렉터리 안의 `.mpr` 프로젝트 탐지·실행
- **설정**: WinBoat 실행 파일, Compose 파일, Docker/Podman, Linux 공유 디렉터리 지정

대시보드, VM 자원 정보, 고급 다운로드 URL, 수동 빌드 번호, 강제 재다운로드 옵션은 제공하지 않습니다.

## 다국어 지원

영어(`en-US`), 한국어(`ko-KR`), 일본어(`ja-JP`)를 지원합니다. 기본값은 시스템 언어이며, 헤더의 언어 선택 메뉴에서 바꾸면 앱 설정에 저장되어 다음 실행에도 유지됩니다. 지원하지 않는 시스템 언어는 영어로 대체됩니다.

번역과 로케일 처리는 Rust 백엔드가 담당합니다.

- 화면 문구와 백엔드 오류 문구는 `src-tauri/i18n/<locale>/mendimaru.ftl`의 Fluent 리소스에서 함께 관리합니다.
- `i18n-embed`가 번역 리소스를 실행 파일에 포함하고 시스템 언어 선택과 영어 폴백을 처리합니다.
- 날짜, 숫자, 다운로드 용량은 ICU4X로 형식화한 값만 프런트엔드에 전달합니다.
- 프런트엔드는 백엔드가 전달한 번역 번들을 표시하며, 문구를 기준으로 상태를 판별하지 않습니다. 다운로드 취소처럼 동작에 영향을 주는 값은 별도 코드와 상태로 전달합니다.
- 테스트는 모든 언어의 번역 키·변수 구성이 같은지, React에서 사용하는 정적 번역 키가 백엔드 번들에 포함됐는지 확인합니다.

언어를 추가할 때는 `src-tauri/i18n.rs`의 지원 로케일 목록에 BCP 47 언어 태그와 표시 이름을 등록하고, 기존 영어 파일과 키·변수 구성이 같은 Fluent 파일을 `src-tauri/i18n/<locale>/mendimaru.ftl`에 추가합니다. 새 화면 문구는 Fluent 파일 세 곳과 `UI_MESSAGE_KEYS`에 추가합니다. `cargo test`가 빠진 번역이나 변수 불일치를 검출합니다.

## Studio Pro 버전 조회와 설치

`kirakiraichigo-mendix-manager`와 같은 방식으로 [Mendix Marketplace Studio Pro 페이지](https://marketplace.mendix.com/link/studiopro)의 데이터그리드를 Chromium으로 읽습니다.

- 첫 10개 버전을 자동으로 갱신하고 **이전 버전 더 불러오기**로 다음 페이지를 가져옵니다.
- 목록은 앱 캐시 디렉터리의 `studio-version-catalog.json`에 저장해 다음 실행 때 먼저 표시합니다.
- 최신, LTS, MTS, Beta 표시와 출시일을 함께 가져옵니다.
- Studio Pro 11 이상은 `Mendix-<version>-Setup.exe` 공식 아티팩트를 사용합니다.
- Studio Pro 10 이하는 버전 상세 페이지에서 `Build <number>`를 자동 추출해 `Mendix-<version>.<build>-Setup.exe`를 사용합니다.
- 사용자는 목록에서 버전을 고르기만 하면 되며 URL이나 빌드 번호를 입력하지 않습니다.

Chrome 탐지 순서는 `MENDIMARU_CHROME_PATH`, `google-chrome-stable`, `google-chrome`, `chromium`, `chromium-browser`입니다.

## Windows 경로

참조 앱과 동일한 경로를 사용합니다.

| 용도 | Windows 경로 |
| --- | --- |
| Studio Pro 설치 루트 | `C:\Program Files\Mendix` |
| Studio Pro 실행 파일 | `C:\Program Files\Mendix\<version>\modeler\studiopro.exe` |
| Studio Pro 제거 정보 | `C:\ProgramData\Mendix` |
| 기본 공유 경로 | `\\host.lan\Data` |

설치 파일은 Linux 공유 디렉터리의 `.mendimaru/installers`에 저장합니다. Windows에서는 공유 경로의 보안 경고가 숨은 상태로 설치를 막지 않도록 파일을 로컬 임시 디렉터리로 복사하고 차단을 해제한 뒤 실행합니다. WinBoat RemoteApp에는 따옴표 영향을 받지 않는 UTF-16LE 인코딩 PowerShell 명령으로 전달하며, 설치 프로세스 종료 코드가 성공이고 해당 버전의 `StudioPro.exe`가 생성된 뒤에만 완료로 처리합니다.

제거할 때도 Windows 제거 프로세스가 끝나고 해당 버전의 `StudioPro.exe`가 사라진 것을 확인한 뒤 설치된 버전 목록을 자동으로 갱신합니다.

Studio Pro 실행 버튼은 Windows 프로세스의 실제 창이 생성되고 FreeRDP가 표시할 준비를 마칠 때까지 비활성화됩니다. 실행 준비 중에는 다른 버전과 프로젝트의 실행 버튼도 잠겨 중복 실행을 방지합니다. 실행 스크립트는 공유 폴더에 저장하고 짧은 호출 명령만 RemoteApp으로 전달해 FreeRDP RAIL의 명령 길이 제한을 넘지 않습니다. Windows Script Host가 PowerShell을 숨김 모드로 실행하며, 설치·제거는 이미 관리자 권한인 WinBoat 세션의 토큰을 상속하므로 PowerShell 콘솔이나 별도의 UAC 창을 표시하지 않습니다.

## 공유 워크스페이스

Linux 공유 디렉터리는 WinBoat Compose의 `<host path>:/shared` 마운트와 연결됩니다. 프로젝트 목록은 이 디렉터리만 탐색하며 `.git`, `node_modules`, `deployment`, `.mendix-cache`, `.mendimaru` 같은 생성·캐시 디렉터리는 제외합니다.

공유 디렉터리를 바꾸면 기존 Compose 파일을 `*.mendimaru.bak`으로 백업합니다. 설정에서 즉시 적용을 선택하면 WinBoat 컨테이너를 다시 만들지만 `/storage` 가상 디스크와 설치된 Windows 앱은 유지됩니다.

## 개발

필수 환경은 Node.js, Rust, Tauri의 Linux 시스템 의존성, WinBoat, Docker 또는 Podman, FreeRDP 3, Google Chrome 또는 Chromium입니다.

```bash
npm install
npm run tauri dev
```

검증과 번들 생성:

```bash
npm run check
cd src-tauri && cargo clippy --all-targets -- -D warnings
npm run tauri build
```

실제 Marketplace 연동 테스트는 기본 테스트에서 제외되며 다음처럼 실행할 수 있습니다.

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## 보안

Windows 사용자명과 암호는 앱 설정에 저장하지 않습니다. RemoteApp 실행 시 실행 중인 WinBoat 컨테이너에서 자격 증명을 읽어 FreeRDP 3의 표준 입력으로 전달하므로 프로세스 인자나 앱 로그에 암호가 노출되지 않습니다.
