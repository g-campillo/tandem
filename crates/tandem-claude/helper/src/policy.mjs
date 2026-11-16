// Tool-permission policy for the four turn kinds: discuss, guided, check and takeover.
// Claude may change files only during a takeover turn, or in any turn when the permission
// mode is bypass, and never outside the project folder.

import fs from 'node:fs';
import path from 'node:path';

export const EDIT_TOOLS = new Set(['Edit', 'Write', 'NotebookEdit']);

export const PERMISSION_MODES = new Set(['ask', 'auto', 'bypass']);

/** True when Claude's edit tools may run: during a takeover, and in every turn in bypass. */
export function mayEdit(policy, mode) {
  return policy === 'takeover' || mode === 'bypass';
}

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

// Line breaks, other control characters, and blanks other than space and tab, which shells
// may or may not split words on.
const UNMODELLED_CHARACTERS = /[\0-\x08\n-\x1f\x7f]|[^\S \t]/u;
// Unquoted characters that start syntax the classifier does not model: expansions, brace
// expansion, subshells, command separators, redirections other than `2>&1`, and comments.
const UNMODELLED_SYNTAX = new Set(['$', '`', '{', '}', '(', ')', ';', '<', '>', '#']);
const GLOB = /[*?[]/;
// Characters a backslash escapes inside double quotes; before any other it stays literal.
const DOUBLE_QUOTE_ESCAPES = new Set(['$', '`', '"', '\\']);
const MERGE_STDERR_TAIL = '>&1';
const WORD_ENDS = new Set([undefined, ' ', '\t', '|', '&']);

/** True at the `>` of a `2>&1` that stands as a word of its own. */
function mergesStderr(command, index, word) {
  return (
    word !== null &&
    word.start === index - 1 &&
    command[word.start] === '2' &&
    command.startsWith(MERGE_STDERR_TAIL, index) &&
    WORD_ENDS.has(command[index + MERGE_STDERR_TAIL.length])
  );
}

/**
 * Splits `command` the way the shell would: into `&&` parts, each a list of `|` stages, each
 * a list of words with quotes and backslashes removed. Returns null for syntax it does not
 * model, and for a glob whose matches could start with `-` and so turn into an option.
 */
function parseCommand(command) {
  if (UNMODELLED_CHARACTERS.test(command)) {
    return null;
  }
  const parts = [];
  let stages = [];
  let words = [];
  let word = null;

  const append = (text, glob = false) => {
    if (word.text === '' && text !== '') {
      word.leadingGlob = glob;
    }
    word.text += text;
    word.glob ||= glob;
  };
  const endWord = () => {
    if (word !== null) {
      if (word.glob && (word.leadingGlob || word.text.startsWith('-'))) {
        return false;
      }
      words.push(word.text);
      word = null;
    }
    return true;
  };
  const endStage = () => {
    if (!endWord() || words.length === 0) {
      return false;
    }
    stages.push(words);
    words = [];
    return true;
  };
  const endPart = () => {
    if (!endStage()) {
      return false;
    }
    parts.push(stages);
    stages = [];
    return true;
  };

  let index = 0;
  while (index < command.length) {
    const character = command[index];
    const next = command[index + 1];
    if (character === ' ' || character === '\t') {
      if (!endWord()) {
        return null;
      }
      index += 1;
    } else if (character === '|') {
      if (next === '|' || next === '&' || !endStage()) {
        return null;
      }
      index += 1;
    } else if (character === '&') {
      if (next !== '&' || !endPart()) {
        return null;
      }
      index += 2;
    } else if (mergesStderr(command, index, word)) {
      word = null;
      index += MERGE_STDERR_TAIL.length;
    } else if (UNMODELLED_SYNTAX.has(character)) {
      return null;
    } else {
      word ??= { start: index, text: '', glob: false, leadingGlob: false };
      if (character === "'") {
        const end = command.indexOf("'", index + 1);
        if (end === -1) {
          return null;
        }
        append(command.slice(index + 1, end));
        index = end + 1;
      } else if (character === '"') {
        index += 1;
        while (command[index] !== '"') {
          const quoted = command[index];
          if (quoted === undefined || quoted === '$' || quoted === '`') {
            return null;
          }
          if (quoted === '\\' && DOUBLE_QUOTE_ESCAPES.has(command[index + 1])) {
            append(command[index + 1]);
            index += 2;
          } else {
            append(quoted);
            index += 1;
          }
        }
        index += 1;
      } else if (character === '\\') {
        if (next === undefined) {
          return null;
        }
        append(next);
        index += 2;
      } else {
        append(character, GLOB.test(character));
        index += 1;
      }
    }
  }
  return endPart() ? parts : null;
}

const PIPE_FILTERS = new Set(['head', 'tail', 'grep', 'rg', 'wc', 'sort', 'uniq']);
const ENV_ASSIGNMENT = /^[A-Z_][A-Z0-9_]*=[\w./:-]*$/;
const READ_COMMANDS = new Set([
  'ls', 'pwd', 'cat', 'head', 'tail', 'wc', 'grep', 'rg', 'tree', 'file', 'stat', 'which', 'echo',
]);
const FIND_ACTIONS = new Set([
  '-delete', '-exec', '-execdir', '-ok', '-okdir', '-fprint', '-fprint0', '-fprintf', '-fls',
]);
const GIT_READS = new Set(['status', 'diff', 'log', 'show', 'branch', 'ls-files', 'blame', 'rev-parse']);
// With any other argument, such as a name, `git branch` creates, deletes, moves or copies one.
const GIT_BRANCH_LISTING = new Set([
  '-a', '--all', '-r', '--remotes', '-v', '-vv', '--verbose', '-l', '--list', '--show-current',
]);
const SCRIPT_RUNNERS = new Set(['npm', 'pnpm', 'yarn', 'bun']);
const SCRIPT_NAMES = new Set(['test', 'lint', 'build', 'typecheck', 'check', 'tsc']);
// Long options of read commands that write a file or run another program: git diff/log/show
// and sort `--output`, rg `--pre` and `--hostname-bin`, sort `--compress-program`.
const WRITING_OPTIONS = ['--output', '--pre', '--hostname-bin', '--compress-program'];

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
      // `-o` names an output file and `-R` writes 00Tree.html into every folder.
      return options.some((token) => /^-[^-]*[oR]/.test(token));
    case 'file':
      return options.some((token) => /^-[^-]*C/.test(token)) || hasLongOption(options, ['--compile']);
    case 'uniq': {
      // A second operand is the output file, and a glob can expand into several operands.
      const operands = args.filter((token) => token === '-' || !token.startsWith('-'));
      return operands.length > 1 || operands.some((token) => GLOB.test(token));
    }
    default:
      return false;
  }
}

/** A `cd` target that stays inside the project: relative, no `..`, `~`, `-`, `$` or glob. */
function staysInside(target) {
  return (
    !path.isAbsolute(target) &&
    !target.startsWith('~') &&
    !target.startsWith('-') &&
    !target.includes('$') &&
    !GLOB.test(target) &&
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
    const [subcommand, ...rest] = args;
    if (subcommand === 'branch') {
      return rest.every((token) => GIT_BRANCH_LISTING.has(token));
    }
    return GIT_READS.has(subcommand);
  }
  if (name === 'cd') {
    return args.length === 1 && staysInside(args[0]);
  }
  return false;
}

function rewritesFiles(tokens) {
  return tokens.some(
    (token) =>
      token === '--fix' ||
      token.startsWith('--fix=') ||
      token === '--fix-only' ||
      token === '--write' ||
      // Go's flag package also accepts `--w` and `-w=true`.
      /^--?w(=|$)/.test(token),
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

function classifyStage(tokens) {
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

/**
 * Classifies a shell command as `'read'`, `'build'` or `'other'`, judging the words the shell
 * will see after quote removal. Anything it cannot read that way is `'other'`.
 */
export function classifyBash(command) {
  const parts = parseCommand(command);
  if (parts === null) {
    return 'other';
  }
  const results = [];
  for (const [first, ...filters] of parts) {
    if (filters.some((stage) => !PIPE_FILTERS.has(stage[0]) || readerWritesOrRuns(stage))) {
      return 'other';
    }
    results.push(classifyStage(first));
  }
  if (results.includes('other')) {
    return 'other';
  }
  return results.includes('build') ? 'build' : 'read';
}

/**
 * Decides a tool call for the given policy and permission mode (`ask`, `auto` or `bypass`;
 * missing or unknown counts as `ask`): `allow`, `deny` (with a reason Claude sees), `ask` (the
 * user decides in Tandem) or `pass` (Claude Code's own permission mode and rules decide).
 */
export function decide({ policy, mode = 'ask', tool, input, cwd }) {
  const args = input ?? {};
  if (tool.startsWith(TANDEM_TOOL_PREFIX)) {
    return { decision: 'allow' };
  }
  if (EDIT_TOOLS.has(tool)) {
    if (!mayEdit(policy, mode)) {
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
    // Only ask adds prompts of Tandem's own; any value but auto and bypass counts as ask.
    const relaxed = mode === 'auto' || mode === 'bypass';
    if (kind === 'build' && (relaxed || policy === 'check' || policy === 'takeover')) {
      return { decision: 'allow' };
    }
    if (relaxed || policy === 'takeover') {
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
