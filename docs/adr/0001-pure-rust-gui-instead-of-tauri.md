---
status: accepted
date: 2026-11-03
---

# GUI は Tauri ではなく純 Rust で作る

社内の前例（MarkdownPreview）は Tauri v2 で、自動更新の仕組み（`tauri-plugin-updater` と `tauri-action`）をそのまま流用できる。
それでも本アプリは純 Rust の GUI を採用する。常駐してメモリを監視するアプリ自身が WebView2 で 80〜150 MB を消費するのは目的に反するためで、
純 Rust なら 30〜60 MB 程度・単一 exe で済む。

## Considered Options

- **Tauri v2**: 更新機構を前例から丸ごと流用できる。常駐時のメモリが大きい。却下。
- **純 Rust GUI**: 軽量・単一 exe。更新確認・ダウンロード・署名検証・差し替えを自前で書く必要がある。採用。

## Consequences

- 「MarkdownPreview と同じ方式」は、タグ push で GitHub Actions がビルドし Releases に exe・署名・`latest.json` を置く流れを指す。アプリ側の更新コードは共有しない。
- 更新ファイルの署名は MarkdownPreview と同じ minisign 方式にそろえ、鍵ペアは本アプリ専用に新規生成する。
