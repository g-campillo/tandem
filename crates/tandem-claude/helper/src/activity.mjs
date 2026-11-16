// Short labels for what Claude is doing, shown under its reply ("read src/cli.rs").

import path from 'node:path';

const MAX_LABEL = 200;
const TANDEM_TOOL_PREFIX = 'mcp__tandem__';
const MCP_TOOL = /^mcp__(.+?)__(.+)$/;

function cut(text) {
  if (text.length <= MAX_LABEL) {
    return text;
  }
  let label = text.slice(0, MAX_LABEL);
  const last = label.charCodeAt(label.length - 1);
  if (last >= 0xd800 && last <= 0xdbff) {
    label = label.slice(0, -1);
  }
  return label;
}

function shownPath(file, cwd) {
  if (typeof file !== 'string') {
    return '';
  }
  if (cwd && path.isAbsolute(file)) {
    const relative = path.relative(cwd, file);
    const outside =
      relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative);
    if (relative !== '' && !outside) {
      return relative;
    }
  }
  return file;
}

function text(value) {
  return typeof value === 'string' ? value : '';
}

/** Returns `{ kind, label }` for a tool call, or `null` when nothing should be shown. */
export function activityFor(tool, input, cwd) {
  const args = input ?? {};
  switch (tool) {
    case 'Read':
      return { kind: 'read', label: cut(shownPath(args.file_path, cwd)) };
    case 'Grep':
      return { kind: 'search', label: cut(`"${text(args.pattern)}"`) };
    case 'Glob':
      return { kind: 'search', label: cut(text(args.pattern)) };
    case 'WebSearch':
      return { kind: 'web_search', label: cut(text(args.query)) };
    case 'WebFetch':
      return { kind: 'web_fetch', label: cut(text(args.url)) };
    case 'Bash':
      return { kind: 'command', label: cut(text(args.command)) };
    case 'Edit':
    case 'Write':
      return { kind: 'edit', label: cut(shownPath(args.file_path, cwd)) };
    case 'NotebookEdit':
      return { kind: 'edit', label: cut(shownPath(args.notebook_path, cwd)) };
    default:
      break;
  }
  if (tool.startsWith(TANDEM_TOOL_PREFIX)) {
    return null;
  }
  const mcp = MCP_TOOL.exec(tool);
  if (mcp) {
    return { kind: 'tool', label: cut(`${mcp[1]}: ${mcp[2]}`) };
  }
  return { kind: 'tool', label: cut(tool) };
}
