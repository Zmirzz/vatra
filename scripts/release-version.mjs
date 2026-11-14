// Prepare a manual GitHub release. No dependencies, builds, or private credentials needed.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const stable = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const versionFiles = ['Cargo.toml', 'Cargo.lock', 'apps/desktop/package.json', 'apps/desktop/package-lock.json'];

export function nextVersion(current, tags) {
  if (!stable.test(current)) throw new Error('Application version must be a stable major.minor.patch version');
  const versions = [current, ...tags.filter(tag => /^v/.test(tag)).map(tag => tag.slice(1)).filter(v => stable.test(v))];
  const highest = versions.map(v => v.split('.').map(BigInt)).sort((a, b) => {
    for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i] ? -1 : 1;
    return 0;
  })[0];
  highest[2] += 1n;
  return highest.join('.');
}

export function synchronizeVersions(root, version, workspacePackages) {
  if (!stable.test(version)) throw new Error('Expected a stable release version');
  const read = name => fs.readFileSync(path.join(root, name), 'utf8');
  const cargo = read('Cargo.toml');
  const workspaceVersion = /(\[workspace\.package\][\s\S]*?\bversion\s*=\s*")[^"]+("[^\r\n]*)/;
  if (!workspaceVersion.test(cargo)) throw new Error('Workspace package version not found');
  const names = new Set(workspacePackages);
  const found = new Set();
  const cargoLock = read('Cargo.lock').split(/(?=^\[\[package\]\])/m).map(block => {
    const name = block.match(/^name = "([^"]+)"/m)?.[1];
    if (!names.has(name) || /^source = /m.test(block)) return block;
    found.add(name);
    return block.replace(/^version = "[^"]+"/m, `version = "${version}"`);
  }).join('');
  for (const name of names) if (!found.has(name)) throw new Error(`Workspace package missing from Cargo.lock: ${name}`);
  const pkg = JSON.parse(read('apps/desktop/package.json'));
  const lock = JSON.parse(read('apps/desktop/package-lock.json'));
  if (!lock.packages?.['']) throw new Error('npm lockfile root package not found');
  pkg.version = lock.version = lock.packages[''].version = version;
  // Validate all inputs before modifying any files.
  const contents = [cargo.replace(workspaceVersion, (_, prefix, suffix) => prefix + version + suffix), cargoLock,
    JSON.stringify(pkg, null, 2) + '\n', JSON.stringify(lock, null, 2) + '\n'];
  versionFiles.forEach((name, i) => fs.writeFileSync(path.join(root, name), contents[i]));
}

export function prepareRelease({ root, runId, ref, repository, releaseTags }) {
  if (ref !== 'refs/heads/main') throw new Error('Automatic releases must run from main');
  if (!/^\d+$/.test(runId ?? '')) throw new Error('A GitHub workflow run ID is required');
  const run = (command, args) => execFileSync(command, args, {
    cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
  }).trim();
  const git = (...args) => run('git', args);
  if (git('status', '--porcelain')) throw new Error('Release preparation requires a clean checkout');
  git('fetch', 'origin', 'main', '--tags');
  const marker = `Vatra release run ${runId}`;
  const existing = git('for-each-ref', '--format=%(refname:short)|%(contents:subject)', 'refs/tags/v*')
    .split('\n').filter(line => line.endsWith(`|${marker}`)).map(line => line.split('|')[0]);
  if (existing.length > 1) throw new Error('Multiple release tags belong to this workflow run');
  if (existing.length) {
    // A retry checks out the same version commit, even if main has moved since the first attempt.
    const tag = existing[0];
    if (!stable.test(tag.slice(1))) throw new Error('Invalid release tag for this workflow run');
    git('checkout', '--detach', tag);
    return { tag, sha: git('rev-parse', 'HEAD') };
  }
  git('checkout', '--detach', 'origin/main');
  const current = JSON.parse(fs.readFileSync(path.join(root, 'apps/desktop/package.json'), 'utf8')).version;
  const tags = git('tag', '--list', 'v*').split('\n');
  const releases = releaseTags ?? run('gh', ['api', '--paginate', `repos/${repository}/releases`, '--jq', '.[].tag_name']).split('\n');
  const version = nextVersion(current, [...tags, ...releases]);
  const metadata = JSON.parse(run('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1']));
  const members = new Set(metadata.workspace_members);
  const packages = metadata.packages.filter(pkg => members.has(pkg.id));
  if (packages.some(pkg => pkg.version !== current)) throw new Error('Workspace package versions must match the desktop version');
  synchronizeVersions(root, version, packages.map(pkg => pkg.name));
  const tag = `v${version}`;
  git('add', '--', ...versionFiles);
  const identity = ['-c', 'user.name=github-actions[bot]', '-c', 'user.email=41898282+github-actions[bot]@users.noreply.github.com'];
  git(...identity, 'commit', '-m', `Release ${tag}`);
  git(...identity, 'tag', '-a', tag, '-m', marker);
  // A concurrent main update or tag collision fails without partially updating the remote.
  // GITHUB_TOKEN pushes do not trigger another workflow, so this tag does not build twice.
  git('push', '--atomic', 'origin', 'HEAD:refs/heads/main', `refs/tags/${tag}`);
  return { tag, sha: git('rev-parse', 'HEAD') };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const result = prepareRelease({
      root: path.resolve(fileURLToPath(new URL('..', import.meta.url))),
      runId: process.env.GITHUB_RUN_ID,
      ref: process.env.GITHUB_REF,
      repository: process.env.GITHUB_REPOSITORY,
    });
    if (!process.env.GITHUB_OUTPUT) throw new Error('GITHUB_OUTPUT is required');
    fs.appendFileSync(process.env.GITHUB_OUTPUT, `tag=${result.tag}\nsha=${result.sha}\n`);
    console.log(`Prepared ${result.tag} at ${result.sha}`);
  } catch (error) {
    // Do not echo child process output: these commands operate on an authenticated checkout.
    console.error(error instanceof Error ? error.message : 'Release preparation failed');
    process.exitCode = 1;
  }
}
