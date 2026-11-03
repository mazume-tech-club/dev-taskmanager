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

段階式（ADR 0002）。初版は管理者権限なしで取れる値だけを出し、第 2 段で Sensor Provider（PawnIO）を足す。

### 初版（ドライバ無し・通常権限）

- CPU Load: `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` の差分。1 秒に 1 回。
- Effective Clock: パフォーマンスカウンター `% Processor Performance` × `Processor Frequency`。1 秒に 1 回。
  電源マネージャの `CurrentMhz` は公称値しか返さないので使わない。
- Thermal Zone Temperature: パフォーマンスカウンター `\Thermal Zone Information(*)\Temperature`（ケルビン）。
  マザーボードの 1 点で、Core Temperature ではないことを画面に明記する。
- Core Temperature・TjMax・VID・パッケージ電力は「取得不可（Sensor Provider 未導入）」と表示する。

### 第 2 段（Sensor Provider = PawnIO）— 実装済み、実機未検証

- `src/pawnio.rs`: デバイス `\\?\GLOBALROOT\Device\PawnIO` を開き、`IOCTL_PIO_LOAD_BINARY` で公式モジュール
  `IntelMSR.bin`（`assets/pawnio/`、LGPL-2.1、PawnIO.Modules 0.2.11 の署名済みビルド）を読み込み、
  `IOCTL_PIO_EXECUTE_FN` で `ioctl_read_msr` を呼ぶ。入力は 32 バイトの関数名 + i64 引数、出力は i64 配列。
- `src/sensors.rs`: 物理コアごとにスレッドアフィニティを切り替えて `IA32_THERM_STATUS`（コア温度）を読み、
  `MSR_TEMPERATURE_TARGET`（TjMax）、`IA32_PACKAGE_THERM_STATUS`（Package Temperature）、`IA32_PERF_STATUS`（VID）、
  RAPL（`MSR_RAPL_POWER_UNIT`、`MSR_PKG_ENERGY_STATUS` の差分で電力、`MSR_PKG_POWER_INFO` で TDP）を読む。
- 状態は Sample の `sensor_state` で持つ: 設定で無効 / 取得中 / PawnIO 未インストール / 管理者権限が必要 / 読み取り失敗。
  未インストール時はトレイメニューに「PawnIO をインストール」（公式セットアップのダウンロード）、権限不足時は
  「管理者権限で再起動」（UAC 1 回、`--wait-for <pid>` で旧プロセスを待ってから常駐）が出る。
- 管理者権限で動いているときの自動起動は Run キーではなくタスクスケジューラ（`/RL HIGHEST /SC ONLOGON`）に登録する。
  Run キーからは昇格起動できないため。
- ログの Minute Record に `core_temp_max_c`（その分で最も熱かったコア）を追加。Package Temperature の平均・最大は既存列。
- Intel 先行。AMD は別モジュールが必要になった時点で追加。
- 温度の Alert Level: 最も熱い値（パッケージとコア）が TjMax − 15 ℃ で Warning、TjMax − 5 ℃ で Critical。トレイアイコンの色はメモリと温度の悪い方。
- 他ツール（Core Temp 等）の共有メモリは読まない。個人利用限定ライセンスと UAC の問題があるため。
- 2026-11-03 時点: この PC に PawnIO が未インストール（インストールと管理者実行に UAC が要るため無人検証不可）。
  通常権限での `--probe` は「PawnIO がインストールされていません」と正しく報告する。MSR の値の正しさは実機で未確認。

### ウィンドウのレンダラー

OpenGL（glow）を既定にし、RDP の切断中や VM などで OpenGL 2.0 が無い場合は自動で wgpu（DirectX）の
ウィンドウプロセスを起動し直す（`--window-wgpu`）。wgpu 時はウィンドウのメモリが大きい（NVIDIA で約 290 MB）が、
常駐側には影響しない。切断中の RDP セッションで動作確認済み。

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

## プロセス構成（ADR 0003）

| プロセス | 起動方法 | 役割 | 実測（release） |
|---------|---------|------|----------------|
| 常駐 | `commitwatch.exe`（自動起動は `--hidden`） | トレイアイコン、Alert Level、バルーン通知、サンプラー、CSV ログ、共有メモリ書き込み | Process Commit 約 10 MB、CPU 0.03% |
| ウィンドウ | 常駐が `--window` で起動 | 共有メモリを毎秒読んで表示。閉じると終了 | Process Commit 約 45 MB |

- 共有メモリ `Local\CommitWatch.state`: 直近 600 秒の Sample と最新の Top Processes。seqlock で整合を取る。
- 設定はウィンドウが `config.toml` に書き、常駐が更新時刻を 5 秒ごとに見て読み直す。
- 2 回目の起動は既存ウィンドウを前面に出すだけ。

## 技術

- 純 Rust。GUI は egui / eframe（レンダラーは glow 固定、`system_fonts` 無効）、グラフは egui_plot。
- トレイは Win32 API を直接使う（`tray-icon` クレートはバルーン通知に非対応）。
- 日本語フォントは BIZ UDGothic（SIL OFL）を `tools/subset_font.py` で JIS 第 1 水準までに絞って同梱。
- Rust stable、edition 2024。
- 配布は単一 exe。初回起動時に `%LOCALAPPDATA%\CommitWatch\` へのコピーと自動起動登録を提案する。

## リリースと自動更新

### リリース手順

1. `Cargo.toml` の `version` を上げてコミットする。
2. `git tag vX.Y.Z && git push origin main vX.Y.Z` を実行する。
3. GitHub Actions（`.github/workflows/release.yml`、windows-latest）が、タグと `Cargo.toml` の一致を確認し、
   `cargo test` → `cargo build --release` → `signer` で minisign 署名 → `latest.json` 生成 → Release 作成
   （`commitwatch.exe`、`commitwatch.exe.sig`、`latest.json` を添付、リリースノートは自動生成）を行う。
4. push / PR では `.github/workflows/ci.yml` が `cargo fmt --check`、`cargo clippy -D warnings`、`cargo test` を回す。

### 署名鍵

- `cargo run --bin signer -- keygen <dir>` で本アプリ専用の minisign 鍵ペアを作る（暗号化なし）。
- 公開鍵は `assets/update-public-key.txt` としてリポジトリに入れ、exe に埋め込む。
- 秘密鍵 `commitwatch.key` はリポジトリに入れない（`.gitignore` の `*.key`）。ファイルの中身をそのまま
  GitHub の Secrets `COMMITWATCH_SIGNING_KEY` に登録する。minisign CLI で作ったパスワード付き鍵なら
  `COMMITWATCH_SIGNING_KEY_PASSWORD` も登録する。
- 鍵を替えると、旧鍵で署名された版からの自動更新は検証に失敗する。替える場合は手動で入れ直してもらう。

### `latest.json`（Tauri updater 形式）

```json
{
  "version": "0.2.0",
  "notes": "CommitWatch v0.2.0",
  "pub_date": "2026-11-03T05:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<minisign 署名ファイルの本文を base64 にしたもの>",
      "url": "https://github.com/mazume-tech-club/dev-taskmanager/releases/download/v0.2.0/commitwatch.exe"
    }
  }
}
```

既定の取得先は `https://github.com/mazume-tech-club/dev-taskmanager/releases/latest/download/latest.json`。
`config.toml` の `[update] endpoint` で上書きでき、`file://` も使える（ローカル検証用）。

### アプリ側の動き

| 段階 | 内容 |
|------|------|
| 確認 | 常駐が起動 30 秒後、以後 `check_interval_hours` ごと、またはトレイメニュー「更新を確認」／設定タブ「今すぐ確認」。バックグラウンドスレッドで実行 |
| 準備 | semver で新しければ exe をダウンロードし、埋め込み公開鍵で署名を検証してから `%LOCALAPPDATA%\CommitWatch\updates\` に置く（`staged.json`）。検証に失敗したものは残さない |
| 通知 | 準備できたらバルーン通知（情報）。適用はしない |
| 適用 | トレイメニュー「vX に更新して再起動」。動作中の exe を `.old.exe` に改名 → 新 exe をコピー → `--after-update <pid>` で新プロセス起動 → 旧プロセス終了（ログの部分分を書き出し） |
| 後始末 | 新プロセスは旧 pid の終了を待ち、`.old.exe` と `updates\` を削除して常駐を開始する |

ローカルで端から端まで検証済み（2026-11-03）: 改ざんした exe は署名検証で拒否、正規の exe はステージされ、
`--apply-update` で差し替え・再起動・後始末まで完了。診断コマンドは `--check-update`（確認と準備のみ）と
`--apply-update`（ステージ済みを適用）。

## 検証済みの前提

- `vmmem` / `vmmemWSL` の Process Commit は VM の確保量を反映する（2026-11-03 確認。ワーキングセット 2 GB に対し Process Commit 6 GB）。
