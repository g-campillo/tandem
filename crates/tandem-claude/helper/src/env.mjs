// Removes everything that would make Claude Code bill an API key or another provider
// instead of the user's Claude subscription.

const REMOVED_KEYS = new Set(['ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN']);
const REMOVED_PREFIX = 'CLAUDE_CODE_USE_';

export function scrubEnv(env) {
  const scrubbed = {};
  for (const [key, value] of Object.entries(env)) {
    if (REMOVED_KEYS.has(key) || key.startsWith(REMOVED_PREFIX)) {
      continue;
    }
    scrubbed[key] = value;
  }
  return scrubbed;
}
