# ADR 0001: Rust + Tauri を採用する

**状態**: 承認 (2026-04-29)

## コンテキスト

リモートデスクトップソフトウェアの言語・UIフレームワーク選択。

候補:
- C++ (TeamViewer/AnyDesk)
- C# (.NET MAUI)
- Rust (RustDesk)
- Go
- Electron (UI)
- Tauri (UI)
- Flutter (UI)
- 純ネイティブ (Win32/Cocoa/Qt)

## 判断

**コア: Rust、UI: Tauri v2 + React**

## 理由

### Rust を採用した理由
1. **メモリ安全**: リアルタイムバイナリプロトコル + crypto + GPU バッファ操作で
   バッファオーバーフロー脆弱性は致命的。RustDesk が実証済み。
2. **パフォーマンス**: C/C++ と同等。GC ストール無し。
3. **エコシステム**: tokio, quinn, rustls, ring 等の品質高い暗号・非同期ライブラリ。
4. **クロスコンパイル**: GitHub Actions で全 OS/アーキ向けビルドが容易。
5. **FFI**: ffmpeg/libvpx/CoreGraphics 等の C ライブラリと自然に連携。

### Tauri v2 を採用した理由 (Electron 不採用)
1. **バイナリサイズ**: Tauri ~10MB vs Electron ~120MB。
2. **メモリ**: Tauri (system WebView) ~50MB vs Electron (Chromium 同梱) ~250MB。
3. **起動時間**: Tauri ~500ms vs Electron ~2000ms。
4. **セキュリティ**: Tauri のコマンド allowlist + CSP 強制。
5. **ネイティブ統合**: Rust コアと直接 IPC、JSON のシリアライズコストのみ。

### Flutter 不採用の理由
- Dart 言語は Rust から呼ぶ FFI が複雑。
- ネイティブ統合 (system tray、screen capture) で結局 platform channels が必要。
- バイナリサイズが Tauri より大きい。

## 結果

- ✓ 単一の Rust コードベースで全 OS 対応
- ✓ UI は HTML/CSS/JS で柔軟に開発可能
- ✓ セキュリティ: メモリ安全 + sandbox WebView
- ✗ WebView の差分により UI に微妙な見た目差 (Edge/WebKit/WebKitGTK)
  → CSS 互換性に注意

## 代替案 (将来検討)
- Native UI (egui/iced/Slint) — WebView 差分排除したい場合
- Tauri Mobile (iOS/Android) — モバイル対応時
