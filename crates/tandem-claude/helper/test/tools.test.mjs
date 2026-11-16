import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import * as mockSdk from './support/mock-sdk.mjs';
import { TANDEM_TOOL_NAMES, createTandemServer } from '../src/tools.mjs';

const NAMES = ['propose_plan', 'show_step', 'report_check', 'step_done', 'read_editor_buffer'];
/** `show_step` inputs that Tandem's Rust tests read too. */
const SHOW_STEP_SAMPLES = JSON.parse(
  fs.readFileSync(new URL('./fixtures/show_step.json', import.meta.url), 'utf8'),
);

function serverWith(answer) {
  const asked = [];
  const server = createTandemServer({
    sdk: mockSdk,
    askHost: async (request) => {
      asked.push(request);
      return answer;
    },
  });
  return { server, asked };
}

test('the server is named tandem and has the five tools', () => {
  const { server } = serverWith({ ok: true, text: '' });
  assert.equal(server.type, 'sdk');
  assert.equal(server.name, 'tandem');
  assert.deepEqual(
    server.tools.map((tool) => tool.name),
    NAMES,
  );
  for (const tool of server.tools) {
    assert.ok(tool.description.length > 0);
    assert.equal(typeof tool.inputSchema, 'object');
  }
});

test('TANDEM_TOOL_NAMES has the five prefixed names', () => {
  assert.deepEqual(
    TANDEM_TOOL_NAMES,
    NAMES.map((name) => `mcp__tandem__${name}`),
  );
});

for (const name of NAMES) {
  test(`${name} forwards its input to the host and returns the host text`, async () => {
    const { server, asked } = serverWith({ ok: true, text: `shown ${name}` });
    const input = { marker: name };
    const tool = server.tools.find((candidate) => candidate.name === name);
    const result = await tool.handler(input, {});
    assert.deepEqual(asked, [{ request: 'tool', name, input }]);
    assert.deepEqual(result, { content: [{ type: 'text', text: `shown ${name}` }], isError: false });
  });
}

test('an answer that is not ok marks the result as an error', async () => {
  const { server } = serverWith({ ok: false, text: 'Step 9 does not exist.' });
  const tool = server.tools.find((candidate) => candidate.name === 'show_step');
  const result = await tool.handler({ step: 9 }, {});
  assert.deepEqual(result, {
    content: [{ type: 'text', text: 'Step 9 does not exist.' }],
    isError: true,
  });
});

test('an answer without text gives an empty text block', async () => {
  const { server } = serverWith({ ok: true });
  const tool = server.tools.find((candidate) => candidate.name === 'step_done');
  const result = await tool.handler({ step: 1, summary: 's' }, {});
  assert.deepEqual(result, { content: [{ type: 'text', text: '' }], isError: false });
});

test('the input shapes accept a valid plan and reject an empty one', () => {
  const { server } = serverWith({ ok: true });
  const shape = server.tools.find((candidate) => candidate.name === 'propose_plan').inputSchema;
  const valid = {
    goal: 'g',
    done_when: ['d'],
    steps: [{ kind: 'edit_file', target: 'src/cli.rs', summary: 's', language: 'rust' }],
  };
  for (const [key, schema] of Object.entries(shape)) {
    assert.equal(schema.safeParse(valid[key]).success, true, key);
  }
  assert.equal(shape.steps.safeParse([]).success, false);
  assert.equal(shape.steps.safeParse([{ ...valid.steps[0], kind: 'delete_file' }]).success, false);
});

function showStepShape() {
  const { server } = serverWith({ ok: true });
  return server.tools.find((candidate) => candidate.name === 'show_step').inputSchema;
}

test('the show_step shape accepts every fixture input', () => {
  const shape = showStepShape();
  const samples = Object.entries(SHOW_STEP_SAMPLES);
  assert.equal(samples.length, 5);
  for (const [name, sample] of samples) {
    for (const key of Object.keys(sample)) {
      assert.ok(Object.hasOwn(shape, key), `${name}: ${key} is not in the shape`);
    }
    for (const [key, schema] of Object.entries(shape)) {
      assert.equal(schema.safeParse(sample[key]).success, true, `${name}: ${key}`);
    }
  }
});

test('the show_step shape rejects a bad location', () => {
  const shape = showStepShape();
  const valid = { action: 'insert', line: 6, anchor: 'pub name: String,', placement: 'after' };
  assert.equal(shape.location.safeParse(valid).success, true);
  const withoutAnchor = { ...valid };
  delete withoutAnchor.anchor;
  for (const bad of [
    { ...valid, action: 'move' },
    { ...valid, line: 0 },
    withoutAnchor,
    { ...valid, placement: 'inside' },
  ]) {
    assert.equal(shape.location.safeParse(bad).success, false, JSON.stringify(bad));
  }
  const keys = Object.keys(shape);
  assert.ok(!keys.includes('anchor'));
  assert.ok(keys.includes('where'));
  assert.ok(keys.includes('location'));
});
