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
- CPU Load（全体と論理プロセッサごと）とクロック。詳細は「CPU」の節
- Core Temperature / Package Temperature / TjMax / 電力（Sensor Provider 経由）
- ディスク・GPU は対象外（標準タスクマネージャで足りる）
- Kernel Commit（Commit Charge − 全 Process Commit の合計）を「カーネル等」として表示

## Alert Level と通知

| Level | 条件（Commit Usage） | トレイアイコン | 通知 |
|-------|---------------------|---------------|------|
| Normal | 85% 未満 | 緑 | なし |
| Warning | 85% 以上 | 黄 | なし |
| Critical | 95% 以上 | 赤 | バルーン通知を 1 回（同じ状態が続く間は再通知しない） |

しきい値は設定で変更できる。通知はトレイアイコンのバルーン（Shell_NotifyIcon）。トースト通知は使わない。

## CPU

- CPU Load とクロックは自前で取る（`NtQuerySystemInformation` と `CallNtPowerInformation`、1 秒に 1 回ずつ）。
  電源マネージャのクロックは公称値で固定されることがあるので、Sensor Provider の値があればそちらを優先する。
- 温度は MSR の読み取りが必要でユーザーモードでは取れないため、自前ドライバは持たず Sensor Provider から借りる。
- Sensor Provider は Core Temp（alcpu.com）。共有メモリ `CoreTempMappingObjectEx` を 1 秒に 1 回コピーするだけなので負荷はほぼゼロ。
  Core Temp 未起動時は「温度: Core Temp 未起動」と表示し、トレイメニューに「Core Temp を起動」を置く。
- Core Temp は `requireAdministrator` 指定のため起動時に UAC が出る。常時表示したい場合は Core Temp 側の「Windows 起動時に開始」を使ってもらう。
- 温度の Alert Level: Package Temperature が TjMax − 15 ℃ で Warning、TjMax − 5 ℃ で Critical。トレイアイコンの色はメモリと温度の悪い方。
- 検証済み（2026-11-03、Xeon W-1250 6C/12T）: CPU Load と公称クロック取得、Core Temp 共有メモリ構造体のオフセット（4740 バイト）。
  Core Temp 起動中の実読み取りは未確認。

## 性能予算

「監視アプリ自身が負荷になるのは NG」を数値にしたもの。超えたら機能より予算を優先する。

| 項目 | 上限 |
|------|------|
| 常駐中の平均 CPU 使用率 | 0.5% 未満 |
| 常駐中のメモリ（Process Commit） | 40 MB 未満 |
| ウィンドウ非表示時 | 描画を止める。取得とログだけ続ける |

## 表示

- トレイ常駐。アイコンの色が Alert Level を表す。
- アイコンクリックでウィンドウ（グラフ + 数値）を開閉。通常モードと最前面コンパクトモードを切替。
- Sample は 1 秒ごと。グラフは直近 10 分（600 点）。
- Top Processes は 5 秒ごとに更新。
- 管理者権限は使わない。読めないプロセスは「(アクセス不可)」と表示。

## ログ

- Minute Record を CSV で `%LOCALAPPDATA%\CommitWatch\logs\YYYY-MM-DD.csv` に書く。
- 区切りは壁時計の分境界。タイムスタンプはローカル時刻の ISO 8601。
- 1 行 = 各指標の平均と最大 + 分末尾の Top Processes 5 件（名前と Process Commit）+ 全体 CPU Load の平均と最大 + Package Temperature の平均と最大 + 各コア温度の最大。温度が取れない分は空欄。
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
