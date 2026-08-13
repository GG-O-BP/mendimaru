<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="public/mendimaru.png" alt="Mendimaru ロゴ" width="180">
</p>

<h1 align="center">Mendimaru</h1>

Mendimaru は、WinBoat を介して Linux 上に Mendix Studio Pro をインストールし、使用するバージョンを選んで起動し、共有ワークスペース内のプロジェクトを開くための Tauri GUI アプリです。

## 画面構成

- **Studio Pro**: WinBoat の Windows 環境にインストール済みのバージョンの起動・削除、および Mendix Marketplace で実際に入手できるバージョンの検索・インストール
- **プロジェクト**: 設定した Linux 共有ディレクトリ内の `.mpr` プロジェクトを検出・起動
- **設定**: WinBoat の実行ファイル、Compose ファイル、Docker または Podman、Linux 共有ディレクトリを設定

ダッシュボード、VM リソース情報、高度なダウンロード URL、ビルド番号の手動入力、強制再ダウンロードのオプションは提供しません。

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

言語を追加するには、`src-tauri/i18n.rs` の対応ロケール一覧に BCP 47 言語タグと表示名を登録し、英語ファイルと同じキーおよび変数を持つ Fluent ファイルを `src-tauri/i18n/<locale>/mendimaru.ftl` に追加します。新しい UI テキストは 3 つの Fluent ファイルと `UI_MESSAGE_KEYS` に追加してください。`cargo test` により、翻訳の欠落や変数の不一致を検出できます。

## Studio Pro バージョンの検索とインストール

`kirakiraichigo-mendix-manager` と同じ方法で、[Mendix Marketplace の Studio Pro ページ](https://marketplace.mendix.com/link/studiopro)にあるデータグリッドを Chromium で読み取ります。

- 最初の 10 バージョンを自動的に更新し、**以前のバージョンをさらに読み込む** を選ぶと次のページを取得します。
- 一覧をアプリのキャッシュディレクトリにある `studio-version-catalog.json` に保存し、次回起動時に先に表示します。
- リリース日とともに Latest、LTS、MTS、Beta のラベルを取得します。
- Studio Pro 11 以降では公式の `Mendix-<version>-Setup.exe` アーティファクトを使用します。
- Studio Pro 10 以前では、バージョン詳細ページから `Build <number>` を自動的に抽出し、`Mendix-<version>.<build>-Setup.exe` を使用します。
- 一覧からバージョンを選ぶだけでよく、URL やビルド番号を入力する必要はありません。

Chrome は `MENDIMARU_CHROME_PATH`、`google-chrome-stable`、`google-chrome`、`chromium`、`chromium-browser` の順に検出されます。

## Windows のパス

参照アプリと同じパスを使用します。

| 用途 | Windows パス |
| --- | --- |
| Studio Pro のインストールルート | `C:\Program Files\Mendix` |
| Studio Pro の実行ファイル | `C:\Program Files\Mendix\<version>\modeler\studiopro.exe` |
| Studio Pro のアンインストール情報 | `C:\ProgramData\Mendix` |
| 既定の共有パス | `\\host.lan\Data` |

インストーラーは Linux 共有ディレクトリ内の `.mendimaru/installers` に保存されます。Windows では、共有パスに対する非表示のセキュリティ警告によってインストールが停止しないよう、ファイルをローカルの一時ディレクトリにコピーし、ブロックを解除してから実行します。引用符の影響を避けるため、PowerShell コマンドは UTF-16LE でエンコードして WinBoat RemoteApp に渡します。インストーラーが正常終了し、対象バージョンの `StudioPro.exe` が作成されたことを確認して初めて、インストール完了と判定します。

削除についても同様に、Windows のアンインストーラーが終了し、対象バージョンの `StudioPro.exe` がなくなったことを確認した後、インストール済みバージョンの一覧を自動的に更新します。

Studio Pro の起動ボタンは、Windows プロセスが実際のウィンドウを作成し、FreeRDP が表示できる状態になるまで無効のままです。起動準備中は、重複起動を防ぐため、ほかのバージョンやプロジェクトの起動ボタンもロックされます。起動スクリプトは共有フォルダーに保存し、短い呼び出しコマンドだけを RemoteApp に渡すことで、FreeRDP RAIL のコマンド長制限を超えないようにしています。Windows Script Host が PowerShell を非表示モードで実行します。インストールと削除は、すでに管理者権限を持つ WinBoat セッションのトークンを継承するため、PowerShell コンソールや別の UAC ウィンドウは表示されません。

## 共有ワークスペース

Linux 共有ディレクトリは、WinBoat の Compose ファイルにある `<host path>:/shared` マウントに接続されます。プロジェクト一覧はこのディレクトリだけを走査し、`.git`、`node_modules`、`deployment`、`.mendix-cache`、`.mendimaru` などの生成ディレクトリやキャッシュディレクトリを除外します。

共有ディレクトリを変更すると、既存の Compose ファイルを `*.mendimaru.bak` としてバックアップします。設定画面で変更をすぐに適用するよう選択すると、`/storage` 仮想ディスクとインストール済みの Windows アプリを維持したまま WinBoat コンテナを再作成します。

## 開発

開発には Node.js、Rust、Tauri の Linux 向けシステム依存パッケージ、WinBoat、Docker または Podman、FreeRDP 3、Google Chrome または Chromium が必要です。

```bash
npm install
npm run tauri dev
```

プロジェクトの検証とアプリバンドルの生成：

```bash
npm run check
cd src-tauri && cargo clippy --all-targets -- -D warnings
npm run tauri build
```

実際の Marketplace と連携するテストは、既定のテスト実行から除外されています。次のコマンドで実行できます。

```bash
cd src-tauri
cargo test marketplace::tests::live_ -- --ignored --nocapture
```

## セキュリティ

Windows のユーザー名とパスワードはアプリ設定に保存しません。RemoteApp の起動時に、実行中の WinBoat コンテナから認証情報を読み取り、FreeRDP 3 の標準入力に渡すため、パスワードがプロセス引数やアプリログに露出することはありません。

## ライセンス

Mendimaru は [MIT ライセンス](LICENSE)の下で提供されます。
