<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="public/mendimaru.png" alt="Mendimaru ロゴ" width="180">
</p>

<h1 align="center">Mendimaru</h1>

Mendimaru は Mendix Studio Pro の各バージョンを検出・インストール・起動・削除する Tauri GUI アプリです。Windows ではネイティブに動作し、Linux では WinBoat を使用します。

## 画面構成

- **Studio Pro**: 現在の Windows 環境にある Studio Pro の検出・起動・インストール・安全な削除
- **プロジェクト**: 設定したワークスペース内の `.mpr` プロジェクトを検出・起動
- **操作センター**: インストール・削除・起動の永続的な進行状況、失敗理由、再試行可否を確認
- **インストール待ち行列**: 検証済み部分ダウンロードの再開と複数バージョンの順序変更・取り消し・再試行・再起動復旧 ([インストール待ち行列ガイド](docs/install-queue.md))
- **設定**: Windows ネイティブワークスペースとポータブル Studio のパス、または Linux の WinBoat 環境を設定

ダッシュボード、VM リソース情報、高度なダウンロード URL、ビルド番号の手動入力は提供しません。

### 安全なプロジェクト起動

プロジェクトが必要とする正確な Studio Pro バージョンがインストール済みなら、そのまま開きます。バージョンが未導入、不明、または明示的に選んだバージョンと異なる場合、起動アシスタントが Marketplace の正確なリリースを確認し、必要ならインストールしたうえで、同じバージョンが実際に検出された場合にだけ元の `.mpr` を開きます。別のインストール済みバージョンへ暗黙に置き換えることはありません。不一致のバージョンまたはバージョン不明のプロジェクトを開くには、明示的な選択とバックアップ案内の確認が必要です。

選択したバージョンと未完了の起動意図は、キャンセル、インストール失敗、アプリ再起動後も保持され、続きから再開できます。この設定はホスト専用のアプリ設定ディレクトリに保存され、プロジェクトは正規化したパスの SHA-256 digest だけで識別します。実際のプロジェクトパスは保存しません。

### 環境診断

設定画面では、WinBoat 実行ファイル、Compose 構造、コンテナーランタイム daemon、FreeRDP、共有ワークスペースとマウント、コンテナー状態、Guest API、ゲスト時計のずれ、loopback RDP ポート、Marketplace ブラウザーを個別に確認します。失敗した項目には、再検出、Windows の起動、WinBoat を開く、関連設定へ移動するなど、明示的で安全な次の操作のみを提示します。診断レポートは JSON としてコピーまたは書き出せます。許可された状態フィールドだけを含み、設定パス、資格情報、token、コマンド payload は除外します。ゲスト時刻の同期については [WinBoat 時計同期ガイド](docs/winboat-clock-sync.md)を参照してください。

### 永続的な操作履歴

インストール・削除・起動の操作は、信頼されない共有ワークスペースの外にあるホスト専用のアプリ設定ディレクトリへアトミックに記録されます。操作センターは画面の再読み込みやアプリ再起動後も履歴を復元し、失敗した段階、安全な理由、取得できた Windows 終了コードを表示します。また、安全に再試行できる操作と、プロジェクトを選び直す必要がある保護された起動を区別します。以前のアプリプロセスで実行中だった記録は、試行ごとの HMAC キーが残っていないため古い結果を信頼せず、中断として調整します。既存の Windows レポートはファイル名だけを一度取り込み、認証できない中断記録として表示し、payload から成功を推測しません。

完了履歴の消去では、終了済みのホスト記録だけを削除します。実行中の操作、ダウンロード済みインストーラー、コマンドスクリプト、Windows レポートは削除しません。履歴スキーマには、プロジェクトパス、コマンド payload、URL、資格情報、HMAC キーを保存しません。

## Windows へのインストール

GitHub Release のアセットから MSI または NSIS セットアップをダウンロードします。Windows ビルドでは WinBoat、Docker、Guest API、RDP、FreeRDP、パス変換は不要です。

ネイティブ Windows モードでは次を行います。

- 32/64 ビットのアンインストールレジストリ、Mendix 標準フォルダー、Version Selector の情報、設定したカスタム／ポータブルパスから Studio Pro を検出
- `StudioPro.exe` を直接起動し、選択した `.mpr` パスをプロセス引数として渡す
- プロジェクトとインストーラーに Windows ネイティブパスのみを使用
- UAC を要求する前に、ダウンロードファイルの SHA-256 の安定性と Mendix／Siemens の信頼済み Authenticode 署名を検証
- 昇格したインストーラーまたは公式登録アンインストーラーの終了コードと実際の結果を確認
- 対象の `StudioPro.exe` が実行中なら削除を拒否し、公式削除情報がないポータブル版では削除を無効化

以前の設定は自動移行され、新しい `windowsStudioPaths` 一覧は既定で空のため、既存の Linux 設定もそのまま有効です。

## Arch Linux へのインストール

AUR から `mendimaru` パッケージをインストールします。

```bash
paru -S mendimaru
```

WinBoat は必須依存関係です。`winboat` 依存関係を満たすパッケージがない場合、
`paru` は AUR の `winboat` を自動的にインストールします。`winboat`、
`winboat-bin`、`winboat-electron`、`winboat-git` のいずれかがすでに
インストールされている場合は、そのパッケージをそのまま使用し、再インストール
しません。新しいシステムで別のパッケージを選ぶには、
`paru -S winboat-bin mendimaru` のように同じコマンドで指定できます。

Mendix Marketplace からインストール可能な Studio Pro バージョンを検索するには Chromium または Google Chrome も必要で、両方を任意のブラウザー依存関係として宣言しています。

## WinBoat の初期設定

WinBoat がインストール済みで Windows VM がまだ構成されていない場合、Mendimaru の **WinBoat セットアップを開始** ボタンから公式の WinBoat セットアップウィザードを開けます。Windows アカウント、VM リソース、Windows イメージ、Guest Server のインストールは WinBoat が処理します。

Mendimaru はウィザードが完了するまで状態を監視し、その後、次の作業を自動的に行います。

- AUR の `winboat-bin` が使用する `/opt/winboat/winboat` を含め、WinBoat の実行ファイルを検出
- `~/.winboat/docker-compose.yml` または `podman-compose.yml` を検出
- 実行中のコンテナから Guest API と RDP に動的に割り当てられた実際のホストポートを検出
- 設定した Linux ワークスペースを Compose ファイルの `/shared` マウントに適用
- 元の Compose ファイルを `*.mendimaru.bak` としてバックアップし、仮想ディスクを維持したままコンテナを一度再作成

初期設定をキャンセルした場合やウィンドウを閉じた場合は、**セットアップを続行** を選ぶと公式ウィザードを再度開けます。Mendimaru が Windows のユーザー名やパスワードを独自の設定にコピーすることはありません。

## 多言語対応

英語（`en-US`）、韓国語（`ko-KR`）、日本語（`ja-JP`）に対応しています。初期値にはシステム言語が使用されます。ヘッダーの言語メニューで選んだ言語はアプリ設定に保存され、次回の起動時にも使用されます。未対応のシステム言語では英語にフォールバックします。

翻訳とロケール依存の処理は Rust バックエンドが担当します。

- UI テキストとバックエンドのエラーメッセージは、`src-tauri/i18n/<locale>/mendimaru.ftl` の Fluent リソースで一元管理します。
- `i18n-embed` が翻訳リソースを実行ファイルに埋め込み、システム言語の選択と英語へのフォールバックを処理します。
- 日付、数値、ダウンロードサイズは、ICU4X でフォーマットしてからフロントエンドに渡します。
- フロントエンドはバックエンドから渡された翻訳バンドルを表示し、翻訳された文言から状態を判定しません。ダウンロードのキャンセルなど、動作に影響する値は個別のコードと状態として渡されます。
- テストでは、全言語の翻訳キーと変数の構成が一致すること、および React が使用する静的な翻訳キーがすべてバックエンドのバンドルに含まれることを確認します。

言語を追加するには、`src-tauri/src/i18n.rs` の対応ロケール一覧に BCP 47 言語タグと表示名を登録し、英語ファイルと同じキーおよび変数を持つ Fluent ファイルを `src-tauri/i18n/<locale>/mendimaru.ftl` に追加します。新しい UI テキストは 3 つの Fluent ファイルと `src/shared/contracts/uiMessages.json` に追加してください。このレジストリは TypeScript の翻訳キー型と Rust の UI バンドルの両方で使用され、`cargo test` により翻訳の欠落や変数の不一致を検出できます。

## Studio Pro バージョンの検索とインストール

`kirakiraichigo-mendix-manager` と同じ方法で、[Mendix Marketplace の Studio Pro ページ](https://marketplace.mendix.com/link/studiopro)にあるデータグリッドを Chromium で読み取ります。

- 最初の 10 バージョンを自動的に更新し、**以前のバージョンをさらに読み込む** を選ぶと次のページを取得します。
- 一覧をアプリのキャッシュディレクトリにある `studio-version-catalog.json` に保存し、次回起動時に先に表示します。
- リリース日とともに Latest、LTS、MTS、Beta のラベルを取得します。
- Studio Pro 11 以降では公式の `Mendix-<version>-Setup.exe` アーティファクトを使用します。
- Studio Pro 10 以前では、バージョン詳細ページから `Build <number>` を自動的に抽出し、`Mendix-<version>.<build>-Setup.exe` を使用します。
- 一覧からバージョンを選ぶだけでよく、URL やビルド番号を入力する必要はありません。
- ダウンロード済みインストーラーは、記録された配布元、想定サイズ、Windows PE 構造、SHA-256 がすべて一致する場合にのみ再利用します。メタデータのない旧キャッシュや変更されたキャッシュは削除して再ダウンロードします。
- 未インストールの各カタログバージョンには、既存キャッシュを再利用せずインストール失敗から復旧するための強制再ダウンロード操作があります。

Windows ではシステムおよびユーザーの標準場所にある Microsoft Edge と Chrome を検出します。Linux では `MENDIMARU_CHROME_PATH`、`google-chrome-stable`、`google-chrome`、`chromium`、`chromium-browser` の順に検出します。

## Windows のパス

レジストリと Version Selector の情報に加えて、次の既定場所も検出します。

| 用途 | Windows パス |
| --- | --- |
| Studio Pro のインストールルート | `C:\Program Files\Mendix` |
| Studio Pro の実行ファイル | `C:\Program Files\Mendix\<version>\modeler\studiopro.exe` |
| Studio Pro のアンインストール情報 | `C:\ProgramData\Mendix` |
| ネイティブの既定ワークスペース | `%USERPROFILE%\Mendix`（存在しない場合は別のディレクトリを指定するよう案内） |
| Linux WinBoat の共有パス | `\\host.lan\Data` |

インストーラーは設定したワークスペース内の `.mendimaru/installers` に保存されます。ネイティブモードでは署名を検証し、コマンドシェルを使わず Windows の昇格 API で起動します。Linux モードでは UTF-16LE でエンコードした PowerShell コマンドを WinBoat RemoteApp に渡します。プロセスが正常終了し、対象バージョンの `StudioPro.exe` が検出されて初めて完了と判定します。

Marketplace カタログのキャッシュは最大 6 時間再利用されるため、通常の起動時にバックグラウンドブラウザーを再起動しません。すぐに更新する場合はカタログの更新操作を使用してください。

削除についても同様に、Windows のアンインストーラーが終了し、対象バージョンの `StudioPro.exe` がなくなったことを確認した後、インストール済みバージョンの一覧を自動的に更新します。

WinBoat Guest が `apps-query-v1` を通知する場合、Mendimaru は WinBoat の非公開共有トークンで認証し、設定された Mendix ルート配下からアイコンを含まない `Name`、`Path`、`Source` フィールドだけを要求します。古い Guest ではサイズ制限付きの完全な `/apps` 応答へ安全にフォールバックしますが、軽量 capability が通知された後の認証、タイムアウト、検証エラーを完全な経路へ暗黙にダウングレードすることはありません。詳細は [WinBoat Studio の検出](docs/winboat-studio-discovery.md) を参照してください。

Linux WinBoat モードでは、Studio Pro の起動ボタンは Windows プロセスが実際のウィンドウを作成し、FreeRDP が表示できる状態になるまで無効のままです。WinBoat が保持できる接続済み Studio Pro RemoteApp は一度に一つだけなので、接続中のセッションがある場合は、Studio Pro 画面でそのセッションを終了するまで、新しい Studio の起動、プロジェクトを開く操作、インストール、削除をロックします。起動準備中も同じ操作をロックして重複起動を防ぎます。Windows は共有操作スクリプトのハッシュを固定し、一意の専用パスへコピーしてそのコピーだけを実行します。インストールと削除は、すでに管理者権限を持つ WinBoat セッションのトークンを継承するため、別の UAC ウィンドウは表示されません。

## Linux 共有ワークスペース

Linux 共有ディレクトリは、WinBoat の Compose ファイルにある `<host path>:/shared` マウントに接続されます。プロジェクト一覧はこのディレクトリだけを走査し、`.git`、`node_modules`、`deployment`、`.mendix-cache`、`.mendimaru` などの生成ディレクトリやキャッシュディレクトリを除外します。

プロジェクト検出は UI backend の経路を占有せず、深さ 8、最大 100,000 entry、10,000 プロジェクト、`project-settings.user.json` ごと 256 KiB、全体 16 MiB、5 秒の制限内で実行します。より大きなツリーや読み取れない・通常ファイルではない settings は、静かな成功ではなく skipped/error 数を含む部分結果になり、UI は最初の 100 件を表示して追加読み込みを提供します。watcher イベントは debounce・統合され、watcher を利用できない場合は 30 秒の fallback（watcher 安全網は 5 分）で更新します。古いワークスペースの応答が新しい結果を上書きすることはありません。お気に入りと最終起動時刻はハッシュ化したプロジェクト identity のみ保存し、検出されなくなったお気に入りは自動的に整理します。

Projects 画面では、このワークスペース外の `.mpr` を一つ明示的に選択して開くこともできます。Mendimaru は選択パスを canonicalize し、ファイルまたは親パスの symlink を拒否してから、`.mpr` の直接の親ディレクトリだけを、同じ retained RemoteApp 接続の書き込み可能なセッション単位 FreeRDP drive として接続します。プロジェクトのコピー、同期、Compose への追加、通常のプロジェクト一覧への永続登録は行いません。Windows から redirected `.mpr` が実ファイルとして見えるまで最大 30 秒確認してから、正確な Studio Pro を起動するため、保存内容は元の Linux プロジェクトへ反映されます。

共有名はパス digest から生成した長さ制限付き ASCII 値で、ホストディレクトリ名を公開しません。GUI 選択結果には、その digest から生成した範囲制限付きの現在プロセストークンだけを渡します。生の Linux パスは直列化された選択・起動 DTO、operation history、session DTO、Windows report、診断、ログに記録しません。Studio Pro を終了すると retained FreeRDP プロセスと一時 drive も終了します。アプリまたは RemoteApp 接続が先に終了して一時 drive が失われたセッションを、自動的に再接続可能とは表示しません。そのセッションを停止するか、同じ `.mpr` を再選択して新しい保護された起動を開始してください。コンマ、バックスラッシュ、改行、非 UTF-8 パス、ファイルシステムのルート、ホーム全体、読み取り専用プロジェクトディレクトリは、安全に表現または範囲制限できないため起動前に拒否します。

ネイティブ FreeRDP プロセスには、選択したディレクトリを読み取る権限が必要です。sandbox／Flatpak の FreeRDP wrapper を使う場合は、パッケージシステムのファイルシステム権限でそのディレクトリだけを明示的に許可し、もう一度選択してください。Mendimaru は解決のためにホーム全体を共有しません。Windows セッションへ書き込み可能な drive を提供するため、現在のディストリビューションでセキュリティ修正済みの FreeRDP 3 を使用してください。

共有ディレクトリを変更すると、既存の Compose ファイルを `*.mendimaru.bak` としてバックアップします。設定画面で変更をすぐに適用するよう選択すると、`/storage` 仮想ディスクとインストール済みの Windows アプリを維持したまま WinBoat コンテナを再作成します。

## Backend capability contract

エージェントと CI は GUI を起動せずに、プラットフォーム中立の backend contract を確認できます。

```bash
mendimaru capabilities --json
```

応答は host、Studio、任意の Runtime platform を区別し、Studio、Runtime、UI automation、browser の全操作を supported または unsupported として報告します。明示的な `--backend` は現在の host と一致する必要があり、別の backend へ暗黙に fallback しません。詳細は [Platform backend and capability contract](docs/backend-contract.md) と機械可読な [JSON Schemas](schemas/) を参照してください。

### Headless CLI

インストール済みの `mendimaru` 実行ファイルは、Tauri やダイアログを起動せずに、環境の確認・準備、Studio Pro の正確なバージョンの一覧・インストール・削除・起動、Studio セッションの照会・停止、opaque project ID の解決、永続 operation の照会・再試行を実行できます。結果 JSON は stdout、エラー JSON は stderr に分離され、`--ndjson` は構造化された進捗イベントを追加します。`--timeout-seconds` と `Ctrl+C` は共有 operation 境界でキャンセルし、中断された処理は operation ID で再照会できます。全コマンド、終了コード、schema、安全規則は [Headless CLI contract](docs/headless-cli.md) を参照してください。

Linux の `browser test` は、明示 URL、Portable Runtime session、または WinBoat Run Locally session に対して、同じ宣言型 Playwright/Chromium suite を実行します。browser の download は明示操作に限定され、失敗時には mask 済み HTML、DOM/accessibility、screenshot、trace、console、network 証跡を上限付き retention policy で保存します。詳細は [browser testing guide](docs/browser-testing.md) を参照してください。

## 開発

開発には Node.js 22.22.2 以降、Rust、ホスト向け Tauri システム依存パッケージが必要です。Linux 統合には WinBoat、Docker または Podman、FreeRDP 3、Chrome／Chromium が追加で必要で、Windows の一覧取得には Edge または Chrome を使用します。

```bash
npm install
npm run tauri dev
```

`npm run tauri dev` の実行時に `MENDIMARU_STUDIO_TRACE=1` を設定すると、Studio overview の区間時間と request coalescing 診断を出力できます。trace には時間、payload size、項目数だけが含まれ、設定 path や Guest の raw payload は記録されません。

ホストに依存しない検証とアプリバンドルの生成：

```bash
npm run check:portable
npm run test:browser
npm run test:e2e
npm run test:e2e:coverage
npm run check:windows
npm run tauri build
```

Linux の `npm run test:e2e` は、固定バージョンの `tauri-driver` と `WebKitWebDriver` を使い、debug 実行ファイルを Vite development URL に接続します。隔離した WinBoat／API／project fixture により、実際の WebView、Tauri IPC、online application state、project discovery、CSP enforcement、hostile-input rejection、frame sampling を伴う持続的な online-route motion と busy 中だけの motion、allowlist 外の idle animation がないこと、主要画面の navigation、および startup／IPC／navigation／private-memory／idle-CPU budget を検証します。driver bridge は `cargo install tauri-driver --version 2.0.6 --locked` でインストールし、host に `WebKitWebDriver` も必要です。`npm run test:app-flow` は OS 境界をモックした高速な React application-flow suite で、`npm run test:browser` は Mendimaru desktop shell ではなく Mendix Runtime page をテストします。CI は 3 層を gate し、Linux E2E の測定値と screenshot を保存します。motion の完全な一覧と変更規則は [Motion contract](docs/motion-contract.md) に記載されています。

`npm run test:e2e:coverage` は、リポジトリ内の E2E coverage model を検証します。Linux と Windows は実 desktop の core functional／security／performance gate では同等ですが、**platform 全体ではまだ同等ではありません**。hosted Linux CI には実 WinBoat lifecycle、MSI／NSIS に対応する AUR／package の install-launch-uninstall lifecycle、live Marketplace refresh がありません。生成される report は `artifacts/e2e/e2e-coverage.json` です。

Windows の `npm run test:e2e:windows` は、テスト専用 Cargo feature で実際のアプリを `tauri dev` から起動し、ネイティブ WebView2 ウィンドウを組み込み WebDriver で操作します。実際の IPC、registry discovery、project scan、settings persistence、Edge ベースの Marketplace refresh、CSP enforcement、hostile input rejection、performance budget を検証します。設定とキャッシュは safety marker 付き一時ディレクトリに隔離し、終了後に削除します。WebDriver feature、permission、global Tauri bridge は通常の development／release build には含まれません。

CI は npm と Cargo の lockfile を監査し、Windows native E2E を実行します。その後、marker 付きの一時 Windows VM で MSI と NSIS をそれぞれ build、install、launch、uninstall します。installer lifecycle script は通常の workstation では実行を拒否します。`WINDOWS_CERTIFICATE`、`WINDOWS_CERTIFICATE_PASSWORD`、`WINDOWS_TIMESTAMP_URL` がすべて設定されている場合は、全 Windows artifact を署名・timestamp 処理し、Authenticode を検証してから upload します。3 項目がすべて未設定の場合は同じ lifecycle check を通過した installer を公開し、GitHub Release 本文と `WINDOWS-BUILDS-UNSIGNED.txt` asset に未署名であることを明示します。一部だけ設定された不完全な signing configuration は release を失敗させます。

online の実 WinBoat VM に対する非破壊 RemoteApp gate は `npm run test:winboat-smoke` で実行し、認証済み session query と stale-session rejection を検証します。Linux の完全な `npm run check` は、実際の状態を変更する lifecycle gate を黙って除外せず、必須で実行します。未インストールの disposable version と明示的な変更許可を指定してください。

```bash
MENDIMARU_E2E_ALLOW_MUTATION=1 \
MENDIMARU_E2E_VERSION=11.13.0 \
npm run check
```

同じ環境変数で `npm run test:winboat-e2e` を実行すると、lifecycle だけを個別に検証できます。指定した disposable version の公式 installer が shared cache に存在する必要があります。このテストは既にインストール済みの対象を拒否し、absent → installed → 実 Studio window → running removal rejection → graceful close → uninstalled を実行して、progress ordering、正確な process identity、既存 installation と installer cache の不変性、stale／repeated action rejection、leaked process や想定外の RemoteApp／PowerShell window がないことまで検証します。両方の live gate は隔離した Xvfb と `xvfb-run`、`xfwm4`、`wmctrl` を必要とします。Arch Linux では `xorg-server-xvfb` が `xvfb-run` を提供します。他の host platform では WinBoat lifecycle を適用対象外として報告します。hosted CI には live WinBoat VM がないため、portable component gate のみを実行し、local live result の通過を主張しません。

Rust の全テストでは registry parsing、path containment、file integrity、Windows argument encoding、UAC／exit-code failure、install から uninstall までの fixture lifecycle を検証します。

Rust と TypeScript で共有するシリアライズ済み enum 値は `src/shared/contracts/enumValues.json` で管理します。TypeScript はこのレジストリから union 型を導出し、Rust テストが契約のずれを検出します。

実際の Marketplace と連携するテストは、既定のテスト実行から除外されています。次のコマンドで実行できます。

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## セキュリティ

Windows ネイティブのコマンドはパスをコマンドシェルへ挿入しません。インストーラー、インストール済みの Studio 実行ファイル、および登録済みの Mendix アンインストーラーには、Mendix または Siemens が発行した有効で信頼済みの Authenticode 署名が必要です。検証した実行ファイルは Windows が起動するまで書き込み／削除共有を拒否したまま開いておくため、署名検証と実行の間の置換も防ぎます。Windows Installer による削除は製品コードへの `/x` 操作と既知の非対話フラグに限定し、登録アンインストーラーは選択したインストールに属し、許可リスト内のフラグだけを使用する必要があります。UAC のキャンセルや失敗終了コードを成功として扱いません。

Linux では Windows のユーザー名とパスワードをアプリ設定に保存しません。RemoteApp 起動時に実行中の WinBoat コンテナから認証情報を読み、FreeRDP 3 の標準入力へ渡します。FreeRDP はアプリ専用の TOFU 証明書ピンを使い、管理者権限の操作は Guest API と RDP がループバックだけにバインドされている場合に限定します。共有操作結果と保持中の Studio session-control request は、試行ごとの HMAC キーとリプレイ防止シーケンスで認証します。

脅威モデル、実行ファイルの信頼チェーン、コンテナ権限と残存リスク、報告方法については、[セキュリティポリシーと WinBoat 信頼境界](SECURITY.md)を参照してください。

## ライセンス

Mendimaru は [MIT ライセンス](LICENSE)の下で提供されます。
