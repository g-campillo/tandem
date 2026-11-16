import { test } from 'node:test';
import assert from 'node:assert/strict';
import { checkAccount, checkInit } from '../src/auth.mjs';

const subscription = {
  email: 'user@example.test',
  subscriptionType: 'Claude Ultra',
  apiProvider: 'firstParty',
};

test('a first-party subscription account is accepted', () => {
  assert.deepEqual(checkAccount(subscription), { ok: true });
  assert.deepEqual(checkAccount({ ...subscription, apiKeySource: 'none' }), { ok: true });
});

test('a subscription without an email is accepted', () => {
  const { email: _email, ...account } = subscription;
  assert.deepEqual(checkAccount(account), { ok: true });
});

for (const [name, account] of [
  ['undefined', undefined],
  ['null', null],
  ['{}', {}],
  ['an empty subscription type and no email', { subscriptionType: '', apiProvider: 'firstParty' }],
]) {
  test(`${name} is signed out`, () => {
    assert.deepEqual(checkAccount(account), {
      ok: false,
      reason: 'signed_out',
      detail: 'Claude Code is not signed in.',
    });
  });
}

test('a non-first-party provider is refused', () => {
  assert.deepEqual(checkAccount({ ...subscription, apiProvider: 'bedrock' }), {
    ok: false,
    reason: 'provider',
    detail: 'Claude Code is set up for bedrock, not a Claude subscription.',
  });
});

test('a missing provider is refused', () => {
  const { apiProvider: _provider, ...account } = subscription;
  assert.deepEqual(checkAccount(account), {
    ok: false,
    reason: 'provider',
    detail: 'Claude Code is set up for another provider, not a Claude subscription.',
  });
});

test('an API key source is refused', () => {
  assert.deepEqual(checkAccount({ ...subscription, apiKeySource: 'ANTHROPIC_API_KEY' }), {
    ok: false,
    reason: 'not_subscription',
    detail: 'Claude Code would bill an API key (ANTHROPIC_API_KEY) instead of your subscription.',
  });
});

test('the provider is checked before the API key source', () => {
  const result = checkAccount({
    ...subscription,
    apiProvider: 'vertex',
    apiKeySource: 'ANTHROPIC_API_KEY',
  });
  assert.equal(result.reason, 'provider');
});

test('a first-party account with an email and no subscription type is refused', () => {
  assert.deepEqual(checkAccount({ email: 'user@example.test', apiProvider: 'firstParty' }), {
    ok: false,
    reason: 'not_subscription',
    detail: 'This Claude account has no Claude subscription.',
  });
  assert.deepEqual(
    checkAccount({ email: 'user@example.test', subscriptionType: '', apiProvider: 'firstParty' }),
    {
      ok: false,
      reason: 'not_subscription',
      detail: 'This Claude account has no Claude subscription.',
    },
  );
});

test('checkInit accepts only apiKeySource none', () => {
  assert.deepEqual(checkInit({ type: 'system', subtype: 'init', apiKeySource: 'none' }), {
    ok: true,
  });
  for (const source of ['ANTHROPIC_API_KEY', 'apiKeyHelper', 'user', 'project', 'org', 'temporary']) {
    assert.deepEqual(checkInit({ type: 'system', subtype: 'init', apiKeySource: source }), {
      ok: false,
      reason: 'not_subscription',
      detail: `Claude Code started with an API key (${source}).`,
    });
  }
  assert.equal(checkInit({ type: 'system', subtype: 'init' }).ok, false);
});
