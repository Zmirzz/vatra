// A real Claude Code session through the service: two turns, the second needs the first. Uses the person's own
// Claude subscription (two very short turns), so it is run by hand, never in CI.
// Usage: node claude-two-turns.mjs <port> <token-file> <claude-binary>
import assert from 'node:assert/strict';
import {readFile, mkdtemp} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {once} from 'node:events';

const [port, tokenFile, binary] = process.argv.slice(2);
const token = (await readFile(tokenFile, 'utf8')).trim();
const socket = new WebSocket(`ws://127.0.0.1:${port}/rpc`, [`vatra.bearer.${token}`]);
await once(socket, 'open');
let id = 0;
const pending = new Map();
socket.addEventListener('message', (event) => {
  const value = JSON.parse(event.data);
  const waiting = value.id && pending.get(value.id);
  if (waiting) { pending.delete(value.id); value.error ? waiting.reject(Error(value.error.message)) : waiting.resolve(value.result); }
});
const call = (method, params = {}) => new Promise((resolve, reject) => { pending.set(++id, {resolve, reject}); socket.send(JSON.stringify({jsonrpc: '2.0', id, method, params})); });
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const known = await call('agents.list');
if (!JSON.stringify(known).includes('"claude"')) {
  await call('agents.save', {kind: 'claude', binary});
  await call('service.shutdown');
  console.log('Saved the claude agent. Start the service again and rerun this script.');
  process.exit(0);
}
const folder = await mkdtemp(join(tmpdir(), 'vatra-claude-'));
const project = await call('projects.create', {name: 'two turns', path: folder});
const session = await call('sessions.create', {project_id: project.id, agent: 'claude'});

async function turn(prompt) {
  await call('turn.start', {session_id: session.id, prompt, mode: 'supervised'});
  for (let i = 0; i < 600; i++) {
    const list = await call('sessions.list');
    if (!list.find((s) => s.id === session.id).running) return call('sessions.events', {session_id: session.id, after_seq: 0});
    await delay(500);
  }
  throw Error('The turn did not finish.');
}
const text = (events) => JSON.stringify(events);

const first = await turn('Remember this code word: PELICAN. Answer with the single word: noted. Do not use any tools.');
assert.ok(text(first).toLowerCase().includes('noted'), 'first turn answered');
const second = await turn('What was the code word? Answer with just that word. Do not use any tools.');
assert.ok(text(second).includes('PELICAN'), 'second turn remembered the first');
console.log('OK: two real Claude turns kept their context.');
await call('service.shutdown');
socket.close();
