// A stand-in for @anthropic-ai/claude-agent-sdk that plays scripted scenarios chosen by the
// text of each user message. It never starts Claude Code.

/**
 * Every `query` call, in order: `{ options, calls, prompts, query }`, where `prompts` holds the
 * user messages read so far and `query` is the returned object.
 */
export const queries = [];

export function tool(name, description, shape, handler) {
  return { name, description, inputSchema: shape, handler };
}

export function createSdkMcpServer({ name, tools }) {
  return { type: 'sdk', name, tools };
}

const MODELS = [
  {
    value: 'default',
    displayName: 'Default (recommended)',
    description: 'd',
    supportedEffortLevels: ['low', 'high'],
  },
  { value: 'haiku', displayName: 'Haiku', description: 'h' },
];

const SUBSCRIBER = {
  email: 'user@example.test',
  subscriptionType: 'Claude Ultra',
  apiProvider: 'firstParty',
};

function accountFor(env) {
  switch (env?.MOCK_ACCOUNT) {
    case 'api_key':
      return { ...SUBSCRIBER, apiKeySource: 'ANTHROPIC_API_KEY' };
    case 'signed_out':
      return {};
    default:
      return { ...SUBSCRIBER };
  }
}

function assistantText(text) {
  return {
    type: 'assistant',
    parent_tool_use_id: null,
    message: { role: 'assistant', content: [{ type: 'text', text }] },
  };
}

const SUCCESS = { type: 'result', subtype: 'success', is_error: false };

export function query({ prompt, options }) {
  const record = { options, calls: [], prompts: [] };
  queries.push(record);
  if (options.env?.MOCK_LOG) {
    // Stray library logging, which the helper must keep off stdout.
    console.log(`${options.env.MOCK_LOG} log`);
    console.info(`${options.env.MOCK_LOG} info`);
    console.warn(`${options.env.MOCK_LOG} warn`);
  }

  let closed = false;
  let resolveClosed;
  const closedSignal = new Promise((resolve) => {
    resolveClosed = resolve;
  });
  // Interrupts are counted so one that arrives before `hold` starts waiting still ends it.
  let interrupts = 0;
  let wakeInterrupt = null;

  function interrupted() {
    if (interrupts > 0) {
      interrupts -= 1;
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      wakeInterrupt = () => {
        interrupts -= 1;
        resolve();
      };
    });
  }

  function handlerFor(name) {
    const server = options.mcpServers?.tandem;
    const found = server?.tools?.find((candidate) => candidate.name === name);
    if (!found) {
      throw new Error(`mock sdk: no ${name} tool`);
    }
    return found.handler;
  }

  async function* play() {
    for await (const message of prompt) {
      record.prompts.push(message);
      yield {
        type: 'system',
        subtype: 'init',
        apiKeySource: options.env?.MOCK_INIT_KEY ?? 'none',
        session_id: 's',
      };
      switch (message.message.content) {
        case 'plan':
          yield {
            type: 'stream_event',
            parent_tool_use_id: null,
            event: { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'Here' } },
          };
          yield {
            type: 'assistant',
            parent_tool_use_id: null,
            message: {
              role: 'assistant',
              content: [
                {
                  type: 'tool_use',
                  id: 'toolu_1',
                  name: 'Read',
                  input: { file_path: `${options.cwd}/src/cli.rs` },
                },
              ],
            },
          };
          await handlerFor('propose_plan')(
            {
              goal: 'g',
              done_when: ['d'],
              steps: [{ kind: 'edit_file', target: 'src/cli.rs', summary: 's', language: 'rust' }],
            },
            {},
          );
          yield assistantText('Here is the plan.');
          yield SUCCESS;
          break;
        case 'ask': {
          const result = await options.canUseTool(
            'Bash',
            { command: 'npm install left-pad' },
            {
              signal: new AbortController().signal,
              title: 'Claude wants to run npm install left-pad',
            },
          );
          yield assistantText(result.behavior === 'allow' ? 'allowed' : 'denied');
          yield SUCCESS;
          break;
        }
        case 'hold':
          await interrupted();
          yield {
            type: 'result',
            subtype: 'error_during_execution',
            is_error: true,
            errors: ['interrupted'],
          };
          break;
        case 'limit':
          yield {
            type: 'assistant',
            parent_tool_use_id: null,
            error: 'rate_limit',
            message: { role: 'assistant', content: [] },
          };
          yield { type: 'result', subtype: 'success', is_error: true };
          break;
        default:
          yield assistantText('ok');
          yield SUCCESS;
          break;
      }
    }
  }

  const messages = play();
  const done = { done: true, value: undefined };

  function control(name) {
    return (value) => {
      record.calls.push([name, value]);
      return Promise.resolve();
    };
  }

  const handle = {
    [Symbol.asyncIterator]() {
      return this;
    },
    next() {
      if (closed) {
        return Promise.resolve(done);
      }
      return Promise.race([messages.next(), closedSignal]);
    },
    return() {
      return Promise.resolve(done);
    },
    initializationResult() {
      if (options.resume === 'missing-session') {
        return Promise.reject(
          new Error(
            `Claude Code returned an error result: No conversation found with session ID: ${options.resume}`,
          ),
        );
      }
      return Promise.resolve({
        account: accountFor(options.env),
        models: MODELS.map((model) => ({ ...model })),
        claude_code_version: '2.1.325',
      });
    },
    interrupt() {
      record.calls.push(['interrupt', undefined]);
      interrupts += 1;
      const wake = wakeInterrupt;
      wakeInterrupt = null;
      wake?.();
      return Promise.resolve();
    },
    setModel: control('setModel'),
    setPermissionMode: control('setPermissionMode'),
    applyFlagSettings: control('applyFlagSettings'),
    close() {
      if (!closed) {
        closed = true;
        resolveClosed(done);
      }
    },
  };
  record.query = handle;
  return handle;
}
