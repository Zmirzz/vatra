# Building and releasing Vatra on Windows

## Desktop executable and NSIS installer

From `apps/desktop`:

```powershell
npm run build:desktop
```

This script runs Tauri with the NSIS bundler. The equivalent explicit command is:

```powershell
npm run tauri -- build --bundles nsis
```

Tauri runs the configured `beforeBuildCommand`, including sidecar preparation. The NSIS output is under `target/release/bundle/nsis/` at the workspace root. Check the command's exit status and output artifact before reporting an installer as built.

`apps/desktop/src-tauri/tauri.conf.json` selects `bundle.windows.nsis.installMode = "currentUser"`. This configures installation for the current user rather than a machine-wide administrator install. A real silent install, update and uninstall (2026-11-03, see the plan D validation report) put the program in `%LOCALAPPDATA%\Vatra`, the same folder that holds the downloaded models in `models\`; it is not `%LOCALAPPDATA%\Programs\Vatra`. The uninstaller removed the program files and the registry entry and left `models\` and `%APPDATA%\Vatra` untouched, because it removes the folder only when it is empty. The installer hook in `apps/desktop/src-tauri/installer-hooks.nsh` stops the `vatra.exe` of that folder first, since the service keeps running after the app closes and would lock the file. Keeping programs and models in one folder works, but separating them is an open decision.

The installer is not signed with Authenticode. Windows may show a SmartScreen warning. Updater signatures are a separate mechanism and do not remove that warning.

## Update configuration and signing

The desktop registers `tauri-plugin-updater`. Its `update_status` command checks availability and reports an empty endpoint list as unconfigured. After user confirmation, `install_update` checks again and calls `download_and_install`, which verifies the artifact signature before installing. Checking an update manifest alone does not prove that its artifact has been verified. The offline `update_signature` tests cover the minisign verification primitive with a public test vector and reject changed payload, signature and trusted-comment bytes; they do not exercise a live endpoint or an installer.

- **Private key:** the configured release workflow expects `%USERPROFILE%\.vatra-release\updater.key`, outside this repository. Never print or commit its contents. This document does not establish that the key exists or matches the public key; verify that locally before publishing.
- **Public key:** `apps/desktop/src-tauri/tauri.conf.json`, `plugins.updater.pubkey`.
- **Endpoint:** `https://github.com/Zmirzz/vatra/releases/latest/download/latest.json`. Updates are served from GitHub Releases; `scripts/release.ps1` builds with the signing key (read into the environment, never printed), writes `latest.json` and creates the release with `gh`.
- **Backup:** protect a backup of the private key. Losing it prevents the existing public key from accepting future artifacts signed with a replacement key; plan a manual reinstall or a migration while the old key is still available.

Before publishing updates:

1. Test the application download/install flow against a local release server with a disposable key and installer, including a rejected invalid signature. The offline primitive test does not establish this end-to-end behavior.
2. Configure an HTTPS `latest.json` endpoint and `bundle.createUpdaterArtifacts = true`. Keep the production private key outside the repository and build logs.
3. Supply the signing key to the build process and use Tauri's bundler so updater artifacts are produced:

   ```powershell
   $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -LiteralPath "$env:USERPROFILE\.vatra-release\updater.key" -Raw
   try {
     npm run tauri -- build --bundles nsis
   } finally {
     Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
   }
   ```

   If the key is password protected, also supply the corresponding signing password through the build environment without logging it.
4. Verify the installer and signature files actually exist; test installation and signature rejection against the built app before uploading.
5. Upload the installer, its `.sig` file and a `latest.json` whose Windows target URL and signature refer to that exact artifact. Do not publish a placeholder manifest or use a test key for production.

## Validation boundary

Building does not authorize installing programs on the user's computer. Real installation/uninstallation, a winget validation install, and native Claude/Codex comparisons require the separate explicit consent recorded in plan D. Use fake binaries and isolated data directories for automated checks. Report builds, automated checks and real installer runs separately.

## Reinstalling the same version

A silent install (`/S`) over an installed copy of the *same* version waits on the installer's "already installed" page and never finishes. Upgrades to a newer version, which is what updates do, run fine. Run it with `/S /UPDATE` to reinstall the same version silently (verified 2026-11-08), or bump the version first.
