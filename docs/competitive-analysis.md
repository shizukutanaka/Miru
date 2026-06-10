# 同種ソフト比較分析

最終更新: 2026-04-29
対象バージョン: Miru v0.1, RustDesk 1.4.5, Parsec (current), Sunshine 0.21+/Moonlight 11+, AnyDesk 8.x, TeamViewer 15.x

## 1. 機能マトリクス

| 機能 | Miru | RustDesk | Parsec | Sunshine+Moonlight | AnyDesk | TeamViewer |
|------|------|----------|--------|--------------------|---------| -----------|
| **オープンソース** | ✓ MIT | ✓ AGPLv3 | ✗ 独自 | ✓ GPLv3 | ✗ | ✗ |
| **セルフホスト** | ✓ | ✓ | △ Enterprise のみ | ✓ | ✗ | ✗ |
| **無料商用利用** | ✓ | ✓ | △ ホビー無料 | ✓ | ✗ $588/年〜 | ✗ $49/月〜 |
| **アカウント不要** | ✓ | ✓ | ✗ | ✓ | △ | ✗ |
| **言語: コア** | Rust | Rust | C | C++ | C++ | C++ |
| **言語: UI** | TS+React (Tauri) | Dart (Flutter) | C/Native | Web/Native | C++ | Native |
| **バイナリサイズ** | ~10-15 MB目標 | 30-40 MB | 5 MB | 10 MB | 10 MB | 50 MB |
| **メモリ常駐** | ~50 MB目標 | 150-200 MB | 80 MB | 60 MB | 100 MB | 200 MB |

## 2. プロトコル / ネットワーク

| 項目 | Miru | RustDesk | Parsec | Sunshine |
|------|------|----------|--------|----------|
| **トランスポート** | QUIC + WS Relay | UDP/TCP punch + Relay | UDP (BUD独自) | UDP (RTSP系) |
| **暗号** | X25519+Ed25519+ChaCha20-Poly1305 | NaCl (libsodium) | DTLS 1.2 + AES128/256 | AES-GCM |
| **NAT 越え成功率 (公称)** | 目標 85% | 不明、高め | **97%** ★ | LAN前提 |
| **3層フォールバック** | ✓ P2P→Relay | ✓ Punch→IPv6→Relay | ✓ P2P→Relay | ✗ LAN前提 |
| **0-RTT 再接続** | 計画 | ✓ | ✓ | ? |
| **ハンドシェイク** | X25519+Ed25519 (PFS) | NaCl | DTLS | PIN+TLS |
| **PFS** | ✓ ephemeral | ✓ | ✓ | ? |
| **認証** | TOFU + PIN + Ed25519 | パスワード | PIN + アカウント | PIN |
| **バッファリング** | あり (削減検討) | あり | **なし** ★ (予測検出) | あり |
| **輻輳制御** | AIMD (シンプル) | 不明 | **BBR系独自** ★ | 標準 |
| **FEC** | 計画 (Opus内蔵のみ) | ? | **あり** ★ | ? |

## 3. キャプチャ / コーデック

| 項目 | Miru | RustDesk | Parsec | Sunshine |
|------|------|----------|--------|----------|
| **Win キャプチャ** | DXGI Desktop Duplication | DXGI + Desktop Duplication | DDA/DXGI | DXGI/WGC |
| **macOS キャプチャ** | CGDisplay→SCK計画 | SCK | AVFoundation | ScreenCaptureKit |
| **Linux X11** | ✓ MIT-SHM | ✓ | ✗ | ✓ |
| **Linux Wayland** | PipeWire計画 | ✓ PipeWire/portal | ✗ | ✓ KMS/PipeWire |
| **VRAM ゼロコピー** | 計画 | **VRamEncoder** ★ | **GPU pipeline** ★ | ✓ |
| **VP8** | ✓ | ✓ | ✗ | ✗ |
| **VP9** | ✓ libvpx | ✓ libvpx | ✗ | ✗ |
| **AV1 ソフトウェア** | 計画 | ✓ aom | ✗ | ✗ |
| **AV1 HW** | 計画 (NVENC/AMF/QSV検出) | ✓ | ✗ | ✓ NVENC/AMF/QSV |
| **H.264 HW** | hwプローブ済み | ✓ | ✓ NVENC/AMF/QSV/VT | ✓ |
| **H.265 HW** | hwプローブ済み | ✓ | ✓ | ✓ |
| **HDR (Main 10)** | 未対応 | △ | ✗ | **✓** ★ |
| **dirty rect 利用** | 取得済・未活用 | ? | ? | ? |

## 4. セキュリティ (CVE 履歴から学ぶ)

### RustDesk 既知のCVE (2026年3月時点)
- **CVE-2026-30785** (高): パスワードハッシュ計算量不足 + プロトタイプ汚染
- **CVE-2026-30794** (高): TLS 失敗時に `danger_accept_invalid_certs(true)` フォールバック → AiTM 可能
- **CVE-2026-30791**: 設定ファイルの脆弱な暗号化
- **CVE-2026-30798**: heartbeat の改ざん検証不足
- **CVE-2026-2490**: ファイル転送のシンボリックリンクトラバーサル → SYSTEM権限で任意ファイル読取
- **CVE-2024-25140**: WDKTestCert を Trusted Root に追加 (テスト証明書を本番出荷)

### Miru の対策 (RustDesk の失敗を回避)

| RustDesk CVE | Miru の対応 |
|---|---|
| 弱いパスワードハッシュ | PBKDF2-SHA256 100k+ iter (実装済), v0.3 で **Argon2id** 移行 |
| TLS 検証無効フォールバック | **すべての TLS で full validation 強制**、フォールバック禁止 |
| 脆弱な設定暗号化 | identity ファイルは平文 (chmod 600)、設定は ChaCha20-Poly1305 |
| Heartbeat 改ざん | すべての制御メッセージは認証済み暗号化 (AEAD) |
| ファイル転送シンボリックリンク | **`fs::symlink_metadata` + 拒否**、宛先パス正規化、ホワイトリスト |
| テスト証明書出荷 | ビルドプロセスで本番/テスト分離、CI で検出 |

## 5. パフォーマンス比較 (公開情報)

### LAN 環境のレイテンシ
- **Parsec**: 7ms (最低)
- **Sunshine/Moonlight**: <10ms (ゲーミング最適化)
- **AnyDesk**: 16-30ms (公称)
- **RustDesk**: 30-50ms (推定)
- **TeamViewer**: 50-100ms

### Miru の目標
- v0.1: 30-50ms LAN (測定未実施)
- v1.0: **15ms LAN** (Parsec の半分以下を目標)

### エンコード遅延 (Parsec データより)
- NVENC: 5.8 ms (中央値) ← 一番速い
- QSV: 11.0 ms
- AMD VCE: 15.0 ms
- ソフトウェア (VP9): 20-50 ms

→ Miru は AV1 NVENC 統合で 6-8ms を目指す

## 6. UX / UI

| 項目 | Miru | RustDesk | Parsec | Sunshine |
|------|------|----------|--------|----------|
| **デバイス ID 形式** | XXXX-XXXX | 9桁数字 | アカウントID | LAN自動検出 |
| **ペアリング UI** | 6桁PIN + 指紋 | パスワード | アカウント | PIN |
| **多モニタ** | UI実装済 | ✓ Wayland 1.4.3+ | △ Teams | ✓ |
| **クリップボード同期** | ✓ | ✓ | ✓ | ? |
| **ファイル転送** | プロトコル済・UI未 | ✓ | △ | ✗ |
| **HDR 表示** | 未対応 | △ | ✗ | ✓ |
| **ゲームパッド** | 未対応 | △ | ✓ | ✓ ViGEm |
| **タブレット/スタイラス** | 未対応 | △ | ✓ Warp | △ |
| **音声品質** | Opus 32-64kbps | Opus | Opus | Opus 5.1/7.1 |
| **モバイルクライアント** | 計画 | ✓ Android/iOS | ✓ | ✓ |
| **Web クライアント** | 計画 | ✓ | ✓ | ✓ |
| **パフォーマンス overlay** | FPS/RTT のみ | △ | △ | **✓ 5段階分解** ★ |

## 7. Miru が目指す差別化

### A. 最高水準のセキュリティ
- E2E + PFS + identity pinning + Argon2id (RustDesk より厳格)
- 全 TLS で full validation (検証無効フォールバック禁止)
- ファイル転送のシンボリックリンク完全拒否
- 年1回外部監査 (Trail of Bits 等)
- 再現可能ビルド (バイナリ改ざん検出)
- post-quantum ハイブリッド (X25519 + Kyber768) v2.0

### B. パフォーマンス (Parsec を参考)
- バッファレス設計 → 予測ベース輻輳検出 (BBR系)
- ゼロコピー GPU パイプライン (DXGI texture → NVENC 直接)
- AV1 HW 優先 (NVENC RTX40+ / Arc / RX 7000+)
- 5段階パフォーマンス overlay (capture/encode/network/decode/display)
- FEC (Forward Error Correction) で再送なしロス補償

### C. 開放性 (RustDesk + 改善)
- MIT (RustDesk の AGPLv3 より緩い、商用統合容易)
- すべてのプロトコル仕様公開 (RFC形式)
- セルフホスト server バイナリ単体 (Docker compose)
- WASI プラグインシステム

### D. 日本語 First
- ドキュメント・UI の第一言語が日本語
- 1000+言語対応を目標 (Crowdin/Weblate)
- BCP-47 タグ徹底

### E. 10年運用
- プロトコル versioning による後方互換
- 6か月の deprecation 期間
- 静的リンクで OS 依存最小化
- 全ADR 永続保存

## 8. やらないこと (意図的に削除)

- **広告・テレメトリ**: 一切なし (オプトインクラッシュレポートのみ)
- **アカウント強制**: 不要
- **クラウド配信前提**: セルフホストで完結
- **モバイル先行**: デスクトップを完成させてから
- **ゲーミング特化**: Parsec/Sunshine の領域、競合しない
- **Enterprise SaaS**: コードを売らず、サポートを提供する形
