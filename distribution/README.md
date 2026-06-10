# Miru 配布チャネル

すべての OS で 1コマンドインストール可能。

## macOS / Linux

### Homebrew (推奨)

```bash
brew install shizukutanaka/miru/miru
```

サーバーとして動かす場合:

```bash
brew services start miru
```

ファイル: `distribution/homebrew/miru.rb`
タップ: `https://github.com/shizukutanaka/homebrew-miru`

## Linux 専用

### Arch (AUR)

```bash
yay -S miru        # or paru -S miru
```

ファイル: `distribution/aur/PKGBUILD`

### Debian / Ubuntu (`.deb`)

```bash
curl -L https://github.com/shizukutanaka/miru/releases/latest/download/miru.deb \
  -o /tmp/miru.deb
sudo dpkg -i /tmp/miru.deb
```

### Fedora / RHEL (`.rpm`)

```bash
sudo dnf install https://github.com/shizukutanaka/miru/releases/latest/download/miru.rpm
```

### Flatpak (準備中)

```bash
flatpak install flathub app.miru.Host
```

### Snap (準備中)

```bash
sudo snap install miru
```

## Windows

### Scoop (推奨)

```powershell
scoop bucket add miru https://github.com/shizukutanaka/scoop-miru
scoop install miru
```

ファイル: `distribution/scoop/miru.json`

### Chocolatey (準備中)

```powershell
choco install miru
```

### winget (準備中)

```powershell
winget install shizukutanaka.miru
```

### MSIX

GitHub Releases から `.msix` をダウンロードし、ダブルクリック。
EV 署名済み (SmartScreen 警告なし)。

## 開発者向け: Cargo

```bash
cargo install miru-host miru-mcp miru-signal
```

## クライアント (Tauri ビューア)

各 OS の `miru-viewer` 配布:

| OS | 形式 |
|----|------|
| macOS | `.dmg` (Apple ノータライズ済み) |
| Windows | `.msi` (EV 署名済み) |
| Linux | `.AppImage` / `.deb` / `.rpm` / Flatpak |

## モバイル (準備中)

| OS | チャネル |
|----|---------|
| iOS | App Store |
| Android | Google Play / F-Droid |

## 検証

すべてのリリースに `SHA256SUMS` + `SHA256SUMS.sig` (Ed25519署名) を同梱:

```bash
# SHA256 検証
sha256sum -c SHA256SUMS
# 署名検証
miru-host verify-release SHA256SUMS SHA256SUMS.sig
```

公開鍵: `brand/release-pubkey.txt` に同梱、また Sigstore Rekor に登録済み。
