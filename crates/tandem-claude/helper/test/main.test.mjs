import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createFrameReader } from '../src/frames.mjs';

const HELPER_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const FIXTURE = path.join(HELPER_DIR, 'test', 'fixtures', 'contract.jsonl');
const PLACEHOLDERS = new Set(['<helper>', '<node>']);
const WAIT_MS = 5000;

/** Starts main.mjs with the mock SDK and collects what it writes. */
function spawnHelper(extraEnv = {}) {
  const env = { ...process.env, ...extraEnv };
  delete env.MOCK_ACCOUNT;
  delete env.MOCK_INIT_KEY;
  delete env.MOCK_AUTO;
  const child = spawn(process.execPath, ['main.mjs', '--sdk=test/support/mock-sdk.mjs'], {
    cwd: HELPER_DIR,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const frames = [];
  const readerErrors = [];
  const listeners = new Set();
  let stdout = '';
  let stderr = '';
  const reader = createFrameReader(
    (frame) => {
      frames.push(frame);
      for (const listener of [...listeners]) {
        listener();
      }
    },
    (error) => readerErrors.push(error),
  );
  child.stdout.on('data', (chunk) => {
    stdout += chunk.toString('utf8');
    reader.push(chunk);
  });
  child.stdout.on('end', () => reader.end());
  child.stderr.on('data', (chunk) => {
    stderr += chunk.toString('utf8');
  });
  const exited = new Promise((resolve) => {
    child.on('close', (code, signal) => resolve({ code, signal }));
  });

  /** Resolves true once `count` frames have arrived, or false after a timeout. */
  function frameCount(count) {
    return new Promise((resolve) => {
      const check = () => {
        if (frames.length >= count) {
          listeners.delete(check);
          clearTimeout(timer);
          resolve(true);
        }
      };
      const timer = setTimeout(() => {
        listeners.delete(check);
        resolve(false);
      }, WAIT_MS);
      listeners.add(check);
      check();
    });
  }

  function send(frame) {
    child.stdin.write(`${JSON.stringify(frame)}\n`);
  }

  return {
    child,
    frames,
    readerErrors,
    exited,
    frameCount,
    send,
    get stdout() {
      return stdout;
    },
    get stderr() {
      return stderr;
    },
  };
}

/** `expected` with each placeholder replaced by the value received, when that is a string. */
function fillPlaceholders(expected, actual) {
  if (typeof expected === 'string' && PLACEHOLDERS.has(expected)) {
    return typeof actual === 'string' ? actual : expected;
  }
  if (Array.isArray(expected)) {
    return expected.map((value, index) => fillPlaceholders(value, actual?.[index]));
  }
  if (expected !== null && typeof expected === 'object') {
    return Object.fromEntries(
      Object.entries(expected).map(([key, value]) => [key, fillPlaceholders(value, actual?.[key])]),
    );
  }
  return expected;
}

function readContract() {
  return fs
    .readFileSync(FIXTURE, 'utf8')
    .split('\n')
    .filter((line) => line.trim() !== '')
    .map((line) => JSON.parse(line));
}

function withTimeout(promise, what) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), WAIT_MS);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

test('the contract fixture lists frames in both directions', () => {
  const entries = readContract();
  assert.ok(entries.length > 0);
  for (const entry of entries) {
    assert.ok(['to_helper', 'from_helper'].includes(entry.dir), JSON.stringify(entry));
    assert.equal(entry.frame.v, 1);
    assert.equal(typeof entry.frame.type, 'string');
  }
});

test('main.mjs replays the contract conversation exactly', async () => {
  const helper = spawnHelper({ MOCK_LOG: 'mock sdk says hi' });
  const expected = [];
  let complete = true;
  for (const entry of readContract()) {
    if (entry.dir === 'from_helper') {
      expected.push(entry.frame);
      continue;
    }
    if (!(await helper.frameCount(expected.length))) {
      complete = false;
      break;
    }
    helper.send(entry.frame);
  }
  complete = complete && (await helper.frameCount(expected.length));
  if (!complete) {
    helper.child.kill();
  }
  const { code } = await withTimeout(helper.exited, 'the helper to exit');

  assert.deepEqual(
    helper.frames,
    expected.map((frame, index) => fillPlaceholders(frame, helper.frames[index])),
    `stderr: ${helper.stderr}`,
  );
  assert.equal(code, 0, `stderr: ${helper.stderr}`);
  const hello = helper.frames[0].data;
  assert.equal(hello.helper, '1.0.0');
  assert.equal(hello.node, process.version);

  // Only frames reached stdout; the mock SDK's console output went to stderr.
  assert.deepEqual(helper.readerErrors, []);
  const lines = helper.stdout.split('\n');
  assert.equal(lines.pop(), '');
  assert.equal(lines.length, helper.frames.length);
  for (const line of lines) {
    assert.equal(JSON.parse(line).v, 1);
  }
  assert.match(helper.stderr, /mock sdk says hi log/);
  assert.match(helper.stderr, /mock sdk says hi info/);
  assert.match(helper.stderr, /mock sdk says hi warn/);
});

test('a line that is not JSON makes the helper exit with code 2', async () => {
  const helper = spawnHelper();
  helper.child.stdin.write('not json\n');
  const { code } = await withTimeout(helper.exited, 'the helper to exit');
  assert.equal(code, 2);
  assert.match(helper.stderr, /invalid frame/);
  assert.equal(helper.stdout, '');
});

test('closing stdin makes the helper exit with code 0', async () => {
  const helper = spawnHelper();
  helper.send({ v: 1, type: 'hello', id: 1 });
  assert.ok(await helper.frameCount(1));
  helper.child.stdin.end();
  const { code } = await withTimeout(helper.exited, 'the helper to exit');
  assert.equal(code, 0, `stderr: ${helper.stderr}`);
  assert.equal(helper.frames.length, 1);
  assert.equal(helper.frames[0].type, 'reply');
});

test('closing stdin with an open session closes it and exits with code 0', async () => {
  const helper = spawnHelper();
  helper.send({
    v: 1,
    type: 'open_session',
    id: 1,
    cwd: '/tmp/tandem-contract',
    session_id: '11111111-1111-4111-8111-111111111111',
    resume: null,
    use_my_setup: false,
    model: 'default',
    effort: null,
    system_append: 'x',
  });
  assert.ok(await helper.frameCount(1));
  helper.send({
    v: 1,
    type: 'turn',
    id: 2,
    text: 'hold',
    policy: 'discuss',
    model: 'default',
    effort: null,
  });
  assert.ok(await helper.frameCount(3));
  helper.child.stdin.end();
  const { code } = await withTimeout(helper.exited, 'the helper to exit');
  assert.equal(code, 0, `stderr: ${helper.stderr}`);
});
