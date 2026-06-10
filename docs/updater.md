# Auto-Update System

Miru uses Tauri's updater plugin with Ed25519-signed release manifests.

## How it works

1. Client polls a manifest URL on launch (configurable)
2. Manifest is JSON containing the current version + signed download URLs
3. If newer version available, prompt user to install
4. Downloaded binary is verified against signature before applying

## Manifest format (`updates.json`)

```json
{
  "version": "0.2.0",
  "notes": "VP9 hardware acceleration; PipeWire capture",
  "pub_date": "2026-05-01T12:00:00Z",
  "platforms": {
    "linux-x86_64": {
      "signature": "<base64 ed25519>",
      "url": "https://github.com/shizukutanaka/miru/releases/download/v0.2.0/miru-x86_64-linux.AppImage.tar.gz"
    },
    "darwin-x86_64": { ... },
    "darwin-aarch64": { ... },
    "windows-x86_64": { ... }
  }
}
```

## Generating signing keys (one-time setup)

```bash
# Generate keypair (keep .key file SECRET, commit .key.pub)
npx @tauri-apps/cli signer generate -w ~/.miru-update-key

# Output:
# Public key: <PUBKEY>  ← embed in tauri.conf.json
# Private key path: ~/.miru-update-key.key
```

## Signing a release

```bash
# After GitHub Actions builds the binaries
npx @tauri-apps/cli signer sign \
  -k ~/.miru-update-key.key \
  -p "<password>" \
  /path/to/miru.AppImage.tar.gz

# Outputs miru.AppImage.tar.gz.sig — put this URL in updates.json
```

## CI integration

`.github/workflows/release.yml` should:
1. Build binaries for all platforms (already does this)
2. Sign each artifact with stored secret key (`secrets.UPDATER_PRIVATE_KEY`)
3. Generate `updates.json` from signed artifacts
4. Upload `updates.json` to GitHub Pages or the release itself

## Threat model

- Update server compromise → can serve a malicious manifest, but signature won't verify
- Build server compromise → attacker can sign malicious binaries → mitigated by:
  - Reproducible builds (planned for v1.0)
  - Sigstore + transparency log (planned for v2.0)
- Network MITM → manifest must be HTTPS; signature verification is mandatory
