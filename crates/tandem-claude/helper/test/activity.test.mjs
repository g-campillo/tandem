import { test } from 'node:test';
import assert from 'node:assert/strict';
import { activityFor } from '../src/activity.mjs';

const cwd = '/work/project';

test('Read shows the path', () => {
  assert.deepEqual(activityFor('Read', { file_path: '/work/project/src/cli.rs' }, cwd), {
    kind: 'read',
    label: 'src/cli.rs',
  });
});

test('paths outside the project are shown as given', () => {
  assert.deepEqual(activityFor('Read', { file_path: '/etc/hosts' }, cwd), {
    kind: 'read',
    label: '/etc/hosts',
  });
  assert.deepEqual(activityFor('Read', { file_path: '/work/project-other/a.rs' }, cwd), {
    kind: 'read',
    label: '/work/project-other/a.rs',
  });
  assert.deepEqual(activityFor('Read', { file_path: 'src/relative.rs' }, cwd), {
    kind: 'read',
    label: 'src/relative.rs',
  });
});

test('the project folder itself is shown as given', () => {
  assert.deepEqual(activityFor('Read', { file_path: '/work/project' }, cwd), {
    kind: 'read',
    label: '/work/project',
  });
});

test('Grep shows the pattern in double quotes', () => {
  assert.deepEqual(activityFor('Grep', { pattern: 'clap bool flag', path: 'src' }, cwd), {
    kind: 'search',
    label: '"clap bool flag"',
  });
});

test('Glob shows the pattern', () => {
  assert.deepEqual(activityFor('Glob', { pattern: 'src/**/*.rs' }, cwd), {
    kind: 'search',
    label: 'src/**/*.rs',
  });
});

test('WebSearch shows the query', () => {
  assert.deepEqual(activityFor('WebSearch', { query: 'clap derive flags' }, cwd), {
    kind: 'web_search',
    label: 'clap derive flags',
  });
});

test('WebFetch shows the URL', () => {
  assert.deepEqual(
    activityFor('WebFetch', { url: 'https://docs.rs/clap', prompt: 'summarise' }, cwd),
    { kind: 'web_fetch', label: 'https://docs.rs/clap' },
  );
});

test('Bash shows the command', () => {
  assert.deepEqual(activityFor('Bash', { command: 'cargo test', description: 'Run tests' }, cwd), {
    kind: 'command',
    label: 'cargo test',
  });
});

test('edit tools show the path', () => {
  assert.deepEqual(activityFor('Edit', { file_path: '/work/project/src/main.rs' }, cwd), {
    kind: 'edit',
    label: 'src/main.rs',
  });
  assert.deepEqual(activityFor('Write', { file_path: '/work/project/tests/cli.rs' }, cwd), {
    kind: 'edit',
    label: 'tests/cli.rs',
  });
  assert.deepEqual(
    activityFor('NotebookEdit', { notebook_path: '/work/project/notes/a.ipynb' }, cwd),
    { kind: 'edit', label: 'notes/a.ipynb' },
  );
});

test('Tandem tools show nothing', () => {
  assert.equal(activityFor('mcp__tandem__propose_plan', { goal: 'g' }, cwd), null);
  assert.equal(activityFor('mcp__tandem__show_step', { step: 1 }, cwd), null);
});

test('other MCP tools show the server and tool', () => {
  assert.deepEqual(activityFor('mcp__brain__brain_search', { query: 'x' }, cwd), {
    kind: 'tool',
    label: 'brain: brain_search',
  });
});

test('any other tool shows its name', () => {
  assert.deepEqual(activityFor('TodoWrite', { todos: [] }, cwd), {
    kind: 'tool',
    label: 'TodoWrite',
  });
  assert.deepEqual(activityFor('Agent', {}, cwd), { kind: 'tool', label: 'Agent' });
});

test('missing input fields give an empty label', () => {
  assert.deepEqual(activityFor('Read', {}, cwd), { kind: 'read', label: '' });
  assert.deepEqual(activityFor('Bash', undefined, cwd), { kind: 'command', label: '' });
});

test('labels are cut to 200 characters', () => {
  const command = `echo ${'x'.repeat(300)}`;
  const { label } = activityFor('Bash', { command }, cwd);
  assert.equal(label.length, 200);
  assert.equal(label, command.slice(0, 200));
  const exact = 'y'.repeat(200);
  assert.equal(activityFor('Bash', { command: exact }, cwd).label, exact);
});

test('truncation never splits a surrogate pair', () => {
  const command = `${'x'.repeat(199)}🙂tail`;
  const { label } = activityFor('Bash', { command }, cwd);
  assert.ok([...label].length <= 200);
  assert.ok(label.length <= 200);
  assert.equal(label.isWellFormed(), true);
});
