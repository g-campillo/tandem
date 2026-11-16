#!/usr/bin/env node
// Tandem's Claude helper: wire protocol v1 frames on stdin and stdout, logs on stderr.

import { Console } from 'node:console';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createFrameReader, encodeFrame } from './src/frames.mjs';
import { createHelper } from './src/host.mjs';

// Stdout carries only frames, so stray logging from any library goes to stderr.
const stderrConsole = new Console({ stdout: process.stderr, stderr: process.stderr });
for (const method of ['log', 'info', 'warn', 'debug', 'dir', 'dirxml', 'table']) {
  console[method] = stderrConsole[method].bind(stderrConsole);
}

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SDK_PACKAGE = path.join(
  HERE,
  'node_modules',
  '@anthropic-ai',
  'claude-agent-sdk',
  'package.json',
);
const SDK_ARGUMENT = '--sdk=';

let helper = null;
let exiting = false;

function oneLine(text) {
  return String(text).replace(/\s*\n\s*/g, ' ');
}

function logLine(text) {
  process.stderr.write(`tandem-claude-helper: ${oneLine(text)}\n`);
}

function messageOf(error) {
  return error?.message ?? error;
}

/** Exits once everything written so far has left both pipes (pipe writes are async on macOS). */
function exit(code) {
  if (exiting) {
    return;
  }
  exiting = true;
  process.exitCode = code;
  let flushing = 2;
  const flushed = () => {
    flushing -= 1;
    if (flushing === 0) {
      process.exit(code);
    }
  };
  process.stdout.write('', flushed);
  process.stderr.write('', flushed);
}

function closeHelper() {
  try {
    helper?.close();
  } catch (error) {
    logLine(`closing failed: ${messageOf(error)}`);
  }
}

function fatal(error) {
  logLine(`fatal: ${messageOf(error)}`);
  closeHelper();
  exit(1);
}

process.on('uncaughtException', fatal);
process.on('unhandledRejection', fatal);

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

/** The real SDK, or the module named by `--sdk=<file>` (tests only; Tandem never passes it). */
async function loadSdk(argv) {
  const override = argv.find((argument) => argument.startsWith(SDK_ARGUMENT));
  if (override) {
    const file = path.resolve(override.slice(SDK_ARGUMENT.length));
    return { sdk: await import(pathToFileURL(file).href), sdkVersion: 'mock', claudeCode: 'mock' };
  }
  const sdk = await import('@anthropic-ai/claude-agent-sdk');
  const sdkPackage = readJson(SDK_PACKAGE);
  return { sdk, sdkVersion: sdkPackage.version, claudeCode: sdkPackage.claudeCodeVersion };
}

async function main() {
  const { sdk, sdkVersion, claudeCode } = await loadSdk(process.argv.slice(2));
  const versions = {
    helper: readJson(path.join(HERE, 'package.json')).version,
    sdk: sdkVersion,
    claudeCode,
  };
  helper = createHelper({
    sdk,
    write: (frame) => process.stdout.write(encodeFrame(frame)),
    versions,
    env: process.env,
    onExit: exit,
  });

  const reader = createFrameReader(
    (frame) => {
      // Not awaited: a response must be handled while a tool call waits for it.
      helper
        .handle(frame)
        .catch((error) => logLine(`handling ${frame.type} failed: ${messageOf(error)}`));
    },
    (error) => {
      logLine(error.message);
      closeHelper();
      exit(2);
    },
  );
  process.stdin.on('data', (chunk) => reader.push(chunk));
  process.stdin.on('end', () => {
    reader.end();
    closeHelper();
    exit(0);
  });
}

main().catch(fatal);
