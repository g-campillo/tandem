import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import * as mockSdk from './support/mock-sdk.mjs';
import { createHelper } from '../src/host.mjs';
import { GUIDED_EDIT_MESSAGE, OUTSIDE_PROJECT_MESSAGE } from '../src/policy.mjs';

const { queries } = mockSdk;
const SESSION_ID = '11111111-1111-4111-8111-111111111111';
const RATE_LIMIT_TEXT = "Your Claude plan's usage limit was reached. Try again later.";
const WAIT_MS = 2000;

let project;
let outside;

before(() => {
  project = fs.mkdtempSync(path.join(os.tmpdir(), 'tandem-session-'));
  outside = fs.mkdtempSync(path.join(os.tmpdir(), 'tandem-outside-'));
});

after(() => {
  fs.rmSync(project, { recursive: true, force: true });
  fs.rmSync(outside, { recursive: true, force: true });
});

/** A helper driven in-process with the mock SDK, collecting every frame it writes. */
function start(env = {}) {
  const frames = [];
  const exits = [];
  const listeners = new Set();
  const helper = createHelper({
    sdk: mockSdk,
    write: (frame) => {
      frames.push(frame);
      for (const listener of [...listeners]) {
        listener();
      }
    },
    versions: { helper: '1.0.0', sdk: 'mock', claudeCode: 'mock' },
    env,
    onExit: (code) => exits.push(code),
  });
  let nextId = 1;

  function send(frame) {
    const id = frame.id ?? nextId++;
    helper.handle({ v: 1, id, ...frame });
    return id;
  }

  /** Resolves with the first frame from index `from` on that matches, waiting if needed. */
  function waitFor(predicate, from = 0) {
    return new Promise((resolve, reject) => {
      const check = () => {
        const index = frames.findIndex((frame, at) => at >= from && predicate(frame));
        if (index !== -1) {
          listeners.delete(check);
          clearTimeout(timer);
          resolve(frames[index]);
        }
      };
      const timer = setTimeout(() => {
        listeners.delete(check);
        reject(new Error(`timed out; frames so far: ${JSON.stringify(frames)}`));
      }, WAIT_MS);
      listeners.add(check);
      check();
    });
  }

  async function open(overrides = {}) {
    const id = send({
      type: 'open_session',
      cwd: project,
      session_id: SESSION_ID,
      resume: null,
      use_my_setup: false,
      model: 'default',
      effort: null,
      system_append: 'x',
      ...overrides,
    });
    const reply = await waitFor(isReply(id));
    return { reply, query: queries.at(-1) };
  }

  function turn(text, policy = 'discuss', extra = {}) {
    return send({ type: 'turn', text, policy, model: 'default', effort: null, ...extra });
  }

  /** Starts a `hold` turn and waits until it has started. */
  async function holdTurn(policy, extra = {}) {
    const id = turn('hold', policy, extra);
    await waitFor(isEvent('turn_started', { turn: id }));
    return id;
  }

  async function interruptTurn(turnId) {
    send({ type: 'interrupt' });
    return waitFor(isEvent('turn_finished', { turn: turnId }));
  }

  return { helper, frames, exits, send, waitFor, open, turn, holdTurn, interruptTurn };
}

function isReply(id) {
  return (frame) => frame.type === 'reply' && frame.id === id;
}

function isEvent(name, fields = {}) {
  return (frame) =>
    frame.type === 'event' &&
    frame.event === name &&
    Object.entries(fields).every(([key, value]) => frame[key] === value);
}

function isRequest(kind) {
  return (frame) => frame.type === 'request' && frame.request === kind;
}

/** Every key and string value reachable from `value`, skipping functions. */
function everything(value, seen = new Set()) {
  if (value === null || typeof value !== 'object' || seen.has(value)) {
    return typeof value === 'string' ? [value] : [];
  }
  seen.add(value);
  return Object.entries(value).flatMap(([key, inner]) => [key, ...everything(inner, seen)]);
}

function hookOf(query) {
  return query.options.hooks.PreToolUse[0].hooks[0];
}

function writeInput(file) {
  return { hook_event_name: 'PreToolUse', tool_name: 'Write', tool_input: { file_path: file } };
}

// --- options ---------------------------------------------------------------------------

test('the SDK environment has no API key, auth token or provider switch', async () => {
  const h = start({
    ANTHROPIC_API_KEY: 'k',
    ANTHROPIC_AUTH_TOKEN: 't',
    CLAUDE_CODE_USE_BEDROCK: '1',
    PATH: '/usr/bin',
    HOME: '/home/user',
  });
  const { reply, query } = await h.open();
  assert.equal(reply.ok, true);
  assert.deepEqual(query.options.env, { PATH: '/usr/bin', HOME: '/home/user' });
});

test('the session options follow the open_session frame', async () => {
  const h = start();
  const { query } = await h.open();
  const { options } = query;
  assert.equal(options.cwd, project);
  assert.equal(options.includePartialMessages, true);
  assert.equal(typeof options.canUseTool, 'function');
  assert.deepEqual(options.systemPrompt, { type: 'preset', preset: 'claude_code', append: 'x' });
  assert.equal(options.mcpServers.tandem.name, 'tandem');
  assert.deepEqual(options.allowedTools, [
    'Grep',
    'Glob',
    'WebSearch',
    'WebFetch',
    'mcp__tandem__propose_plan',
    'mcp__tandem__show_step',
    'mcp__tandem__report_check',
    'mcp__tandem__step_done',
    'mcp__tandem__read_editor_buffer',
  ]);
  assert.deepEqual(options.disallowedTools, [
    'AskUserQuestion',
    'EnterPlanMode',
    'ExitPlanMode',
    'EnterWorktree',
    'ExitWorktree',
  ]);
  assert.equal(typeof options.stderr, 'function');
  assert.equal(options.hooks.PreToolUse[0].onFailure, 'block');
});

test('use_my_setup true loads the user setup and every skill', async () => {
  const { query } = await start().open({ use_my_setup: true });
  assert.deepEqual(query.options.settingSources, ['user', 'project', 'local']);
  assert.equal(query.options.skills, 'all');
  assert.ok(!('strictMcpConfig' in query.options));
});

test('use_my_setup false loads no settings and only the Tandem MCP server', async () => {
  const { query } = await start().open({ use_my_setup: false });
  assert.deepEqual(query.options.settingSources, []);
  assert.equal(query.options.strictMcpConfig, true);
  assert.ok(!('skills' in query.options));
});

test('a resume value gives resume and no sessionId', async () => {
  const { query } = await start().open({ session_id: null, resume: 'earlier-session' });
  assert.equal(query.options.resume, 'earlier-session');
  assert.ok(!('sessionId' in query.options));
});

test('a session_id gives sessionId and no resume', async () => {
  const { query } = await start().open();
  assert.equal(query.options.sessionId, SESSION_ID);
  assert.ok(!('resume' in query.options));
});

test('model default passes no model; a named model and effort pass through', async () => {
  const plain = await start().open({ model: 'default', effort: null });
  assert.ok(!('model' in plain.query.options));
  assert.ok(!('effort' in plain.query.options));
  const named = await start().open({ model: 'haiku', effort: 'low' });
  assert.equal(named.query.options.model, 'haiku');
  assert.equal(named.query.options.effort, 'low');
});

test('the open_session reply describes the account and models', async () => {
  const { reply } = await start().open();
  assert.deepEqual(reply, {
    type: 'reply',
    id: 1,
    ok: true,
    data: {
      auth: { ok: true },
      account: { subscription: 'Claude Ultra', provider: 'firstParty' },
      claude_code_version: '2.1.325',
      models: [
        {
          value: 'default',
          name: 'Default (recommended)',
          description: 'd',
          efforts: ['low', 'high'],
        },
        { value: 'haiku', name: 'Haiku', description: 'h', efforts: [] },
      ],
    },
  });
});

test('open_session without a project folder or session id is a bad request', async () => {
  const h = start();
  const noCwd = h.send({ type: 'open_session', session_id: SESSION_ID, resume: null });
  const both = h.send({ type: 'open_session', cwd: project, session_id: SESSION_ID, resume: 'r' });
  for (const id of [noCwd, both]) {
    const reply = await h.waitFor(isReply(id));
    assert.equal(reply.ok, false);
    assert.equal(reply.error.code, 'bad_request');
  }
});

// --- account and init gates ------------------------------------------------------------

test('an API-key account is refused and its turns never reach the SDK', async () => {
  const h = start({ MOCK_ACCOUNT: 'api_key' });
  const { reply, query } = await h.open();
  assert.equal(reply.ok, true);
  assert.equal(reply.data.auth.ok, false);
  assert.equal(reply.data.auth.reason, 'not_subscription');
  const id = h.turn('hello');
  const turnReply = await h.waitFor(isReply(id));
  assert.equal(turnReply.ok, false);
  assert.equal(turnReply.error.code, 'auth_refused');
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(query.prompts, []);
  assert.ok(!h.frames.some(isEvent('turn_started')));
  assert.ok(!h.frames.some(isEvent('session_closed')));
});

test('a signed-out account is refused as signed_out', async () => {
  const { reply } = await start({ MOCK_ACCOUNT: 'signed_out' }).open();
  assert.equal(reply.data.auth.ok, false);
  assert.equal(reply.data.auth.reason, 'signed_out');
});

test('a turn that starts with an API key is refused and so are later turns', async () => {
  const h = start({ MOCK_INIT_KEY: 'ANTHROPIC_API_KEY' });
  const { reply } = await h.open();
  assert.equal(reply.data.auth.ok, true);
  const id = h.turn('hello');
  const refused = await h.waitFor(isEvent('auth_refused'));
  assert.equal(refused.reason, 'not_subscription');
  assert.equal(refused.detail, 'Claude Code started with an API key (ANTHROPIC_API_KEY).');
  const finished = await h.waitFor(isEvent('turn_finished', { turn: id }));
  assert.equal(finished.status, 'error');
  assert.equal(finished.error, refused.detail);
  const later = h.turn('hello');
  const laterReply = await h.waitFor(isReply(later));
  assert.equal(laterReply.error.code, 'auth_refused');
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(!h.frames.some(isEvent('session_closed')));
  assert.ok(!h.frames.some(isEvent('assistant_text')));
});

test('a missing conversation to resume replies resume_failed', async () => {
  const h = start();
  const { reply } = await h.open({ session_id: null, resume: 'missing-session' });
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'resume_failed');
  assert.match(reply.error.message, /No conversation found with session ID/);
});

// --- PreToolUse hook -------------------------------------------------------------------

test('the hook denies edits in a guided turn and stays silent in a takeover turn', async () => {
  const h = start();
  const { query } = await h.open();
  const hook = hookOf(query);

  const guided = await h.holdTurn('guided');
  assert.deepEqual(await hook(writeInput('a.txt'), 'toolu_1', {}), {
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision: 'deny',
      permissionDecisionReason: GUIDED_EDIT_MESSAGE,
    },
  });
  await h.interruptTurn(guided);

  const takeover = await h.holdTurn('takeover');
  assert.deepEqual(await hook(writeInput('a.txt'), 'toolu_2', {}), {});
  assert.deepEqual(await hook(writeInput(path.join(outside, 'b.txt')), 'toolu_3', {}), {
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision: 'deny',
      permissionDecisionReason: OUTSIDE_PROJECT_MESSAGE,
    },
  });
  await h.interruptTurn(takeover);
});

test('the hook treats a call outside any turn as discuss and denies on an internal error', async () => {
  const { query } = await start().open();
  const hook = hookOf(query);
  const idle = await hook(writeInput('a.txt'), 'toolu_1', {});
  assert.equal(idle.hookSpecificOutput.permissionDecision, 'deny');
  assert.deepEqual(await hook({ hook_event_name: 'PreToolUse' }, 'toolu_2', {}), {
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision: 'deny',
      permissionDecisionReason: 'Tandem could not check this action.',
    },
  });
});

// --- canUseTool ------------------------------------------------------------------------

function signal() {
  return new AbortController().signal;
}

test('canUseTool denies an edit in a check turn without asking the host', async () => {
  const h = start();
  const { query } = await h.open();
  const id = await h.holdTurn('check');
  const result = await query.options.canUseTool(
    'Edit',
    { file_path: path.join(project, 'a.txt') },
    { signal: signal() },
  );
  assert.deepEqual(result, {
    behavior: 'deny',
    message: GUIDED_EDIT_MESSAGE,
    decisionClassification: 'user_reject',
  });
  assert.ok(!h.frames.some(isRequest('permission')));
  await h.interruptTurn(id);
});

test('canUseTool denies a takeover edit outside the project without asking the host', async () => {
  const h = start();
  const { query } = await h.open();
  const id = await h.holdTurn('takeover');
  const outsideEdit = await query.options.canUseTool(
    'Write',
    { file_path: path.join(outside, 'a.txt') },
    { signal: signal() },
  );
  const blocked = await query.options.canUseTool(
    'Edit',
    { file_path: path.join(project, 'a.txt') },
    { signal: signal(), blockedPath: path.join(outside, 'b.txt') },
  );
  const missing = await query.options.canUseTool('Edit', {}, { signal: signal() });
  for (const result of [outsideEdit, blocked, missing]) {
    assert.deepEqual(result, {
      behavior: 'deny',
      message: OUTSIDE_PROJECT_MESSAGE,
      decisionClassification: 'user_reject',
    });
  }
  assert.ok(!h.frames.some(isRequest('permission')));
  await h.interruptTurn(id);
});

test('canUseTool asks the host about a takeover edit inside the project', async () => {
  const h = start();
  const { query } = await h.open();
  const id = await h.holdTurn('takeover');
  const file = path.join(project, 'a.txt');
  const pending = query.options.canUseTool('Edit', { file_path: file }, { signal: signal() });
  const request = await h.waitFor(isRequest('permission'));
  assert.equal(request.tool, 'Edit');
  assert.equal(request.title, 'Claude wants to use Edit');
  assert.equal(request.command, null);
  assert.equal(request.path, file);
  h.send({ type: 'response', id: request.id, allow: true });
  assert.equal((await pending).behavior, 'allow');
  await h.interruptTurn(id);
});

test('canUseTool asks the host about Bash and maps allow and deny', async () => {
  const h = start();
  const { query } = await h.open();
  const input = { command: 'npm install left-pad' };

  const allowed = query.options.canUseTool('Bash', input, {
    signal: signal(),
    title: 'Claude wants to run npm install left-pad',
    description: 'Installs a package',
  });
  const first = await h.waitFor(isRequest('permission'));
  assert.deepEqual(first, {
    type: 'request',
    id: 'r1',
    request: 'permission',
    tool: 'Bash',
    title: 'Claude wants to run npm install left-pad',
    detail: 'Installs a package',
    command: 'npm install left-pad',
    path: null,
  });
  h.send({ type: 'response', id: 'r1', allow: true });
  assert.deepEqual(await allowed, {
    behavior: 'allow',
    updatedInput: input,
    decisionClassification: 'user_temporary',
  });

  const denied = query.options.canUseTool('Bash', input, {
    signal: signal(),
    decisionReason: 'Not on the allow list',
  });
  const second = await h.waitFor((frame) => isRequest('permission')(frame) && frame.id === 'r2');
  assert.equal(second.detail, 'Not on the allow list');
  assert.equal(second.title, 'Claude wants to use Bash');
  h.send({ type: 'response', id: 'r2', allow: false });
  assert.deepEqual(await denied, {
    behavior: 'deny',
    message: 'The user declined this in Tandem.',
    decisionClassification: 'user_reject',
  });
});

test('an aborted permission request is cancelled and denied', async () => {
  const h = start();
  const { query } = await h.open();
  const controller = new AbortController();
  const pending = query.options.canUseTool(
    'Bash',
    { command: 'npm install left-pad' },
    { signal: controller.signal },
  );
  const request = await h.waitFor(isRequest('permission'));
  controller.abort();
  const cancelled = await h.waitFor(isEvent('permission_cancelled'));
  assert.deepEqual(cancelled, { type: 'event', event: 'permission_cancelled', id: request.id });
  assert.deepEqual(await pending, {
    behavior: 'deny',
    message: 'The user declined this in Tandem.',
    decisionClassification: 'user_reject',
  });
});

// --- turn settings ---------------------------------------------------------------------

function settingCalls(query) {
  return query.calls.filter(([name]) => name !== 'interrupt');
}

test('turn settings are applied only when they change', async () => {
  const h = start();
  const { query } = await h.open();

  let id = h.turn('hello', 'takeover', { model: 'haiku', effort: 'low' });
  await h.waitFor(isEvent('turn_finished', { turn: id }));
  assert.deepEqual(settingCalls(query), [
    ['setModel', 'haiku'],
    ['applyFlagSettings', { effortLevel: 'low' }],
    ['setPermissionMode', 'acceptEdits'],
  ]);

  query.calls.length = 0;
  id = h.turn('hello', 'guided', { model: 'haiku', effort: 'low' });
  await h.waitFor(isEvent('turn_finished', { turn: id }));
  assert.deepEqual(settingCalls(query), [['setPermissionMode', 'default']]);

  query.calls.length = 0;
  id = h.turn('hello', 'guided', { model: 'default', effort: null });
  await h.waitFor(isEvent('turn_finished', { turn: id }));
  assert.deepEqual(settingCalls(query), [
    ['setModel', undefined],
    ['applyFlagSettings', { effortLevel: null }],
  ]);
});

test('a setting that cannot be applied becomes a notice and the turn still runs', async () => {
  const h = start();
  const { query } = await h.open();
  const original = query.query.setModel;
  query.query.setModel = () => Promise.reject(new Error('unknown model'));
  const id = h.turn('hello', 'discuss', { model: 'nope' });
  await h.waitFor(isEvent('turn_finished', { turn: id, status: 'completed' }));
  const notice = h.frames.find(isEvent('notice'));
  assert.equal(notice.text, 'Could not apply the model: unknown model');
  query.query.setModel = original;
});

// --- permission modes ------------------------------------------------------------------

const POLICY_ORDER = ['discuss', 'guided', 'check', 'takeover'];
const DISALLOWED_TOOLS = [
  'AskUserQuestion',
  'EnterPlanMode',
  'ExitPlanMode',
  'EnterWorktree',
  'ExitWorktree',
];

function bashInput(command) {
  return { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: { command } };
}

function hookDecision(permissionDecision, permissionDecisionReason) {
  return {
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision,
      ...(permissionDecisionReason ? { permissionDecisionReason } : {}),
    },
  };
}

/** Runs each `[policy, extra]` of `turns` in order, waiting until it completes. */
async function finishTurns(h, turns) {
  for (const [policy, extra] of turns) {
    const id = h.turn('hello', policy, extra);
    await h.waitFor(isEvent('turn_finished', { turn: id, status: 'completed' }));
  }
}

test('a session opened in ask or auto cannot bypass permissions', async () => {
  const today = await start().open({ use_my_setup: true });
  for (const overrides of [{}, { permission_mode: 'ask' }, { permission_mode: 'auto' }]) {
    const { query } = await start().open({ use_my_setup: true, ...overrides });
    assert.equal(query.options.permissionMode, 'default');
    const reachable = everything(query.options);
    assert.ok(!reachable.includes('allowDangerouslySkipPermissions'));
    assert.ok(!reachable.includes('bypassPermissions'));
    assert.deepEqual(Object.keys(query.options), Object.keys(today.query.options));
  }
});

test('a session opened in bypass may switch to bypassPermissions', async () => {
  const { query } = await start().open({ permission_mode: 'bypass' });
  assert.equal(query.options.allowDangerouslySkipPermissions, true);
  assert.equal(query.options.permissionMode, 'default');
});

test('every mode keeps AskUserQuestion, plan mode and worktree tools switched off', async () => {
  for (const mode of ['ask', 'auto', 'bypass']) {
    const { query } = await start().open({ permission_mode: mode });
    assert.deepEqual(query.options.disallowedTools, DISALLOWED_TOOLS);
  }
});

test('an unknown permission mode is a bad request', async () => {
  const refused = await start().open({ permission_mode: 'yolo' });
  assert.equal(refused.reply.ok, false);
  assert.equal(refused.reply.error.code, 'bad_request');

  const h = start();
  const { query } = await h.open();
  const id = h.turn('hello', 'discuss', { permission_mode: 'yolo' });
  const reply = await h.waitFor(isReply(id));
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'bad_request');
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(settingCalls(query), []);
  assert.ok(!h.frames.some(isEvent('turn_started')));
});

test('each mode sets its Claude Code permission mode', async () => {
  const everyPolicyIn = (mode) => POLICY_ORDER.map((policy) => [policy, { permission_mode: mode }]);

  const ask = start();
  const askSession = await ask.open({ permission_mode: 'ask' });
  await finishTurns(ask, everyPolicyIn('ask'));
  assert.deepEqual(settingCalls(askSession.query), [['setPermissionMode', 'acceptEdits']]);

  const auto = start();
  const autoSession = await auto.open({ permission_mode: 'ask' });
  await finishTurns(auto, everyPolicyIn('auto'));
  assert.deepEqual(settingCalls(autoSession.query), [['setPermissionMode', 'auto']]);

  const bypass = start();
  const bypassSession = await bypass.open({ permission_mode: 'bypass' });
  await finishTurns(bypass, everyPolicyIn('bypass'));
  assert.deepEqual(settingCalls(bypassSession.query), [
    ['setPermissionMode', 'bypassPermissions'],
  ]);

  const mixed = start();
  const mixedSession = await mixed.open();
  await finishTurns(mixed, [
    ['discuss', { permission_mode: 'auto' }],
    ['takeover', { permission_mode: 'ask' }],
    ['discuss', { permission_mode: 'ask' }],
    ['guided', { permission_mode: 'auto' }],
  ]);
  assert.deepEqual(settingCalls(mixedSession.query), [
    ['setPermissionMode', 'auto'],
    ['setPermissionMode', 'acceptEdits'],
    ['setPermissionMode', 'default'],
    ['setPermissionMode', 'auto'],
  ]);
  for (const h of [ask, auto, bypass, mixed]) {
    assert.ok(!h.frames.some(isEvent('permission_mode_fallback')));
  }
});

test('auto that Claude Code refuses falls back to ask and tells Tandem', async () => {
  const h = start({ MOCK_AUTO: 'unavailable' });
  const { query } = await h.open();
  const from = h.frames.length;
  const id = await h.holdTurn('discuss', { permission_mode: 'auto' });
  assert.deepEqual(h.frames.slice(from), [
    { type: 'reply', id, ok: true, data: {} },
    {
      type: 'event',
      event: 'permission_mode_fallback',
      wanted: 'auto',
      reason: 'Cannot set permission mode to auto: auto mode unavailable for this model',
    },
    { type: 'event', event: 'turn_started', turn: id },
  ]);
  assert.ok(!h.frames.some(isEvent('notice')));

  const hook = hookOf(query);
  assert.deepEqual(await hook(bashInput('cargo test'), 'toolu_1', {}), hookDecision('ask'));
  const pending = query.options.canUseTool(
    'Bash',
    { command: 'npm install left-pad' },
    { signal: signal() },
  );
  const request = await h.waitFor(isRequest('permission'), from);
  assert.equal(request.command, 'npm install left-pad');
  h.send({ type: 'response', id: request.id, allow: false });
  assert.equal((await pending).behavior, 'deny');
  await h.interruptTurn(id);
});

test('bypass in a session that was not opened for it falls back to ask', async () => {
  const h = start();
  const { query } = await h.open();
  const from = h.frames.length;
  const id = await h.holdTurn('guided', { permission_mode: 'bypass' });
  const fallback = h.frames.slice(from).find(isEvent('permission_mode_fallback'));
  assert.equal(fallback.wanted, 'bypass');
  assert.match(fallback.reason, /not launched with --dangerously-skip-permissions/);

  const hook = hookOf(query);
  assert.deepEqual(
    await hook(writeInput('a.txt'), 'toolu_1', {}),
    hookDecision('deny', GUIDED_EDIT_MESSAGE),
  );
  const pending = query.options.canUseTool(
    'Bash',
    { command: 'npm install left-pad' },
    { signal: signal() },
  );
  const request = await h.waitFor(isRequest('permission'), from);
  assert.equal(request.tool, 'Bash');
  h.send({ type: 'response', id: request.id, allow: false });
  assert.equal((await pending).behavior, 'deny');
  await h.interruptTurn(id);
});

/** Every call of the end-to-end matrix, as the SDK would make it. */
function matrixCalls() {
  const inside = path.join(project, 'a.txt');
  const outsideFile = path.join(outside, 'a.txt');
  return [
    { name: 'hook: Bash git status', via: 'hook', tool: 'Bash', input: { command: 'git status' } },
    { name: 'hook: Bash cargo test', via: 'hook', tool: 'Bash', input: { command: 'cargo test' } },
    {
      name: 'hook: Bash npm install',
      via: 'hook',
      tool: 'Bash',
      input: { command: 'npm install left-pad' },
    },
    { name: 'hook: Write inside', via: 'hook', tool: 'Write', input: { file_path: inside } },
    { name: 'hook: Write outside', via: 'hook', tool: 'Write', input: { file_path: outsideFile } },
    {
      name: 'canUseTool: Write inside',
      via: 'canUseTool',
      tool: 'Write',
      input: { file_path: inside },
    },
    {
      name: 'canUseTool: Write outside',
      via: 'canUseTool',
      tool: 'Write',
      input: { file_path: outsideFile },
    },
    {
      name: 'canUseTool: Edit with a blocked path outside',
      via: 'canUseTool',
      tool: 'Edit',
      input: { file_path: inside },
      blockedPath: path.join(outside, 'b.txt'),
    },
    {
      name: 'canUseTool: Bash npm install',
      via: 'canUseTool',
      tool: 'Bash',
      input: { command: 'npm install left-pad' },
    },
    {
      name: 'canUseTool: mcp__brain__search',
      via: 'canUseTool',
      tool: 'mcp__brain__search',
      input: { query: 'x' },
    },
  ];
}

/** The hook result for an outcome of the header's hook table. */
function expectedHook(outcome) {
  switch (outcome) {
    case 'pass':
      return {};
    case 'guided':
      return hookDecision('deny', GUIDED_EDIT_MESSAGE);
    case 'outside':
      return hookDecision('deny', OUTSIDE_PROJECT_MESSAGE);
    default:
      return hookDecision(outcome);
  }
}

/** The canUseTool result for an outcome; a card is answered with deny. */
function expectedCanUseTool(outcome, input) {
  switch (outcome) {
    case 'allow':
      return { behavior: 'allow', updatedInput: input, decisionClassification: 'user_temporary' };
    case 'guided':
      return refusal(GUIDED_EDIT_MESSAGE);
    case 'outside':
      return refusal(OUTSIDE_PROJECT_MESSAGE);
    default:
      return refusal('The user declined this in Tandem.');
  }
}

function refusal(message) {
  return { behavior: 'deny', message, decisionClassification: 'user_reject' };
}

/**
 * Opens a session in `mode` and, in a held turn of each policy, makes every matrix call and
 * compares its result with `expected[name][policy index]`. Returns the harness.
 */
async function checkMatrix(mode, expected) {
  const h = start();
  const { query } = await h.open({ permission_mode: mode });
  const hook = hookOf(query);
  for (const [index, policy] of POLICY_ORDER.entries()) {
    const id = await h.holdTurn(policy, { permission_mode: mode });
    for (const call of matrixCalls()) {
      const outcome = expected[call.name][index];
      const label = `${mode}, ${policy}, ${call.name}`;
      if (call.via === 'hook') {
        const input = {
          hook_event_name: 'PreToolUse',
          tool_name: call.tool,
          tool_input: call.input,
        };
        assert.deepEqual(await hook(input, 'toolu_1', {}), expectedHook(outcome), label);
        continue;
      }
      const from = h.frames.length;
      const pending = query.options.canUseTool(call.tool, call.input, {
        signal: signal(),
        ...(call.blockedPath ? { blockedPath: call.blockedPath } : {}),
      });
      // A card is written as soon as canUseTool is called.
      const card = h.frames.slice(from).find(isRequest('permission'));
      if (outcome === 'card') {
        assert.ok(card, `${label}: no permission request`);
        assert.equal(card.tool, call.tool, label);
        h.send({ type: 'response', id: card.id, allow: false });
      } else {
        assert.equal(card, undefined, `${label}: unexpected permission request`);
      }
      assert.deepEqual(await pending, expectedCanUseTool(outcome, call.input), label);
    }
    await h.interruptTurn(id);
  }
  assert.ok(!h.frames.some(isEvent('permission_mode_fallback')));
  return h;
}

function four(outcome) {
  return [outcome, outcome, outcome, outcome];
}

/** The same outcome in discuss, guided and check, and another in takeover. */
function untilTakeover(outcome, takeover) {
  return [outcome, outcome, outcome, takeover];
}

test('ask: cards and refusals are as before in every policy', async () => {
  await checkMatrix('ask', {
    'hook: Bash git status': four('allow'),
    'hook: Bash cargo test': ['ask', 'ask', 'allow', 'allow'],
    'hook: Bash npm install': untilTakeover('ask', 'pass'),
    'hook: Write inside': untilTakeover('guided', 'pass'),
    'hook: Write outside': untilTakeover('guided', 'outside'),
    'canUseTool: Write inside': untilTakeover('guided', 'card'),
    'canUseTool: Write outside': untilTakeover('guided', 'outside'),
    'canUseTool: Edit with a blocked path outside': untilTakeover('guided', 'outside'),
    'canUseTool: Bash npm install': four('card'),
    'canUseTool: mcp__brain__search': four('card'),
  });
});

test('auto: Tandem adds no card of its own', async () => {
  await checkMatrix('auto', {
    'hook: Bash git status': four('allow'),
    'hook: Bash cargo test': four('allow'),
    'hook: Bash npm install': four('pass'),
    'hook: Write inside': untilTakeover('guided', 'pass'),
    'hook: Write outside': untilTakeover('guided', 'outside'),
    'canUseTool: Write inside': untilTakeover('guided', 'card'),
    'canUseTool: Write outside': untilTakeover('guided', 'outside'),
    'canUseTool: Edit with a blocked path outside': untilTakeover('guided', 'outside'),
    'canUseTool: Bash npm install': four('card'),
    'canUseTool: mcp__brain__search': four('card'),
  });
});

test('bypass: nothing reaches Tandem as a permission request in any policy', async () => {
  const h = await checkMatrix('bypass', {
    'hook: Bash git status': four('allow'),
    'hook: Bash cargo test': four('allow'),
    'hook: Bash npm install': four('pass'),
    'hook: Write inside': four('pass'),
    'hook: Write outside': four('outside'),
    'canUseTool: Write inside': four('allow'),
    'canUseTool: Write outside': four('outside'),
    'canUseTool: Edit with a blocked path outside': four('outside'),
    'canUseTool: Bash npm install': four('allow'),
    'canUseTool: mcp__brain__search': four('allow'),
  });
  assert.deepEqual(h.frames.filter(isRequest('permission')), []);
});

test('outside a turn the hook and canUseTool use ask, also in a bypass session', async () => {
  const h = start();
  const { query } = await h.open({ permission_mode: 'bypass' });
  const hook = hookOf(query);
  assert.deepEqual(
    await hook(writeInput('a.txt'), 'toolu_1', {}),
    hookDecision('deny', GUIDED_EDIT_MESSAGE),
  );
  const pending = query.options.canUseTool(
    'Bash',
    { command: 'npm install left-pad' },
    { signal: signal() },
  );
  const request = await h.waitFor(isRequest('permission'));
  assert.equal(request.tool, 'Bash');
  h.send({ type: 'response', id: request.id, allow: false });
  assert.equal((await pending).behavior, 'deny');
});

// --- events ----------------------------------------------------------------------------

test('the plan scenario produces its events in order after the turn reply', async () => {
  const h = start();
  await h.open();
  const from = h.frames.length;
  const id = h.turn('plan');
  const request = await h.waitFor(isRequest('tool'));
  assert.deepEqual(request, {
    type: 'request',
    id: 'r1',
    request: 'tool',
    name: 'propose_plan',
    input: {
      goal: 'g',
      done_when: ['d'],
      steps: [{ kind: 'edit_file', target: 'src/cli.rs', summary: 's', language: 'rust' }],
    },
  });
  h.send({ type: 'response', id: 'r1', ok: true, text: 'shown' });
  await h.waitFor(isEvent('turn_finished', { turn: id }));
  assert.deepEqual(h.frames.slice(from), [
    { type: 'reply', id, ok: true, data: {} },
    { type: 'event', event: 'turn_started', turn: id },
    { type: 'event', event: 'text_delta', text: 'Here' },
    { type: 'event', event: 'activity', kind: 'read', label: 'src/cli.rs' },
    request,
    { type: 'event', event: 'assistant_text', text: 'Here is the plan.' },
    { type: 'event', event: 'turn_finished', turn: id, status: 'completed', error: null },
  ]);
});

test('a rate-limited turn finishes with the friendly text', async () => {
  const h = start();
  await h.open();
  const id = h.turn('limit');
  const finished = await h.waitFor(isEvent('turn_finished', { turn: id }));
  assert.equal(finished.status, 'error');
  assert.equal(finished.error, RATE_LIMIT_TEXT);
});

test('an interrupted turn finishes as interrupted', async () => {
  const h = start();
  await h.open();
  const id = await h.holdTurn('takeover');
  const finished = await h.interruptTurn(id);
  assert.equal(finished.status, 'interrupted');
  assert.equal(finished.error, null);
});

test('a turn stopped while its settings are applied finishes as interrupted without reaching Claude', async () => {
  const h = start();
  const { query } = await h.open();
  const original = query.query.setPermissionMode;
  const modeRequested = new Promise((requested) => {
    query.query.setPermissionMode = () =>
      new Promise((applied) => {
        requested(applied);
      });
  });
  const from = h.frames.length;
  const id = h.turn('hello', 'takeover');
  const applyMode = await modeRequested;
  const stop = h.send({ type: 'interrupt' });
  applyMode();
  await h.waitFor(isEvent('turn_finished', { turn: id }));
  query.query.setPermissionMode = original;
  assert.deepEqual(h.frames.slice(from), [
    { type: 'reply', id, ok: true, data: {} },
    { type: 'reply', id: stop, ok: true, data: {} },
    { type: 'event', event: 'turn_started', turn: id },
    { type: 'event', event: 'turn_finished', turn: id, status: 'interrupted', error: null },
  ]);
  assert.deepEqual(query.prompts, []);

  const next = h.turn('hello');
  const finished = await h.waitFor(isEvent('turn_finished', { turn: next }));
  assert.equal(finished.status, 'completed');
});

test('a second turn while one runs replies busy', async () => {
  const h = start();
  await h.open();
  const id = await h.holdTurn('discuss');
  const second = h.turn('hello');
  const reply = await h.waitFor(isReply(second));
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'busy');
  await h.interruptTurn(id);
});

test('a turn before open_session replies no_session', async () => {
  const h = start();
  const id = h.turn('hello');
  const reply = await h.waitFor(isReply(id));
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'no_session');
});

test('a turn with an unknown policy or empty text is a bad request', async () => {
  const h = start();
  await h.open();
  for (const id of [h.turn('hello', 'yolo'), h.turn('', 'discuss')]) {
    const reply = await h.waitFor(isReply(id));
    assert.equal(reply.error.code, 'bad_request');
  }
});

test('Claude Code stopping during a turn finishes the turn and closes the session', async () => {
  const h = start();
  const { query } = await h.open();
  const id = await h.holdTurn('discuss');
  query.query.close();
  const finished = await h.waitFor(isEvent('turn_finished', { turn: id }));
  assert.equal(finished.status, 'error');
  const closed = await h.waitFor(isEvent('session_closed'));
  assert.equal(typeof closed.reason, 'string');
});

test('close_session closes the query without a session_closed event', async () => {
  const h = start();
  const { query } = await h.open();
  const id = h.send({ type: 'close_session' });
  assert.deepEqual(await h.waitFor(isReply(id)), { type: 'reply', id, ok: true, data: {} });
  const next = h.turn('hello');
  assert.equal((await h.waitFor(isReply(next))).error.code, 'no_session');
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(!h.frames.some(isEvent('session_closed')));
  assert.deepEqual(query.prompts, []);
});

// --- host frames -----------------------------------------------------------------------

test('hello replies with the versions', async () => {
  const h = start();
  const id = h.send({ type: 'hello' });
  assert.deepEqual(await h.waitFor(isReply(id)), {
    type: 'reply',
    id,
    ok: true,
    data: { protocol: 1, helper: '1.0.0', sdk: 'mock', claude_code: 'mock', node: process.version },
  });
});

test('an unknown frame type with an id is a bad request', async () => {
  const h = start();
  const id = h.send({ type: 'dance' });
  const reply = await h.waitFor(isReply(id));
  assert.equal(reply.ok, false);
  assert.equal(reply.error.code, 'bad_request');
});

test('a response with an unknown id is ignored', async () => {
  const h = start();
  h.send({ type: 'response', id: 'r99', ok: true, text: 'x' });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(h.frames, []);
});

test('shutdown replies and exits with code 0', async () => {
  const h = start();
  await h.open();
  const id = h.send({ type: 'shutdown' });
  assert.deepEqual(await h.waitFor(isReply(id)), { type: 'reply', id, ok: true, data: {} });
  assert.deepEqual(h.exits, [0]);
});

test('close answers every pending request', async () => {
  const h = start();
  const { query } = await h.open();
  const pending = query.options.canUseTool('Bash', { command: 'make install' }, { signal: signal() });
  await h.waitFor(isRequest('permission'));
  h.helper.close();
  assert.equal((await pending).behavior, 'deny');
});
