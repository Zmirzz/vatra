# Local build cache cleanup (Windows)

Rust debug/test artifacts can accumulate across source revisions and compiler flags.
The workspace disables incremental compilation and debug symbols for dev/test builds
to reduce growth. Release settings are unchanged. This trades slower rebuilds and less
native debugging information for disk space; override the profiles locally when needed.

From the repository root:

```powershell
node scripts/cleanup.mjs          # report size, no deletion
node scripts/cleanup.mjs --apply  # remove regenerable debug artifacts now
powershell -ExecutionPolicy Bypass -File scripts/install-cleanup-task.ps1
```

The installer registers `Vatra-ProjectCacheCleanup` for the current user, daily at 12:30,
with missed runs executed when available. It runs hidden while the user is logged in.
It uses absolute Node/Cargo paths; reinstall the task if you move the checkout or tools.
No administrator access, downloads or credentials are needed.

Automatic runs clean only when `target/debug` exceeds 20 GiB and all its files and
directories have been untouched for at least 24 hours. They skip while Cargo, compiler,
linker or a program under this checkout's debug directory is running. Cargo's own lock
also serializes cleanup with builds. A concurrent build may delay cleanup.

Only `cargo clean --offline --profile dev --target-dir <this checkout>/target` is used.
The tool rejects linked target directories and linked entries in the debug cache.
It preserves `target/release` (including installers), sources, Git history, dependencies,
models, databases and application data. The next debug/test build recompiles its artifacts.
No cache size ceiling is guaranteed during an active build or the 24-hour idle window.

The latest daily result replaces `.cleanup.log`. If a cleanup process is forcibly killed,
inspect whether cleanup is still running before removing a leftover `.cleanup.lock`.

To disable the schedule:

```powershell
Unregister-ScheduledTask -TaskName Vatra-ProjectCacheCleanup -Confirm:$false
```

Safety checks: `node --test scripts/cleanup.test.mjs`.
