import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  EDIT_TOOLS,
  GUIDED_EDIT_MESSAGE,
  OUTSIDE_PROJECT_MESSAGE,
  PERMISSION_MODES,
  classifyBash,
  decide,
  hookOutput,
  insideProject,
  mayEdit,
} from '../src/policy.mjs';

const POLICIES = ['discuss', 'guided', 'check', 'takeover'];
const MODES = ['ask', 'auto', 'bypass'];

const CLASSIFY_CASES = [
  // From the plan.
  ['ls -la', 'read'],
  ['git status', 'read'],
  ['git push', 'other'],
  ['cargo test', 'build'],
  ['cargo test 2>&1 | tail -20', 'build'],
  ['cd crates/x && cargo check', 'build'],
  ['cd .. && ls', 'other'],
  ['cargo fmt', 'other'],
  ['cargo fmt --check', 'build'],
  ['cargo clippy --fix', 'other'],
  ['RUST_BACKTRACE=1 cargo test', 'build'],
  ['npm run build', 'build'],
  ['npm install left-pad', 'other'],
  ['tsc', 'other'],
  ['tsc --noEmit', 'build'],
  ["find . -name '*.rs'", 'read'],
  ['find . -delete', 'other'],
  ['echo hi > file', 'other'],
  ['cat a; rm b', 'other'],
  ['ls $(pwd)', 'other'],
  ['sed -i s/a/b/ f', 'other'],
  ['python3 -m pytest -q', 'build'],
  ['go test ./...', 'build'],
  ['cargo test | sh', 'other'],
  ['', 'other'],
  // Shell syntax that is never classified.
  ['   ', 'other'],
  ['ls\nrm -rf x', 'other'],
  ['ls `pwd`', 'other'],
  ['echo ${HOME}', 'other'],
  ['cat < input', 'other'],
  ['echo hi >> file', 'other'],
  ['cargo test 2> err.txt', 'other'],
  ['cargo test &', 'other'],
  ['cargo test &> log', 'other'],
  ['cargo test |& tail', 'other'],
  ['ls || rm x', 'other'],
  ['ls &&', 'other'],
  ['ls && && pwd', 'other'],
  ['ls | ', 'other'],
  // Read commands.
  ['pwd', 'read'],
  ['cat src/main.rs', 'read'],
  ['head -n 20 src/lib.rs', 'read'],
  ['tail -5 Cargo.toml', 'read'],
  ['wc -l src/*.rs', 'read'],
  ['grep -rn "fn main" src', 'read'],
  ['grep -w main src/main.rs', 'read'],
  ['rg TODO', 'read'],
  ['tree src', 'read'],
  ['file Cargo.lock', 'read'],
  ['stat README.md', 'read'],
  ['which cargo', 'read'],
  ['echo hello', 'read'],
  ['git diff --stat', 'read'],
  ['git log --oneline -5', 'read'],
  ['git show HEAD', 'read'],
  ['git branch', 'read'],
  ['git ls-files', 'read'],
  ['git blame src/lib.rs', 'read'],
  ['git rev-parse HEAD', 'read'],
  ['git', 'other'],
  ['git checkout main', 'other'],
  ['git commit -m x', 'other'],
  ['cd src', 'read'],
  ['cd src && ls', 'read'],
  ['cd /tmp && ls', 'other'],
  ['cd', 'other'],
  ['cd a b', 'other'],
  ['cd a/../.. && ls', 'other'],
  ['cd ./a/b && pwd', 'read'],
  ['ls | grep foo | wc -l', 'read'],
  ['git log | head', 'read'],
  ['cat a | sort | uniq', 'read'],
  ['ls | xargs rm', 'other'],
  ['ls | headless', 'other'],
  ['find . -exec rm {} +', 'other'],
  ['find . -execdir rm {} +', 'other'],
  ['find . -ok rm {} ;', 'other'],
  ['find . -fprint out', 'other'],
  ['find . -fprintf out %p', 'other'],
  // Read commands that would still write files or run programs.
  ['find . -fprint0 out', 'other'],
  ['find . -fls out', 'other'],
  ['find . -okdir rm {} +', 'other'],
  ['git diff --output=patch.diff', 'other'],
  ['git log --output patch.txt', 'other'],
  ['rg --pre ./script pattern', 'other'],
  ['rg --pre=sh pattern', 'other'],
  ['ls | sort -o out.txt', 'other'],
  ['ls | sort -no out.txt', 'other'],
  ['ls | sort --output=out.txt', 'other'],
  ['ls | sort -n', 'read'],
  ['ls | uniq - out.txt', 'other'],
  ['ls | uniq -c', 'read'],
  ['tree -o out.txt', 'other'],
  ['git log --outp=patch.txt', 'other'],
  ['git log --oneline --no-merges', 'read'],
  ['rg --hostname-bin=./script pattern', 'other'],
  ['rg --pretty pattern', 'read'],
  ['ls | sort --compress-program=sh', 'other'],
  ['ls | sort --compress=sh', 'other'],
  ['file -C -m magic', 'other'],
  ['file --compile -m magic', 'other'],
  ['grep -rn -- --output src', 'read'],
  ['GIT_EXTERNAL_DIFF=rm git diff', 'other'],
  ['LC_ALL=C ls', 'other'],
  ['cd ~ && ls', 'other'],
  ['cd ~/elsewhere', 'other'],
  ['cd - && ls', 'other'],
  ['cd $HOME && ls', 'other'],
  ['git branch -a', 'read'],
  ['git branch --list --verbose', 'read'],
  ['git branch --show-current', 'read'],
  ['git branch feature', 'other'],
  ['git branch -D main', 'other'],
  ['git branch -m old new', 'other'],
  ['tree -ao out.txt', 'other'],
  ['tree -R', 'other'],
  ['tree -a src', 'read'],
  ['file -bC -m magic', 'other'],
  ['file -b src/main.rs', 'read'],
  // Quotes, backslashes and expansions are read the way the shell reads them.
  ["find . '-delete'", 'other'],
  ['find . -"exec" rm {} +', 'other'],
  ['find . -"exec" rm x \\;', 'other'],
  ['find . -\\delete', 'other'],
  ['find . -{delete,print}', 'other'],
  ['find .$IFS-delete', 'other'],
  ["find . $'-delete'", 'other'],
  ['find . $"-delete"', 'other'],
  ["cargo clippy '--fix'", 'other'],
  ['cargo clippy --f\\ix', 'other'],
  ["cargo fmt '--check'", 'build'],
  ["find . 'x|grep ' -delete", 'other'],
  ["find . 'x&&ls ' -delete", 'other'],
  ['cargo fmt # --check', 'other'],
  ['tsc # --noEmit', 'other'],
  ['ls -l2>&1x', 'other'],
  ['cargo test 2>&1x', 'other'],
  ["cargo test '2'>&1", 'other'],
  ['cargo test 2>&1|tail', 'build'],
  ['cargo test 2>&1&&ls', 'build'],
  ['ls "$HOME"', 'other'],
  ['ls "`pwd`"', 'other'],
  ['ls $', 'other'],
  ['(rm x)', 'other'],
  ['ls && (rm x)', 'other'],
  ["cat 'unterminated", 'other'],
  ['cat "unterminated', 'other'],
  ['cat trailing\\', 'other'],
  ['ls\r-la', 'other'],
  ['ls -la', 'other'],
  ['ls\t-la', 'read'],
  ["grep 'a|b' src", 'read'],
  ["grep 'a;b > c' src", 'read'],
  ['grep "fn main(" src', 'read'],
  ["grep '$HOME {x}' src", 'read'],
  ['grep "say \\"hi\\"" src', 'read'],
  ["'ls' -la", 'read'],
  ['cat src/main\\ copy.rs', 'read'],
  ["find . -name '*.rs' -print", 'read'],
  // A glob is only accepted when none of its matches can start with `-`.
  ['find *', 'other'],
  ['find . -name *.rs', 'other'],
  ['rg pattern ?', 'other'],
  ['rg pattern [a-z]', 'other'],
  ["rg pattern ''*", 'other'],
  ['cargo clippy --fi?', 'other'],
  ["cargo clippy '-'-fi*", 'other'],
  ['cat src/*.rs', 'read'],
  ['ls .*', 'read'],
  ['ls | uniq s*', 'other'],
  ['cd s*', 'other'],
  // Build commands.
  ['cargo build --release', 'build'],
  ['cargo check -p tandem', 'build'],
  ['cargo clippy --workspace --all-targets -- -D warnings', 'build'],
  ['cargo doc', 'build'],
  ['cargo tree', 'build'],
  ['cargo metadata', 'build'],
  ['cargo nextest run', 'build'],
  ['cargo run', 'other'],
  ['cargo install ripgrep', 'other'],
  ['cargo', 'other'],
  ['cargo fmt -- --check', 'build'],
  ['go build ./...', 'build'],
  ['go vet ./...', 'build'],
  ['go list ./...', 'build'],
  ['go env GOPATH', 'build'],
  ['go version', 'build'],
  ['go run .', 'other'],
  ['go mod tidy', 'other'],
  ['gofmt -l .', 'build'],
  ['gofmt -l -w .', 'other'],
  ['gofmt -l --w .', 'other'],
  ['gofmt -l -w=true .', 'other'],
  ['gofmt -d .', 'other'],
  ['npm test', 'build'],
  ['pnpm test', 'build'],
  ['yarn test', 'build'],
  ['bun test', 'build'],
  ['npm run test', 'build'],
  ['npm run lint', 'build'],
  ['npm run lint -- --fix', 'other'],
  ['npm run typecheck', 'build'],
  ['pnpm run check', 'build'],
  ['yarn run tsc', 'build'],
  ['npm run dev', 'other'],
  ['npm run', 'other'],
  ['npm ci', 'other'],
  ['npx tsc --noEmit', 'build'],
  ['npx tsc', 'other'],
  ['npx eslint src', 'build'],
  ['npx eslint --fix src', 'other'],
  ['npx eslint --fix=true src', 'other'],
  ['npx jest', 'build'],
  ['npx vitest run', 'build'],
  ['npx vitest', 'other'],
  ['npx prettier --check .', 'build'],
  ['npx prettier --write .', 'other'],
  ['npx prettier --check --write .', 'other'],
  ['npx left-pad', 'other'],
  ['node --test', 'build'],
  ['node --check main.mjs', 'build'],
  ['node --version', 'build'],
  ['node main.mjs', 'other'],
  ['pytest', 'build'],
  ['pytest -x tests/test_a.py', 'build'],
  ['python -m unittest discover', 'build'],
  ['python3 -m py_compile a.py', 'build'],
  ['python3 -m pip install x', 'other'],
  ['python3 script.py', 'other'],
  ['ruff check .', 'build'],
  ['ruff check --fix .', 'other'],
  ['ruff check --fix-only .', 'other'],
  ['ruff format .', 'other'],
  ['mypy src', 'build'],
  ['mvn test', 'build'],
  ['mvn compile', 'build'],
  ['mvn verify', 'build'],
  ['mvn package', 'build'],
  ['mvn deploy', 'other'],
  ['gradle test', 'build'],
  ['./gradlew build', 'build'],
  ['./gradlew check', 'build'],
  ['./gradlew compileJava', 'build'],
  ['./gradlew bootRun', 'other'],
  ['javac Main.java', 'build'],
  ['swift build', 'build'],
  ['swift test', 'build'],
  ['swift test --filter Foo', 'build'],
  ['swift run', 'other'],
  ['swift package update', 'other'],
  ['make test', 'build'],
  ['make check', 'build'],
  ['make lint', 'build'],
  ['make build', 'build'],
  ['make install', 'other'],
  ['make', 'other'],
  ['RUST_LOG=debug CARGO_TERM_COLOR=always cargo test', 'build'],
  ['FOO=a/b:c-d.e cargo test', 'build'],
  ['FOO="x y" cargo test', 'other'],
  ['lower=1 cargo test', 'other'],
  ['cargo test -- -w', 'other'],
  ['cargo test --write', 'other'],
  ['cargo test 2>&1 | grep -c FAIL', 'build'],
  ['cargo test 2>&1 | tail -n 20 && git status', 'build'],
  ['cd crates/x && cargo test && cd .. && ls', 'other'],
  ['git status && npm install', 'other'],
  ['cargo test | tee out.txt', 'other'],
];

test('classifyBash', async (t) => {
  for (const [command, expected] of CLASSIFY_CASES) {
    await t.test(JSON.stringify(command), () => {
      assert.equal(classifyBash(command), expected);
    });
  }
});

let root;
let project;
let outside;

before(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'tandem-policy-'));
  project = path.join(root, 'project');
  outside = path.join(root, 'outside');
  fs.mkdirSync(path.join(project, 'src'), { recursive: true });
  fs.mkdirSync(outside);
  fs.writeFileSync(path.join(project, 'src', 'main.rs'), 'fn main() {}\n');
  fs.writeFileSync(path.join(outside, 'secret.txt'), 'x\n');
  fs.symlinkSync(outside, path.join(project, 'escape'));
  fs.symlinkSync(path.join(outside, 'secret.txt'), path.join(project, 'secret-link.txt'));
  fs.symlinkSync(path.join(outside, 'missing.txt'), path.join(project, 'dangling-link.txt'));
  fs.symlinkSync(path.join(project, 'src'), path.join(project, 'src-link'));
  // A sibling folder whose name starts with the project's name must not count as inside it.
  fs.mkdirSync(path.join(root, 'project-other'));
});

after(() => {
  fs.rmSync(root, { recursive: true, force: true });
});

test('insideProject accepts paths inside the project, existing or not', () => {
  assert.equal(insideProject(project, 'src/main.rs'), true);
  assert.equal(insideProject(project, 'src/new/deeper/file.rs'), true);
  assert.equal(insideProject(project, path.join(project, 'README.md')), true);
  assert.equal(insideProject(project, '.'), true);
  assert.equal(insideProject(project, 'src/../Cargo.toml'), true);
  assert.equal(insideProject(project, 'src-link/main.rs'), true);
  assert.equal(insideProject(project, 'src/main.rs/child'), true);
});

test('insideProject resolves a project folder reached through a symlink', () => {
  const alias = path.join(root, 'alias');
  fs.symlinkSync(project, alias);
  assert.equal(insideProject(alias, 'src/main.rs'), true);
  assert.equal(insideProject(alias, path.join(project, 'src/main.rs')), true);
  assert.equal(insideProject(project, path.join(alias, 'src/main.rs')), true);
});

test('insideProject rejects paths outside the project', () => {
  assert.equal(insideProject(project, '../outside.txt'), false);
  assert.equal(insideProject(project, path.join(outside, 'secret.txt')), false);
  assert.equal(insideProject(project, '/etc/hosts'), false);
  assert.equal(insideProject(project, path.join(root, 'project-other', 'a.txt')), false);
  assert.equal(insideProject(project, 'escape/secret.txt'), false);
  assert.equal(insideProject(project, 'escape/new.txt'), false);
  assert.equal(insideProject(project, 'secret-link.txt'), false);
  assert.equal(insideProject(project, 'dangling-link.txt'), false);
  assert.equal(insideProject(path.join(root, 'no-such-project'), 'a.txt'), false);
});

test('EDIT_TOOLS', () => {
  assert.deepEqual([...EDIT_TOOLS].sort(), ['Edit', 'NotebookEdit', 'Write']);
});

test('edit tools are denied outside takeover', () => {
  for (const policy of ['discuss', 'guided', 'check']) {
    for (const tool of EDIT_TOOLS) {
      const input =
        tool === 'NotebookEdit' ? { notebook_path: 'a.ipynb' } : { file_path: 'src/main.rs' };
      assert.deepEqual(
        decide({ policy, tool, input, cwd: project }),
        { decision: 'deny', reason: GUIDED_EDIT_MESSAGE },
        `${tool} in ${policy}`,
      );
    }
  }
});

test('edit tools inside the project pass during takeover', () => {
  for (const [tool, input] of [
    ['Edit', { file_path: 'src/main.rs' }],
    ['Write', { file_path: path.join(project, 'src', 'new.rs') }],
    ['NotebookEdit', { notebook_path: 'notebooks/a.ipynb' }],
  ]) {
    assert.deepEqual(decide({ policy: 'takeover', tool, input, cwd: project }), {
      decision: 'pass',
    });
  }
});

test('edit tools outside the project are denied during takeover', () => {
  for (const file_path of [
    '../outside.txt',
    path.join(outside, 'new.txt'),
    'escape/secret.txt',
    'secret-link.txt',
    'dangling-link.txt',
  ]) {
    for (const tool of EDIT_TOOLS) {
      const input = tool === 'NotebookEdit' ? { notebook_path: file_path } : { file_path };
      assert.deepEqual(
        decide({ policy: 'takeover', tool, input, cwd: project }),
        { decision: 'deny', reason: OUTSIDE_PROJECT_MESSAGE },
        `${tool} ${file_path}`,
      );
    }
  }
});

test('edit tools without a path are denied during takeover', () => {
  for (const input of [{}, { file_path: '' }, { file_path: 42 }, { content: 'x' }]) {
    for (const tool of EDIT_TOOLS) {
      assert.deepEqual(decide({ policy: 'takeover', tool, input, cwd: project }), {
        decision: 'deny',
        reason: OUTSIDE_PROJECT_MESSAGE,
      });
    }
  }
  assert.deepEqual(decide({ policy: 'takeover', tool: 'Write', input: undefined, cwd: project }), {
    decision: 'deny',
    reason: OUTSIDE_PROJECT_MESSAGE,
  });
});

test('Bash read commands are allowed in every policy', () => {
  for (const policy of POLICIES) {
    assert.deepEqual(
      decide({ policy, tool: 'Bash', input: { command: 'git status' }, cwd: project }),
      { decision: 'allow' },
      policy,
    );
  }
});

test('Bash build commands are allowed in check and takeover and asked otherwise', () => {
  const expected = { discuss: 'ask', guided: 'ask', check: 'allow', takeover: 'allow' };
  for (const policy of POLICIES) {
    assert.deepEqual(
      decide({ policy, tool: 'Bash', input: { command: 'cargo test' }, cwd: project }),
      { decision: expected[policy] },
      policy,
    );
  }
});

test('other Bash commands are asked, except during takeover where they pass', () => {
  const expected = { discuss: 'ask', guided: 'ask', check: 'ask', takeover: 'pass' };
  for (const policy of POLICIES) {
    assert.deepEqual(
      decide({ policy, tool: 'Bash', input: { command: 'npm install left-pad' }, cwd: project }),
      { decision: expected[policy] },
      policy,
    );
  }
});

test('Bash without a command is asked', () => {
  assert.deepEqual(decide({ policy: 'guided', tool: 'Bash', input: {}, cwd: project }), {
    decision: 'ask',
  });
});

test('Tandem tools are allowed and other tools pass', () => {
  for (const policy of POLICIES) {
    assert.deepEqual(
      decide({ policy, tool: 'mcp__tandem__show_step', input: { step: 1 }, cwd: project }),
      { decision: 'allow' },
    );
    assert.deepEqual(
      decide({ policy, tool: 'Read', input: { file_path: '/etc/hosts' }, cwd: project }),
      { decision: 'pass' },
    );
    assert.deepEqual(
      decide({ policy, tool: 'mcp__brain__search', input: { query: 'x' }, cwd: project }),
      { decision: 'pass' },
    );
  }
});

test('PERMISSION_MODES', () => {
  assert.deepEqual([...PERMISSION_MODES].sort(), ['ask', 'auto', 'bypass']);
});

test('mayEdit allows edits during a takeover and in every bypass turn', () => {
  for (const mode of MODES) {
    assert.equal(mayEdit('takeover', mode), true, `takeover in ${mode}`);
  }
  for (const policy of POLICIES) {
    assert.equal(mayEdit(policy, 'bypass'), true, `${policy} in bypass`);
  }
  for (const policy of ['discuss', 'guided', 'check']) {
    for (const mode of ['ask', 'auto', 'yolo']) {
      assert.equal(mayEdit(policy, mode), false, `${policy} in ${mode}`);
    }
  }
});

const ALLOW = { decision: 'allow' };
const ASK = { decision: 'ask' };
const PASS = { decision: 'pass' };
const DENY_GUIDED = { decision: 'deny', reason: GUIDED_EDIT_MESSAGE };
const DENY_OUTSIDE = { decision: 'deny', reason: OUTSIDE_PROJECT_MESSAGE };

/** The same result in discuss, guided, check and takeover. */
function everyPolicy(result) {
  return POLICIES.map(() => result);
}

/** The input of an edit tool that targets `target`, or no path at all for undefined. */
function editInput(tool, target) {
  if (target === undefined) {
    return {};
  }
  return tool === 'NotebookEdit' ? { notebook_path: target } : { file_path: target };
}

/**
 * The hook table of the plan: each tool call with its expected result per mode, listed in
 * POLICIES order. Built inside a test because the paths depend on the `before` hook.
 */
function hookTable() {
  const rows = [
    {
      label: 'Bash git status',
      tool: 'Bash',
      input: { command: 'git status' },
      expected: { ask: everyPolicy(ALLOW), auto: everyPolicy(ALLOW), bypass: everyPolicy(ALLOW) },
    },
    {
      label: 'Bash cargo test',
      tool: 'Bash',
      input: { command: 'cargo test' },
      expected: {
        ask: [ASK, ASK, ALLOW, ALLOW],
        auto: everyPolicy(ALLOW),
        bypass: everyPolicy(ALLOW),
      },
    },
    {
      label: 'Bash npm install left-pad',
      tool: 'Bash',
      input: { command: 'npm install left-pad' },
      expected: { ask: [ASK, ASK, ASK, PASS], auto: everyPolicy(PASS), bypass: everyPolicy(PASS) },
    },
    {
      label: 'mcp__tandem__show_step',
      tool: 'mcp__tandem__show_step',
      input: { step: 1 },
      expected: { ask: everyPolicy(ALLOW), auto: everyPolicy(ALLOW), bypass: everyPolicy(ALLOW) },
    },
    {
      label: 'Read',
      tool: 'Read',
      input: { file_path: '/etc/hosts' },
      expected: { ask: everyPolicy(PASS), auto: everyPolicy(PASS), bypass: everyPolicy(PASS) },
    },
    {
      label: 'mcp__brain__search',
      tool: 'mcp__brain__search',
      input: { query: 'x' },
      expected: { ask: everyPolicy(PASS), auto: everyPolicy(PASS), bypass: everyPolicy(PASS) },
    },
  ];
  const outsideOrMissing = [
    '../outside.txt',
    path.join(outside, 'new.txt'),
    'escape/secret.txt',
    'secret-link.txt',
    'dangling-link.txt',
    undefined,
  ];
  for (const tool of EDIT_TOOLS) {
    const inside = tool === 'NotebookEdit' ? 'notebooks/a.ipynb' : 'src/main.rs';
    const refusedOutsideTakeover = [DENY_GUIDED, DENY_GUIDED, DENY_GUIDED, PASS];
    rows.push({
      label: `${tool} ${inside}`,
      tool,
      input: editInput(tool, inside),
      expected: {
        ask: refusedOutsideTakeover,
        auto: refusedOutsideTakeover,
        bypass: everyPolicy(PASS),
      },
    });
    for (const target of outsideOrMissing) {
      const refused = [DENY_GUIDED, DENY_GUIDED, DENY_GUIDED, DENY_OUTSIDE];
      rows.push({
        label: `${tool} ${target ?? 'without a path'}`,
        tool,
        input: editInput(tool, target),
        expected: { ask: refused, auto: refused, bypass: everyPolicy(DENY_OUTSIDE) },
      });
    }
  }
  return rows;
}

test('decide follows the mode and policy matrix', () => {
  for (const { label, tool, input, expected } of hookTable()) {
    for (const mode of MODES) {
      POLICIES.forEach((policy, column) => {
        assert.deepEqual(
          decide({ policy, mode, tool, input, cwd: project }),
          expected[mode][column],
          `${label} in ${policy}, ${mode}`,
        );
      });
    }
  }
});

test('decide in ask, or in an unknown mode, decides as without a mode', () => {
  for (const { label, tool, input } of hookTable()) {
    for (const policy of POLICIES) {
      const withoutMode = decide({ policy, tool, input, cwd: project });
      for (const mode of ['ask', 'yolo']) {
        assert.deepEqual(
          decide({ policy, mode, tool, input, cwd: project }),
          withoutMode,
          `${label} in ${policy}, ${mode}`,
        );
      }
    }
  }
});

test('Bash without a command is asked only in ask', () => {
  const expected = { ask: ASK, auto: PASS, bypass: PASS };
  for (const mode of MODES) {
    assert.deepEqual(
      decide({ policy: 'guided', mode, tool: 'Bash', input: {}, cwd: project }),
      expected[mode],
      mode,
    );
  }
});

test('hookOutput shapes', () => {
  assert.deepEqual(hookOutput({ decision: 'allow' }), {
    hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'allow' },
  });
  assert.deepEqual(hookOutput({ decision: 'deny', reason: GUIDED_EDIT_MESSAGE }), {
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision: 'deny',
      permissionDecisionReason: GUIDED_EDIT_MESSAGE,
    },
  });
  assert.deepEqual(hookOutput({ decision: 'ask' }), {
    hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'ask' },
  });
  assert.deepEqual(hookOutput({ decision: 'pass' }), {});
});

test('messages', () => {
  assert.equal(
    GUIDED_EDIT_MESSAGE,
    'Tandem is in guided mode: the user writes the code. Explain the change instead of editing files.',
  );
  assert.equal(
    OUTSIDE_PROJECT_MESSAGE,
    'Tandem only lets Claude change files inside the project folder.',
  );
});
