# CommitWatch

Windows のタスクマネージャでは気づきにくい **コミット不足**（RAM は空いているのにメモリ確保が失敗する状態）を、
トレイ常駐で見張るツールです。Claude Code・Orca・Docker Desktop（WSL2）を併用する開発機向けに作りました。

- Commit Charge / Commit Limit / Commit Usage を 1 秒ごとに取得し、トレイアイコンの色（緑・黄・赤）で状態を表示
- Available / Free / Standby、ページファイル、`vmmem`（WSL2 / Docker）の確保量、カーネル側の確保量を表示
- Process Commit 上位プロセス、CPU Load、実クロック、ACPI 温度
- PawnIO ドライバがあればコア温度・TjMax・VID・パッケージ電力（Intel、管理者権限が必要）
- 1 分ごとの CSV ログ（Excel でそのまま開ける）
- GitHub Releases からの自動更新（minisign 署名検証、適用は手動）

## インストール

1. [Releases](https://github.com/mazume-tech-club/dev-taskmanager/releases/latest) から `commitwatch.exe` をダウンロードします。
2. 任意の場所に置いて実行します。トレイに常駐し、ウィンドウが開きます。
3. Windows 起動時に自動で常駐させるには、トレイアイコンを右クリック → 「Windows 起動時に開始」を選びます。

SmartScreen の警告が出た場合は「詳細情報」→「実行」を選んでください（コード署名証明書は付けていません）。

常駐プロセスは約 10 MB、ウィンドウを開いている間だけ約 45 MB を使います。

## 使い方

| 操作 | 内容 |
|------|------|
| トレイアイコン左クリック | ウィンドウを開く／前面に出す |
| トレイアイコン右クリック | 表示、ログフォルダを開く、Windows 起動時に開始、更新を確認、終了 |
| ウィンドウ「トレイへ」 | ウィンドウを閉じる（監視は続く） |
| ウィンドウ「コンパクト」 | 最前面の小さな表示に切り替える |
| 設定タブ | しきい値、ログ、更新、センサー、自動起動を変更して保存 |

Commit Usage が 85% で警告（黄）、95% で危険（赤）になり、危険に入ったときに 1 回だけバルーン通知を出します。
しきい値は設定タブで変更できます。

## コア温度（任意）

コア温度は CPU のモデル固有レジスタを読む必要があり、Windows ではカーネルドライバと管理者権限が要ります。
本アプリは署名済みの汎用ドライバ [PawnIO](https://pawnio.eu/) を使います。

1. [PawnIO_setup.exe](https://github.com/namazso/PawnIO.Setup/releases/latest/download/PawnIO_setup.exe) をインストールします。
2. トレイアイコンを右クリック → 「管理者権限で再起動（温度センサーを有効化）」を選びます。
3. ウィンドウの CPU タブにコア温度・TjMax・電力が表示されます。

管理者権限で動かしている間に「Windows 起動時に開始」を有効にすると、タスクスケジューラに最上位特権で登録され、
次回以降は UAC なしで常駐します。対応 CPU は現在 Intel のみです。

## ログ

`%LOCALAPPDATA%\CommitWatch\logs\YYYY-MM-DD.csv` に 1 分 1 行で記録します（UTF-8 BOM、CRLF）。
各指標の平均・最大、CPU Load、温度、Process Commit 上位 5 プロセスが入ります。30 日より古いファイルは自動で削除します。

## 設定ファイル

`%LOCALAPPDATA%\CommitWatch\config.toml`。無ければ既定値で動きます。設定タブからも編集できます。

```toml
[alert]
warning_commit_usage = 0.85
critical_commit_usage = 0.95
warning_temp_margin_c = 15.0
critical_temp_margin_c = 5.0

[log]
enabled = true
retention_days = 30

[update]
enabled = true
check_interval_hours = 24

[general]
top_processes = 5
process_interval_secs = 5
history_secs = 600

[sensors]
enabled = true
```

## コマンドライン

| 引数 | 動作 |
|------|------|
| （なし） | 常駐を開始してウィンドウを開く。すでに常駐していればウィンドウを前面に出す |
| `--hidden` | ウィンドウを開かずに常駐（自動起動用） |
| `--probe` | 現在の値を 1 回表示して終了（診断） |
| `--headless [秒]` | ウィンドウなしで監視とログを実行（診断） |
| `--check-update` | 更新を確認してダウンロード・検証まで行う（適用はしない） |
| `--apply-update` | 準備済みの更新を適用して再起動する |

## 開発

```powershell
cargo build --release
cargo test
cargo run -- --probe
```

- Rust stable、edition 2024。GUI は egui / eframe、トレイは Win32 API。
- 設計の決定事項は `docs/design.md`、用語は `GLOSSARY.md`、理由を残す決定は `docs/adr/` にあります。
- リリースは `Cargo.toml` の `version` を上げて `vX.Y.Z` タグを push すると GitHub Actions が行います
  （署名鍵の扱いは `docs/design.md` を参照）。

## ライセンスと同梱物

- 同梱フォント: [BIZ UDGothic](https://github.com/googlefonts/morisawa-biz-ud-gothic)（SIL Open Font License 1.1、
  `assets/fonts/OFL.txt`）。JIS 第 1 水準までに絞って埋め込んでいます。
- 同梱モジュール: [PawnIO.Modules](https://github.com/namazso/PawnIO.Modules) の `IntelMSR`（LGPL-2.1、
  `assets/pawnio/COPYING`）。PawnIO ドライバ本体（GPL-2.0）は同梱せず、利用者がインストールします。
