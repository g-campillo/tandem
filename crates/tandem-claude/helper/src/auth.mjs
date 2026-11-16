// Decides whether a Claude Code session runs on the user's Claude subscription. Only the
// account summary the SDK reports is inspected; credentials are never read.

function nonEmptyString(value) {
  return typeof value === 'string' && value.length > 0;
}

/** Checks the `account` object from the SDK's initialization result. */
export function checkAccount(account) {
  if (!account || (typeof account.email !== 'string' && !nonEmptyString(account.subscriptionType))) {
    return { ok: false, reason: 'signed_out', detail: 'Claude Code is not signed in.' };
  }
  if (account.apiProvider !== 'firstParty') {
    return {
      ok: false,
      reason: 'provider',
      detail: `Claude Code is set up for ${account.apiProvider ?? 'another provider'}, not a Claude subscription.`,
    };
  }
  if (account.apiKeySource !== undefined && account.apiKeySource !== 'none') {
    return {
      ok: false,
      reason: 'not_subscription',
      detail: `Claude Code would bill an API key (${account.apiKeySource}) instead of your subscription.`,
    };
  }
  if (!nonEmptyString(account.subscriptionType)) {
    return {
      ok: false,
      reason: 'not_subscription',
      detail: 'This Claude account has no Claude subscription.',
    };
  }
  return { ok: true };
}

/** Checks the SDK's `system`/`init` message, which names the credential a turn uses. */
export function checkInit(message) {
  if (message.apiKeySource === 'none') {
    return { ok: true };
  }
  return {
    ok: false,
    reason: 'not_subscription',
    detail: `Claude Code started with an API key (${message.apiKeySource}).`,
  };
}
