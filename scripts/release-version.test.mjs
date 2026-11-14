import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { nextVersion, synchronizeVersions, prepareRelease } from './release-version.mjs';

function fixture(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'vatra-release-version-'));
  t.after(() => {
    assert.ok(path.resolve(directory).startsWith(path.resolve(os.tmpdir()) + path.sep));
    fs.rmSync(directory, { recursive: true, force: true });
  });
  fs.mkdirSync(path.join(directory, 'apps/desktop'), { recursive: true });
  fs.mkdirSync(path.join(directory, 'src'));
  fs.writeFileSync(path.join(directory, 'src/lib.rs'), '');
  fs.writeFileSync(path.join(directory, 'Cargo.toml'), '[workspace]\nmembers = ["."]\n[workspace.package]\nversion = "1.1.0"\nedition = "2024"\n[package]\nname = "vatra-fixture"\nversion.workspace = true\nedition.workspace = true\n');
  fs.writeFileSync(path.join(directory, 'Cargo.lock'), 'version = 4\n\n[[package]]\nname = "vatra-fixture"\nversion = "1.1.0"\n');
  fs.writeFileSync(path.join(directory, 'apps/desktop/package.json'), '{"name":"vatra-desktop","version":"1.1.0"}\n');
  fs.writeFileSync(path.join(directory, 'apps/desktop/package-lock.json'), '{"version":"1.1.0","lockfileVersion":3,"packages":{"":{"name":"vatra-desktop","version":"1.1.0"},"node_modules/example":{"version":"9.0.0"}}}\n');
  return directory;
}

test('patch version exceeds source, existing tags, and draft release versions', () => {
  assert.equal(nextVersion('1.1.0', []), '1.1.1');
  assert.equal(nextVersion('1.1.0', ['v1.1.9', 'v1.1.10', 'v1.1.11-beta.1', 'unrelated']), '1.1.11');
  assert.equal(nextVersion('1.1.0', ['v2.0.4']), '2.0.5');
  assert.equal(nextVersion('3.0.0', ['v2.99.99']), '3.0.1');
  assert.throws(() => nextVersion('1.0.0-beta', []), /stable/);
});

test('synchronizes Cargo and npm manifests/locks without changing dependency versions', t => {
  const root = fixture(t);
  fs.appendFileSync(path.join(root, 'Cargo.lock'), '\n[[package]]\nname = "external"\nversion = "1.1.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n');
  synchronizeVersions(root, '1.1.1', ['vatra-fixture']);
  assert.match(fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8'), /version = "1.1.1"/);
  const cargoLock = fs.readFileSync(path.join(root, 'Cargo.lock'), 'utf8');
  assert.match(cargoLock, /name = "vatra-fixture"\nversion = "1.1.1"/);
  assert.match(cargoLock, /name = "external"\nversion = "1.1.0"/);
  const pkg = JSON.parse(fs.readFileSync(path.join(root, 'apps/desktop/package.json'), 'utf8'));
  const lock = JSON.parse(fs.readFileSync(path.join(root, 'apps/desktop/package-lock.json'), 'utf8'));
  assert.equal(pkg.version, '1.1.1');
  assert.equal(lock.version, pkg.version);
  assert.equal(lock.packages[''].version, pkg.version);
  assert.equal(lock.packages['node_modules/example'].version, '9.0.0');
});

test('creates a version commit/tag atomically and reuses that tag on retries', t => {
  const root = fixture(t);
  const remote = path.join(root, 'remote.git');
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  git('init', '--bare', '--initial-branch=main', remote);
  git('init', '--initial-branch=main');
  fs.writeFileSync(path.join(root, '.gitignore'), 'remote.git/\ntarget/\n');
  git('config', 'user.name', 'Release test');
  git('config', 'user.email', 'release-test@example.invalid');
  git('add', '.');
  git('commit', '-m', 'Fixture');
  git('remote', 'add', 'origin', remote);
  git('push', 'origin', 'main');
  const options = { root, runId: '12345', ref: 'refs/heads/main', releaseTags: ['v1.1.7'] };
  const first = prepareRelease(options);
  assert.equal(first.tag, 'v1.1.8');
  assert.equal(git('show', 'origin/main:apps/desktop/package.json'), fs.readFileSync(path.join(root, 'apps/desktop/package.json'), 'utf8').trim());
  assert.equal(git('rev-parse', 'v1.1.8^{}'), first.sha);
  execFileSync('cargo', ['metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], { cwd: root, stdio: 'pipe' });
  const second = prepareRelease(options);
  assert.deepEqual(second, first);
  assert.equal(git('log', '--oneline', '--all').split('\n').length, 2);
  git('checkout', '-B', 'main', 'origin/main');
  fs.writeFileSync(path.join(root, 'later-change.txt'), 'A later change on main');
  git('add', 'later-change.txt');
  git('commit', '-m', 'Later change');
  git('push', 'origin', 'main');
  assert.deepEqual(prepareRelease(options), first);
  assert.equal(fs.existsSync(path.join(root, 'later-change.txt')), false);
  const next = prepareRelease({ ...options, runId: '12346' });
  assert.equal(next.tag, 'v1.1.9');
  assert.equal(fs.readFileSync(path.join(root, 'later-change.txt'), 'utf8'), 'A later change on main');
  assert.throws(() => prepareRelease({ ...options, ref: 'refs/heads/feature' }), /main/);
});
