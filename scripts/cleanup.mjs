import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

export const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const GiB = 2 ** 30;

// Refuse links anywhere under the cleanup boundary, including Windows junctions.
export function inspect(directory) {
  const stat = fs.lstatSync(directory);
  if (stat.isSymbolicLink()) throw new Error(`Refusing linked path: ${directory}`);
  if (!stat.isDirectory()) return { bytes: stat.size, newest: stat.mtimeMs };
  let bytes = 0;
  let newest = stat.mtimeMs;
  for (const name of fs.readdirSync(directory)) {
    const child = inspect(path.join(directory, name));
    bytes += child.bytes;
    newest = Math.max(newest, child.newest);
  }
  return { bytes, newest };
}

export function eligible(stats, now = Date.now()) {
  return stats.bytes > 20 * GiB && now - stats.newest >= 24 * 60 * 60 * 1000;
}

export function runCleanup({ repository = root, apply = false, auto = false,
  cargo = 'cargo', busy = windowsBusy, run = spawnSync } = {}) {
  const resolved = fs.realpathSync(repository);
  if (!fs.existsSync(path.join(resolved, 'Cargo.toml'))) throw new Error('Missing Cargo.toml');
  const target = path.join(resolved, 'target');
  const debug = path.join(target, 'debug');
  if (!fs.existsSync(target)) return console.log('No target cache.');
  if (fs.lstatSync(target).isSymbolicLink()) throw new Error('Refusing linked target directory');
  if (!fs.existsSync(debug)) return console.log('No debug cache.');
  const stats = inspect(debug);
  console.log(`Debug cache: ${(stats.bytes / GiB).toFixed(2)} GiB`);
  if (auto && !eligible(stats)) return console.log('Skipped: below 20 GiB or used within 24 hours.');
  if (!apply && !auto) return console.log('Preview only. Use --apply to clean debug artifacts.');
  if (busy(resolved)) return console.log('Skipped: a build or debug program is running.');
  const lockPath = path.join(resolved, '.cleanup.lock');
  let lock;
  try { lock = fs.openSync(lockPath, 'wx'); }
  catch (error) {
    if (error.code === 'EEXIST') return console.log('Skipped: another cleanup holds .cleanup.lock.');
    throw error;
  }
  try {
    // Cargo serializes this operation with builds using its own build-directory lock.
    // Explicit profile + absolute target-dir: never remove release installers or user data.
    const result = run(cargo, ['clean', '--offline', '--profile', 'dev', '--target-dir', target,
      '--manifest-path', path.join(resolved, 'Cargo.toml')], { cwd: resolved, stdio: 'inherit', windowsHide: true });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`cargo clean failed (${result.status})`);
    console.log('Debug artifacts cleaned. Release artifacts preserved.');
  } finally {
    fs.closeSync(lock);
    fs.unlinkSync(lockPath);
  }
}

function windowsBusy(repository) {
  if (process.platform !== 'win32') throw new Error('Automatic process check is Windows-only.');
  const result = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
    "Get-Process | Select-Object ProcessName,Path | ConvertTo-Json -Compress"],
  { encoding: 'utf8', windowsHide: true });
  if (result.error || result.status !== 0) throw new Error('Cannot check running processes; cleanup cancelled.');
  const processes = JSON.parse(result.stdout || '[]');
  const debugPrefix = path.join(repository, 'target', 'debug').toLowerCase() + path.sep;
  return (Array.isArray(processes) ? processes : [processes]).some(p =>
    /^(cargo|rustc|rustdoc|cl|link|msbuild)$/i.test(p.ProcessName) ||
    p.Path?.toLowerCase().startsWith(debugPrefix));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const cargoIndex = args.indexOf('--cargo');
  const remaining = args.filter((_, i) => i !== cargoIndex && (cargoIndex < 0 || i !== cargoIndex + 1));
  if (remaining.some(arg => !['--apply', '--auto'].includes(arg)) ||
      (cargoIndex >= 0 && !args[cargoIndex + 1])) throw new Error('Usage: cleanup.mjs [--apply | --auto] [--cargo PATH]');
  try { runCleanup({ apply: args.includes('--apply'), auto: args.includes('--auto'),
    cargo: cargoIndex < 0 ? 'cargo' : args[cargoIndex + 1] }); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
