// Routes wire protocol frames from Tandem to the session, and lets the session ask Tandem
// questions (tool calls and permission cards) whose answers come back as `response` frames.

import path from 'node:path';
import { PROTOCOL_VERSION } from './frames.mjs';
import { PERMISSION_MODES } from './policy.mjs';
import { codedError, createSession } from './session.mjs';

const CLOSING_ANSWER = { ok: false, allow: false, text: 'Tandem is closing.' };

function logError(what, error) {
  process.stderr.write(`tandem-claude-helper: ${what}: ${String(error?.message ?? error)}\n`);
}

/** Exactly one of `session_id` and `resume` names the conversation; a mode must be known. */
function checkOpenFrame(frame) {
  const named = [frame.session_id, frame.resume].filter(
    (value) => typeof value === 'string' && value !== '',
  );
  if (typeof frame.cwd !== 'string' || !path.isAbsolute(frame.cwd) || named.length !== 1) {
    throw codedError(
      'bad_request',
      'open_session needs an absolute cwd and exactly one of session_id and resume.',
    );
  }
  if (frame.permission_mode !== undefined && !PERMISSION_MODES.has(frame.permission_mode)) {
    throw codedError('bad_request', `Unknown permission mode: ${String(frame.permission_mode)}`);
  }
}

export function createHelper({ sdk, write, versions, env, onExit }) {
  const pending = new Map();
  let nextRequest = 1;
  let session = null;

  function askHost(request) {
    const id = `r${nextRequest}`;
    nextRequest += 1;
    let settle;
    const answer = new Promise((resolve) => {
      settle = resolve;
    });
    answer.requestId = id;
    pending.set(id, settle);
    try {
      write({ type: 'request', id, ...request });
    } catch (error) {
      pending.delete(id);
      settle({ ok: false, allow: false, text: `Tandem could not be asked: ${error.message}` });
    }
    return answer;
  }

  function emit(event) {
    write({ type: 'event', ...event });
  }

  function closeSession() {
    const closing = session;
    session = null;
    closing?.close();
  }

  async function handle(frame) {
    const { id } = frame;
    let replied = false;
    const reply = (data) => {
      replied = true;
      write({ type: 'reply', id, ok: true, data });
    };
    try {
      switch (frame.type) {
        case 'hello':
          reply({
            protocol: PROTOCOL_VERSION,
            helper: versions.helper,
            sdk: versions.sdk,
            claude_code: versions.claudeCode,
            node: process.version,
          });
          return;
        case 'open_session': {
          checkOpenFrame(frame);
          closeSession();
          const opening = createSession({ sdk, emit, askHost, open: frame, env });
          session = opening;
          try {
            reply(await opening.open());
          } catch (error) {
            if (session === opening) {
              session = null;
            }
            throw error;
          }
          return;
        }
        case 'turn':
          if (!session) {
            throw codedError('no_session', 'No Claude session is open.');
          }
          session.assertCanStart(frame);
          reply({});
          await session.runTurn(frame);
          return;
        case 'interrupt':
          // Not awaited: the reply means the interrupt was requested, and the turn's own
          // turn_finished event follows it.
          session?.interrupt();
          reply({});
          return;
        case 'close_session':
          closeSession();
          reply({});
          return;
        case 'shutdown':
          close();
          reply({});
          onExit(0);
          return;
        case 'response': {
          const settle = pending.get(frame.id);
          if (settle) {
            pending.delete(frame.id);
            settle(frame);
          }
          return;
        }
        default:
          if (typeof id === 'number') {
            throw codedError('bad_request', `Unknown frame type: ${frame.type}`);
          }
          return;
      }
    } catch (error) {
      if (replied || typeof id !== 'number') {
        logError(`handling ${frame.type} failed`, error);
        return;
      }
      write({
        type: 'reply',
        id,
        ok: false,
        error: { code: error.code ?? 'internal', message: String(error.message ?? error) },
      });
    }
  }

  function close() {
    closeSession();
    const waiting = [...pending.values()];
    pending.clear();
    for (const settle of waiting) {
      settle({ ...CLOSING_ANSWER });
    }
  }

  return { handle, close };
}
