# Building and releasing Vatra on Windows

## Release from GitHub (no local build)

The [Release workflow](https://github.com/Zmirzz/vatra/actions/workflows/release.yml) builds and signs the Windows x64 installer on a GitHub-hosted Windows runner. It creates a **draft** GitHub release containing the installer, its `.sig`, and the updater's `latest.json`.

1. Set a new stable version in the workspace `Cargo.toml` and `apps/desktop/package.json`. Update their lockfiles too (`cargo metadata --no-deps --format-version 1` and `npm install --package-lock-only` from `apps/desktop`). Commit and push the changes.
2. Open **Actions > Release > Run workflow**, select `main`, and run it. Alternatively, push a matching tag such as `v1.2.0`; tags must match the application version.
3. Wait for the workflow to finish. Download and check the installer from the draft release or the `vatra-windows-x64` workflow artifact.
4. Open **Releases**, review the draft notes, and click **Publish release**. Existing installations can then discover the update through the configured endpoint.

Your PC can be off after pushing the changes. The workflow always uses the selected commit, including when it creates the version tag. Drafts do not update `releases/latest`. Existing releases are never overwritten: use a new version, or delete an unpublished draft before rebuilding that version. Do not rerun a version that is already public.

The repository and releases can stay public. The private updater signing key belongs in an **encrypted Actions secret**, separate from repository files, and is not included in the installer or release assets. GitHub's build runner uses it to sign the installer; installed apps verify the signature using their existing public key. Automatic updates require keeping this private key somewhere trusted. This setup trusts GitHub Actions with it.

Configure repository **Settings > Secrets and variables > Actions** secrets once. Only the updater signing key is required; OAuth credentials are optional and unrelated to automatic updates:

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Contents of the existing updater private key; keep the key paired with the public key compiled into installed apps. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Key password, if applicable; otherwise leave unset. |
| `VATRA_GOOGLE_CLIENT_ID` | Optional: existing Google OAuth client ID. |
| `VATRA_GOOGLE_CLIENT_SECRET` | Optional: existing Google OAuth client secret. |
| `VATRA_MICROSOFT_CLIENT_ID` | Optional: existing Microsoft OAuth client ID. |

The built-in `GITHUB_TOKEN` publishes releases with `contents: write`; no personal access token is needed. The workflow requires the signing key before building. It runs only on manual dispatch or a version tag, never on pull requests. Only run it against trusted code, because building code has access to the release credentials. If supplied, OAuth values remain compiled into the app, as in the existing local release process. Without OAuth secrets, users must provide their own OAuth client configuration for Google/Microsoft sign-in; automatic updates still work.

Run the isolated release-script checks with `powershell -ExecutionPolicy Bypass -File scripts/release.test.ps1`. These use fake build/publish commands and credentials, and do not establish that a hosted build or live update succeeded.

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

- **Private key:** GitHub Actions uses the `TAURI_SIGNING_PRIVATE_KEY` secret. Local builds fall back to `%USERPROFILE%\.vatra-release\updater.key`, outside this repository. Environment credentials take precedence over local files. Never print or commit key contents; verify that the key matches the public key before publishing.
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
