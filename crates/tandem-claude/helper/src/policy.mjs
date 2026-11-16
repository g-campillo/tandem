// Tool-permission policy for the four turn kinds: discuss, guided, check and takeover.
// Claude may change files only during a takeover turn and only inside the project folder.

import fs from 'node:fs';
import path from 'node:path';

export const EDIT_TOOLS = new Set(['Edit', 'Write', 'NotebookEdit']);

export const GUIDED_EDIT_MESSAGE =
  'Tandem is in guided mode: the user writes the code. Explain the change instead of editing files.';
export const OUTSIDE_PROJECT_MESSAGE =
  'Tandem only lets Claude change files inside the project folder.';

const TANDEM_TOOL_PREFIX = 'mcp__tandem__';

/** Real path of `target`, following symlinks in the part that exists. */
function realpathOfPossiblyMissing(target) {
  const missing = [];
  let existing = target;
  for (;;) {
    let present = true;
    try {
      fs.lstatSync(existing);
    } catch (error) {
      if (error.code !== 'ENOENT' && error.code !== 'ENOTDIR') {
        throw error;
      }
      present = false;
    }
    if (present) {
      // A dangling symlink exists but has no real path; realpathSync throws for it, which
      // the caller treats as outside the project because writing through it could land anywhere.
      return path.join(fs.realpathSync(existing), ...missing);
    }
    const parent = path.dirname(existing);
    if (parent === existing) {
      return path.join(existing, ...missing);
    }
    missing.unshift(path.basename(existing));
    existing = parent;
  }
}

/** True when `candidate`, resolved against `cwd` with symlinks followed, lies inside `cwd`. */
export function insideProject(cwd, candidate) {
  try {
    const resolved = realpathOfPossiblyMissing(path.resolve(cwd, candidate));
    const root = fs.realpathSync(cwd);
    return resolved === root || resolved.startsWith(root + path.sep);
  } catch {
    return false;
  }
}

const PIPE_FILTERS = ['head', 'tail', 'grep', 'rg', 'wc', 'sort', 'uniq'];
const ENV_ASSIGNMENT = /^[A-Z_][A-Z0-9_]*=[\w./:-]*$/;
const READ_COMMANDS = new Set([
  'ls', 'pwd', 'cat', 'head', 'tail', 'wc', 'grep', 'rg', 'tree', 'file', 'stat', 'which', 'echo',
]);
const FIND_ACTIONS = new Set([
  '-delete', '-exec', '-execdir', '-ok', '-okdir', '-fprint', '-fprint0', '-fprintf', '-fls',
]);
const GIT_READS = new Set(['status', 'diff', 'log', 'show', 'branch', 'ls-files', 'blame', 'rev-parse']);
const SCRIPT_RUNNERS = new Set(['npm', 'pnpm', 'yarn', 'bun']);
const SCRIPT_NAMES = new Set(['test', 'lint', 'build', 'typecheck', 'check', 'tsc']);
// Long options of read commands that write a file or run another program: git diff/log/show
// and sort `--output`, rg `--pre` and `--hostname-bin`, sort `--compress-program`.
const WRITING_OPTIONS = ['--output', '--pre', '--hostname-bin', '--compress-program'];

function startsWithWord(stage, words) {
  return words.some((word) => stage === word || stage.startsWith(`${word} `));
}

/** Options before a `--` end-of-options marker. */
function optionsOf(args) {
  const end = args.indexOf('--');
  return (end === -1 ? args : args.slice(0, end)).filter((token) => token.startsWith('-'));
}

/** True when `options` holds one of the long options `names`, also abbreviated (git, GNU sort). */
function hasLongOption(options, names) {
  return options.some((token) => {
    if (!token.startsWith('--')) {
      return false;
    }
    const name = token.split('=', 1)[0];
    return name.length > 2 && names.some((candidate) => candidate.startsWith(name));
  });
}

/**
 * True when a read-only command would still write a file or run another program through
 * one of its options or operands.
 */
function readerWritesOrRuns(tokens) {
  const [name, ...args] = tokens;
  const options = optionsOf(args);
  if (hasLongOption(options, WRITING_OPTIONS)) {
    return true;
  }
  switch (name) {
    case 'sort':
      return options.some((token) => /^-[^-]*o/.test(token));
    case 'tree':
      return options.includes('-o');
    case 'file':
      return options.includes('-C') || hasLongOption(options, ['--compile']);
    case 'uniq':
      // A second operand is the output file.
      return args.filter((token) => token === '-' || !token.startsWith('-')).length > 1;
    default:
      return false;
  }
}

/** A `cd` target that stays inside the project: relative, no `..`, no `~`, `-` or `$`. */
function staysInside(target) {
  return (
    !path.isAbsolute(target) &&
    !target.startsWith('~') &&
    !target.startsWith('-') &&
    !target.includes('$') &&
    !target.split('/').includes('..')
  );
}

function isRead(tokens) {
  const [name, ...args] = tokens;
  if (READ_COMMANDS.has(name)) {
    return true;
  }
  if (name === 'find') {
    return !args.some((token) => FIND_ACTIONS.has(token));
  }
  if (name === 'git') {
    return GIT_READS.has(args[0]);
  }
  if (name === 'cd') {
    return args.length === 1 && staysInside(args[0]);
  }
  return false;
}

function rewritesFiles(tokens) {
  return tokens.some(
    (token) => token === '--fix' || token.startsWith('--fix=') || token === '-w' || token === '--write',
  );
}

function isBuild(tokens) {
  const [name, first, second] = tokens;
  const rest = tokens.slice(1);
  switch (name) {
    case 'cargo':
      if (first === 'fmt') {
        return tokens.slice(2).includes('--check');
      }
      return ['build', 'check', 'test', 'clippy', 'doc', 'tree', 'metadata', 'nextest'].includes(first);
    case 'go':
      return ['build', 'test', 'vet', 'list', 'env', 'version'].includes(first);
    case 'gofmt':
      return rest.includes('-l');
    case 'tsc':
      return rest.includes('--noEmit');
    case 'npx':
      if (first === 'tsc') {
        return tokens.slice(2).includes('--noEmit');
      }
      if (first === 'prettier') {
        return tokens.slice(2).includes('--check');
      }
      return first === 'eslint' || first === 'jest' || (first === 'vitest' && second === 'run');
    case 'node':
      return ['--test', '--check', '--version'].includes(first);
    case 'pytest':
    case 'mypy':
    case 'javac':
      return true;
    case 'python':
    case 'python3':
      return first === '-m' && ['pytest', 'unittest', 'py_compile'].includes(second);
    case 'ruff':
      return first === 'check';
    case 'mvn':
      return ['test', 'compile', 'verify', 'package'].includes(first);
    case 'gradle':
    case './gradlew':
      return ['test', 'build', 'check', 'compileJava'].includes(first);
    case 'make':
      return ['test', 'check', 'lint', 'build'].includes(first);
    default:
      if (SCRIPT_RUNNERS.has(name)) {
        return first === 'test' || (first === 'run' && SCRIPT_NAMES.has(second));
      }
      return false;
  }
}

function classifyStage(stage) {
  const tokens = stage.split(/\s+/);
  let index = 0;
  while (index < tokens.length && ENV_ASSIGNMENT.test(tokens[index])) {
    index += 1;
  }
  const assignments = index > 0;
  const command = tokens.slice(index);
  if (command.length === 0) {
    return 'other';
  }
  if (isRead(command)) {
    // Variables such as GIT_EXTERNAL_DIFF or RIPGREP_CONFIG_PATH turn a read into a program run.
    return assignments || readerWritesOrRuns(command) ? 'other' : 'read';
  }
  if (rewritesFiles(command)) {
    return 'other';
  }
  return isBuild(command) ? 'build' : 'other';
}

/** Classifies a shell command as `'read'`, `'build'` or `'other'`. */
export function classifyBash(command) {
  if (
    command.trim() === '' ||
    ['\n', '`', '$(', '${', ';', '<'].some((needle) => command.includes(needle))
  ) {
    return 'other';
  }
  const simplified = command.replaceAll('2>&1', '');
  if (simplified.includes('>') || simplified.replaceAll('&&', '').includes('&')) {
    return 'other';
  }
  const results = [];
  for (const part of simplified.split('&&')) {
    const stages = part.split('|').map((stage) => stage.trim());
    if (stages.some((stage) => stage === '')) {
      return 'other';
    }
    const filters = stages.slice(1);
    if (
      !filters.every((stage) => startsWithWord(stage, PIPE_FILTERS)) ||
      filters.some((stage) => readerWritesOrRuns(stage.split(/\s+/)))
    ) {
      return 'other';
    }
    results.push(classifyStage(stages[0]));
  }
  if (results.includes('other')) {
    return 'other';
  }
  return results.includes('build') ? 'build' : 'read';
}

/**
 * Decides a tool call for the given policy: `allow`, `deny` (with a reason Claude sees),
 * `ask` (the user decides in Tandem) or `pass` (Claude Code's own permission rules decide).
 */
export function decide({ policy, tool, input, cwd }) {
  const args = input ?? {};
  if (tool.startsWith(TANDEM_TOOL_PREFIX)) {
    return { decision: 'allow' };
  }
  if (EDIT_TOOLS.has(tool)) {
    if (policy !== 'takeover') {
      return { decision: 'deny', reason: GUIDED_EDIT_MESSAGE };
    }
    const target = args.file_path ?? args.notebook_path;
    if (typeof target !== 'string' || target === '' || !insideProject(cwd, target)) {
      return { decision: 'deny', reason: OUTSIDE_PROJECT_MESSAGE };
    }
    return { decision: 'pass' };
  }
  if (tool === 'Bash') {
    const kind = classifyBash(String(args.command ?? ''));
    if (kind === 'read') {
      return { decision: 'allow' };
    }
    if (kind === 'build' && (policy === 'check' || policy === 'takeover')) {
      return { decision: 'allow' };
    }
    if (policy === 'takeover') {
      return { decision: 'pass' };
    }
    return { decision: 'ask' };
  }
  return { decision: 'pass' };
}

/** Maps a `decide` result to the value a `PreToolUse` hook returns to the SDK. */
export function hookOutput(result) {
  switch (result.decision) {
    case 'allow':
    case 'ask':
      return {
        hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: result.decision },
      };
    case 'deny':
      return {
        hookSpecificOutput: {
          hookEventName: 'PreToolUse',
          permissionDecision: 'deny',
          permissionDecisionReason: result.reason,
        },
      };
    default:
      return {};
  }
}
