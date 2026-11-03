# CommitWatch 設計の決定事項

2026-11-03 の設計インタビューで確定した内容。用語は `GLOSSARY.md`、理由を残す決定は `docs/adr/` を参照。

## 目的

Claude Code・Orca・Docker を併用すると、RAM は空いているのに Commit Charge が Commit Limit に迫って
メモリ確保が失敗する。Windows 標準のタスクマネージャではこれに気づけないので、Commit Shortage を
数値・グラフ・ログ・通知で見えるようにする常駐アプリを作る。

## 名前

| 項目 | 値 |
|------|-----|
| 製品名 | CommitWatch |
| 実行ファイル | `commitwatch.exe` |
| データフォルダ | `%LOCALAPPDATA%\CommitWatch\` |
| GitHub | `mazume-tech-club/dev-taskmanager`（public） |

## 監視する指標（初版）

- Commit Charge / Commit Limit / Commit Usage
- Page File の現在サイズ・使用量・自動管理かどうか
- Available / Free / Standby Memory の内訳
- Top Processes（Process Commit 上位 5 件）
- VM Memory（`vmmem` / `vmmemWSL`）を別枠で表示
- CPU・ディスク・GPU は対象外（標準タスクマネージャで足りる）

## Alert Level と通知

| Level | 条件（Commit Usage） | トレイアイコン | 通知 |
|-------|---------------------|---------------|------|
| Normal | 85% 未満 | 緑 | なし |
| Warning | 85% 以上 | 黄 | なし |
| Critical | 95% 以上 | 赤 | バルーン通知を 1 回（同じ状態が続く間は再通知しない） |

しきい値は設定で変更できる。通知はトレイアイコンのバルーン（Shell_NotifyIcon）。トースト通知は使わない。

## 表示

- トレイ常駐。アイコンの色が Alert Level を表す。
- アイコンクリックでウィンドウ（グラフ + 数値）を開閉。通常モードと最前面コンパクトモードを切替。
- Sample は 1 秒ごと。グラフは直近 10 分（600 点）。
- Top Processes は 5 秒ごとに更新。
- 管理者権限は使わない。読めないプロセスは「(アクセス不可)」と表示。

## ログ

- Minute Record を CSV で `%LOCALAPPDATA%\CommitWatch\logs\YYYY-MM-DD.csv` に書く。
- 区切りは壁時計の分境界。タイムスタンプはローカル時刻の ISO 8601。
- 1 行 = 各指標の平均と最大 + 分末尾の Top Processes 5 件（名前と Process Commit）。
- 文字コードは UTF-8 with BOM（Excel でそのまま開ける）。
- 30 日より古いファイルは自動削除。
- 初版ではアプリ内の履歴グラフは作らない。トレイメニューに「ログフォルダを開く」を置く。

## 設定

- `%LOCALAPPDATA%\CommitWatch\config.toml` を正とし、ウィンドウ内の設定タブで編集できる。
- 項目: しきい値 2 段階、自動起動、更新確認間隔、ログ保持日数。
- 自動起動は HKCU の Run キー（ユーザー単位、管理者権限不要）。トレイメニューで ON/OFF。
- 二重起動は防ぐ。

## 技術

- 純 Rust。GUI は egui / eframe、グラフは egui_plot、トレイは tray-icon。
- Rust stable、edition 2024。
- 配布は単一 exe。初回起動時に `%LOCALAPPDATA%\CommitWatch\` へのコピーと自動起動登録を提案する。

## リリースと自動更新

- `vX.Y.Z` タグを push すると GitHub Actions（windows-latest）がビルドし、Releases に exe・`.sig`・`latest.json` を添付する。
- バージョンは `Cargo.toml` を正とし、タグとの一致を CI で検証する。
- push / PR では `cargo test` と `cargo clippy` を回す。
- `latest.json` は Tauri updater と同じ形式（`version`、`notes`、`pub_date`、`platforms.windows-x86_64.{url, signature}`）。
- 署名は minisign。鍵ペアは本アプリ専用に新規生成し、秘密鍵は GitHub secrets にだけ置く。
- 更新確認は起動時 + 24 時間ごと + トレイメニューの「更新を確認」。失敗は黙って次回へ。
- 更新が見つかったら事前にダウンロードし、バルーン通知で知らせる。適用と再起動はユーザーが選んだときだけ行う（監視とログに穴を空けないため）。
- 適用は署名検証後に自分自身の exe を差し替えて再起動する。

## 実装前に検証すること

- `vmmem` / `vmmemWSL` の Process Commit が VM の実使用量を反映するか。反映しなければ VM Memory の取り方を変える。
