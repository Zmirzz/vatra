import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { eligible, inspect, runCleanup } from './cleanup.mjs';

function fixture(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'vatra-cleanup-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  fs.writeFileSync(path.join(directory, 'Cargo.toml'), '[package]\nname="cleanup-fixture"\nversion="0.1.0"\nedition="2024"\n[workspace]\n');
  fs.mkdirSync(path.join(directory, 'src'));
  fs.writeFileSync(path.join(directory, 'src', 'lib.rs'), '');
  fs.mkdirSync(path.join(directory, 'target', 'debug'), { recursive: true });
  fs.writeFileSync(path.join(directory, 'target', 'CACHEDIR.TAG'), 'Signature: 8a477f597d28d172789f06886806bc55\n');
  fs.mkdirSync(path.join(directory, 'target', 'release', 'bundle'), { recursive: true });
  fs.writeFileSync(path.join(directory, 'target', 'debug', 'cache'), 'cache');
  fs.writeFileSync(path.join(directory, 'target', 'release', 'bundle', 'installer.exe'), 'installer');
  fs.writeFileSync(path.join(directory, 'user.db'), 'data');
  return directory;
}

test('automatic policy requires both size and inactivity', () => {
  const now = Date.now(), old = now - 25 * 3600_000;
  assert.equal(eligible({ bytes: 21 * 2 ** 30, newest: old }, now), true);
  assert.equal(eligible({ bytes: 20 * 2 ** 30, newest: old }, now), false);
  assert.equal(eligible({ bytes: 300 * 2 ** 30, newest: now }, now), false);
});

test('preview, recent auto cache and busy builds never invoke Cargo', t => {
  const repository = fixture(t);
  const run = () => { throw new Error('Must not invoke Cargo'); };
  runCleanup({ repository, run });
  runCleanup({ repository, auto: true, run });
  runCleanup({ repository, apply: true, busy: () => true, run });
  assert.ok(fs.existsSync(path.join(repository, 'target', 'debug', 'cache')));
});

test('target junctions and nested junctions are rejected without touching destination', t => {
  const repository = fixture(t), outside = fixture(t);
  fs.symlinkSync(outside, path.join(repository, 'target', 'debug', 'outside'), 'junction');
  assert.throws(() => inspect(path.join(repository, 'target', 'debug')), /linked path/);
  const second = fixture(t);
  fs.renameSync(path.join(second, 'target'), path.join(second, 'original-target'));
  fs.symlinkSync(path.join(outside, 'target'), path.join(second, 'target'), 'junction');
  assert.throws(() => runCleanup({ repository: second, apply: true }), /linked target/);
  assert.ok(fs.existsSync(path.join(outside, 'target', 'debug', 'cache')));
});

test('concurrent cleanup is skipped', t => {
  const repository = fixture(t);
  fs.writeFileSync(path.join(repository, '.cleanup.lock'), '');
  runCleanup({ repository, apply: true, busy: () => false,
    run: () => { throw new Error('Must not invoke Cargo'); } });
});

test('failed Cargo cleanup releases the cleanup lock', t => {
  const repository = fixture(t);
  assert.throws(() => runCleanup({ repository, apply: true, busy: () => false,
    run: () => ({ status: 1 }) }), /cargo clean failed/);
  assert.equal(fs.existsSync(path.join(repository, '.cleanup.lock')), false);
});

test('real Cargo removes only debug artifacts and preserves release, sources and data', t => {
  const repository = fixture(t);
  runCleanup({ repository, apply: true, busy: () => false, run: spawnSync });
  assert.equal(fs.existsSync(path.join(repository, 'target', 'debug', 'cache')), false);
  assert.equal(fs.readFileSync(path.join(repository, 'target', 'release', 'bundle', 'installer.exe'), 'utf8'), 'installer');
  assert.equal(fs.readFileSync(path.join(repository, 'user.db'), 'utf8'), 'data');
  assert.equal(fs.readFileSync(path.join(repository, 'src', 'lib.rs'), 'utf8'), '');
  assert.equal(fs.existsSync(path.join(repository, '.cleanup.lock')), false);
});
