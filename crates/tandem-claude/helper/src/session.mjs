// One long-lived Claude Agent SDK session per task. Turns are pushed into a streaming input
// queue so the same session serves discuss, guided, check and takeover turns.

import { checkAccount, checkInit } from './auth.mjs';
import { activityFor } from './activity.mjs';
import { scrubEnv } from './env.mjs';
import {
  EDIT_TOOLS,
  GUIDED_EDIT_MESSAGE,
  OUTSIDE_PROJECT_MESSAGE,
  decide,
  hookOutput,
  insideProject,
} from './policy.mjs';
import { TANDEM_TOOL_NAMES, createTandemServer } from './tools.mjs';

const POLICIES = new Set(['discuss', 'guided', 'check', 'takeover']);
const HOOK_FAILURE_MESSAGE = 'Tandem could not check this action.';
const DECLINED_MESSAGE = 'The user declined this in Tandem.';
const STOPPED_MESSAGE = 'Claude Code stopped.';
const MISSING_CONVERSATION = /No conversation found with session ID/;

const FRIENDLY_ERRORS = {
  authentication_failed: 'Claude is signed out. Run `claude auth login`, then press Reconnect.',
  billing_error: 'Claude reported a billing problem with this account.',
  rate_limit: "Your Claude plan's usage limit was reached. Try again later.",
  overloaded: 'Claude is overloaded right now. Try again.',
};

const DECLINED = {
  behavior: 'deny',
  message: DECLINED_MESSAGE,
  decisionClassification: 'user_reject',
};

/** An error carrying a wire protocol error code. */
export function codedError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

function messageOf(error) {
  return String(error?.message ?? error);
}

/**
 * The SDK's prompt: yields pushed user messages, waits while empty, and returns only after
 * `end()`. Keeping it open keeps the session alive between turns.
 */
function createInputQueue() {
  const items = [];
  let ended = false;
  let wake = null;

  function notify() {
    const resume = wake;
    wake = null;
    resume?.();
  }

  return {
    push(item) {
      if (!ended) {
        items.push(item);
        notify();
      }
    },
    end() {
      ended = true;
      notify();
    },
    async *[Symbol.asyncIterator]() {
      for (;;) {
        if (ended) {
          return;
        }
        if (items.length > 0) {
          yield items.shift();
        } else {
          await new Promise((resolve) => {
            wake = resolve;
          });
        }
      }
    },
  };
}

function deny(message) {
  return { behavior: 'deny', message, decisionClassification: 'user_reject' };
}

export function createSession({ sdk, emit, askHost, open: openFrame, env }) {
  const cwd = openFrame.cwd;
  const queue = createInputQueue();
  let q = null;
  let current = null;
  let refused = false;
  let ready = false;
  let closed = false;
  let assistantError = null;
  const applied = {
    model: openFrame.model ?? 'default',
    effort: openFrame.effort ?? null,
    mode: 'default',
  };

  async function preToolUse(input) {
    try {
      return hookOutput(
        decide({
          policy: current?.policy ?? 'discuss',
          tool: input.tool_name,
          input: input.tool_input ?? {},
          cwd,
        }),
      );
    } catch {
      return hookOutput({ decision: 'deny', reason: HOOK_FAILURE_MESSAGE });
    }
  }

  function canUseTool(toolName, input, options = {}) {
    const args = input ?? {};
    if (EDIT_TOOLS.has(toolName)) {
      if ((current?.policy ?? 'discuss') !== 'takeover') {
        return Promise.resolve(deny(GUIDED_EDIT_MESSAGE));
      }
      const target = args.file_path ?? args.notebook_path;
      const blocked = options.blockedPath;
      if (
        typeof target !== 'string' ||
        target === '' ||
        !insideProject(cwd, target) ||
        (blocked && !insideProject(cwd, blocked))
      ) {
        return Promise.resolve(deny(OUTSIDE_PROJECT_MESSAGE));
      }
    }
    const answer = askHost({
      request: 'permission',
      tool: toolName,
      title: options.title ?? `Claude wants to use ${toolName}`,
      detail: options.description ?? options.decisionReason ?? '',
      command: toolName === 'Bash' ? String(args.command ?? '') : null,
      path: options.blockedPath ?? args.file_path ?? null,
    });
    const { signal } = options;
    return new Promise((resolve) => {
      let settled = false;
      const cancel = () => {
        if (settled) {
          return;
        }
        settled = true;
        emit({ event: 'permission_cancelled', id: answer.requestId });
        resolve({ ...DECLINED });
      };
      if (signal?.aborted) {
        cancel();
        return;
      }
      signal?.addEventListener('abort', cancel, { once: true });
      answer.then((response) => {
        if (settled) {
          return;
        }
        settled = true;
        signal?.removeEventListener('abort', cancel);
        resolve(
          response?.allow === true
            ? { behavior: 'allow', updatedInput: args, decisionClassification: 'user_temporary' }
            : { ...DECLINED },
        );
      });
    });
  }

  function buildOptions() {
    return {
      cwd,
      env: scrubEnv(env ?? {}),
      includePartialMessages: true,
      permissionMode: 'default',
      canUseTool,
      hooks: { PreToolUse: [{ hooks: [preToolUse], onFailure: 'block' }] },
      mcpServers: { tandem: createTandemServer({ sdk, askHost }) },
      allowedTools: ['Grep', 'Glob', 'WebSearch', 'WebFetch', ...TANDEM_TOOL_NAMES],
      disallowedTools: [
        'AskUserQuestion',
        'EnterPlanMode',
        'ExitPlanMode',
        'EnterWorktree',
        'ExitWorktree',
      ],
      systemPrompt: { type: 'preset', preset: 'claude_code', append: openFrame.system_append },
      settingSources: openFrame.use_my_setup ? ['user', 'project', 'local'] : [],
      ...(openFrame.use_my_setup ? { skills: 'all' } : { strictMcpConfig: true }),
      ...(openFrame.model && openFrame.model !== 'default' ? { model: openFrame.model } : {}),
      ...(openFrame.effort ? { effort: openFrame.effort } : {}),
      // Claude Code exits with an error when given both, so exactly one is passed.
      ...(openFrame.resume ? { resume: openFrame.resume } : { sessionId: openFrame.session_id }),
      stderr: (text) => process.stderr.write(text),
    };
  }

  function finishTurn(status, error) {
    if (!current) {
      return;
    }
    const turn = current.id;
    current = null;
    assistantError = null;
    emit({ event: 'turn_finished', turn, status, error: status === 'error' ? error : null });
  }

  /** Ends the session on purpose: nothing more reaches the SDK and no session_closed is sent. */
  function stop() {
    closed = true;
    queue.end();
    try {
      q?.close();
    } catch (error) {
      process.stderr.write(`tandem-claude-helper: closing the session failed: ${messageOf(error)}\n`);
    }
  }

  function onInit(message) {
    const verdict = checkInit(message);
    if (verdict.ok) {
      return;
    }
    emit({ event: 'auth_refused', reason: verdict.reason, detail: verdict.detail });
    finishTurn('error', verdict.detail);
    refused = true;
    stop();
  }

  function onAssistant(message) {
    if (message.error) {
      assistantError = message.error;
    }
    const content = message.message?.content;
    if (!Array.isArray(content)) {
      return;
    }
    for (const block of content) {
      if (
        block?.type === 'text' &&
        message.parent_tool_use_id === null &&
        typeof block.text === 'string' &&
        block.text !== ''
      ) {
        emit({ event: 'assistant_text', text: block.text });
      } else if (block?.type === 'tool_use') {
        const activity = activityFor(block.name, block.input ?? {}, cwd);
        if (activity) {
          emit({ event: 'activity', kind: activity.kind, label: activity.label });
        }
      }
    }
  }

  function onResult(message) {
    if (!current) {
      return;
    }
    let status = 'completed';
    if (current.interrupting) {
      status = 'interrupted';
    } else if (message.is_error) {
      status = 'error';
    }
    let error = null;
    if (status === 'error') {
      if (assistantError) {
        error = FRIENDLY_ERRORS[assistantError] ?? assistantError;
      } else if (Array.isArray(message.errors) && message.errors.length > 0) {
        error = message.errors.join('; ');
      } else {
        error = message.subtype ?? 'error';
      }
    }
    finishTurn(status, error);
  }

  function onMessage(message) {
    switch (message?.type) {
      case 'system':
        if (message.subtype === 'init') {
          onInit(message);
        }
        break;
      case 'stream_event': {
        const event = message.event;
        if (
          message.parent_tool_use_id === null &&
          event?.type === 'content_block_delta' &&
          event.delta?.type === 'text_delta' &&
          typeof event.delta.text === 'string'
        ) {
          emit({ event: 'text_delta', text: event.delta.text });
        }
        break;
      }
      case 'assistant':
        onAssistant(message);
        break;
      case 'result':
        onResult(message);
        break;
      default:
        break;
    }
  }

  async function pump() {
    let failure = null;
    try {
      for await (const message of q) {
        onMessage(message);
      }
    } catch (error) {
      failure = error;
    }
    const reason = failure ? messageOf(failure) : STOPPED_MESSAGE;
    finishTurn('error', reason);
    // Before the session is open, open() reports the failure in its reply instead.
    if (!closed && ready) {
      emit({ event: 'session_closed', reason });
    }
  }

  async function openSession() {
    let init;
    try {
      q = sdk.query({ prompt: queue, options: buildOptions() });
      pump().catch((error) => {
        process.stderr.write(`tandem-claude-helper: the session failed: ${messageOf(error)}\n`);
      });
      init = await q.initializationResult();
    } catch (error) {
      close();
      const message = messageOf(error);
      throw codedError(MISSING_CONVERSATION.test(message) ? 'resume_failed' : 'spawn_failed', message);
    }
    const auth = checkAccount(init?.account);
    if (!auth.ok) {
      refused = true;
      stop();
    }
    ready = true;
    return {
      auth,
      account: {
        subscription: init?.account?.subscriptionType ?? null,
        provider: init?.account?.apiProvider ?? null,
      },
      claude_code_version: init?.claude_code_version ?? null,
      models: (init?.models ?? []).map((model) => ({
        value: model.value,
        name: model.displayName,
        description: model.description ?? '',
        efforts: model.supportedEffortLevels ?? [],
      })),
    };
  }

  function assertCanStart(turn) {
    if (refused) {
      throw codedError('auth_refused', 'Claude Code is not using your Claude subscription.');
    }
    if (closed || !ready) {
      throw codedError('no_session', 'The Claude session is not open.');
    }
    if (current) {
      throw codedError('busy', 'Claude is already working on a turn.');
    }
    if (!POLICIES.has(turn.policy)) {
      throw codedError('bad_request', `Unknown policy: ${String(turn.policy)}`);
    }
    if (typeof turn.text !== 'string' || turn.text === '') {
      throw codedError('bad_request', 'A turn needs a non-empty text.');
    }
  }

  async function apply(what, key, value, change) {
    if (applied[key] === value) {
      return;
    }
    try {
      await change();
      applied[key] = value;
    } catch (error) {
      emit({ event: 'notice', text: `Could not apply ${what}: ${messageOf(error)}` });
    }
  }

  async function runTurn({ id, text, policy, model, effort }) {
    const turn = { id, policy, interrupting: false };
    current = turn;
    const wantedModel = model ?? 'default';
    const wantedEffort = effort ?? null;
    const wantedMode = policy === 'takeover' ? 'acceptEdits' : 'default';
    await apply('the model', 'model', wantedModel, () =>
      q.setModel(wantedModel === 'default' ? undefined : wantedModel),
    );
    await apply('the effort level', 'effort', wantedEffort, () =>
      q.applyFlagSettings({ effortLevel: wantedEffort }),
    );
    await apply('the permission mode', 'mode', wantedMode, () => q.setPermissionMode(wantedMode));
    if (current !== turn) {
      // The session ended while the settings were applied; the turn is already finished.
      return;
    }
    emit({ event: 'turn_started', turn: id });
    queue.push({ type: 'user', message: { role: 'user', content: text }, parent_tool_use_id: null });
  }

  async function interrupt() {
    if (!current || !q) {
      return;
    }
    current.interrupting = true;
    try {
      await q.interrupt();
    } catch {
      // The turn ends on its own; a failed interrupt changes nothing Tandem can act on.
    }
  }

  function close() {
    stop();
  }

  return {
    open: openSession,
    assertCanStart,
    runTurn,
    interrupt,
    close,
    get busy() {
      return current !== null;
    },
  };
}
