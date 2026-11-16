import { test } from 'node:test';
import assert from 'node:assert/strict';
import { scrubEnv } from '../src/env.mjs';

test('scrubEnv removes API credentials and provider switches', () => {
  const env = {
    PATH: '/usr/bin:/bin',
    HOME: '/Users/someone',
    ANTHROPIC_MODEL: 'haiku',
    ANTHROPIC_API_KEY: 'sk-test',
    ANTHROPIC_AUTH_TOKEN: 'token',
    CLAUDE_CODE_USE_BEDROCK: '1',
    CLAUDE_CODE_USE_VERTEX: '1',
    CLAUDE_CODE_USE_FOUNDRY: '1',
  };
  assert.deepEqual(scrubEnv(env), {
    PATH: '/usr/bin:/bin',
    HOME: '/Users/someone',
    ANTHROPIC_MODEL: 'haiku',
  });
});

test('scrubEnv keeps unrelated variables that only look similar', () => {
  const env = {
    CLAUDE_CODE_ENTRYPOINT: 'sdk-ts',
    MY_ANTHROPIC_API_KEY: 'kept',
    ANTHROPIC_API_KEY_HELPER: 'kept',
    claude_code_use_bedrock: 'kept',
  };
  assert.deepEqual(scrubEnv(env), env);
});

test('scrubEnv does not modify its input', () => {
  const env = {
    PATH: '/usr/bin',
    ANTHROPIC_API_KEY: 'sk-test',
    CLAUDE_CODE_USE_BEDROCK: '1',
  };
  const before = { ...env };
  const scrubbed = scrubEnv(env);
  assert.notEqual(scrubbed, env);
  assert.deepEqual(env, before);
});
