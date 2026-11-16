// Newline-delimited JSON framing for wire protocol v1 between Tandem and this helper.

export const PROTOCOL_VERSION = 1;
export const MAX_FRAME_BYTES = 8 * 1024 * 1024;

const NEWLINE = 0x0a;

/**
 * Serialises one frame as a protocol line. The byte limit covers the whole line, newline
 * included, so every frame this helper writes fits any reader that enforces the limit.
 */
export function encodeFrame(frame) {
  const line = `${JSON.stringify({ v: PROTOCOL_VERSION, ...frame })}\n`;
  if (Buffer.byteLength(line, 'utf8') > MAX_FRAME_BYTES) {
    throw new RangeError('frame too large');
  }
  return line;
}

function isPlainObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Splits a byte stream into frames. Bytes are buffered and decoded one whole line at a
 * time, so a multi-byte character split across chunks decodes correctly. The first problem
 * is reported once through `onError`, after which all input is ignored.
 */
export function createFrameReader(onFrame, onError) {
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let pending = [];
  let pendingBytes = 0;
  let failed = false;

  function fail(message) {
    failed = true;
    pending = [];
    pendingBytes = 0;
    onError(new Error(message));
  }

  function parseLine(bytes) {
    if (bytes.length === 0) {
      return;
    }
    if (bytes.length > MAX_FRAME_BYTES) {
      fail('frame too large');
      return;
    }
    let frame;
    try {
      frame = JSON.parse(decoder.decode(bytes));
    } catch {
      fail('invalid frame');
      return;
    }
    if (!isPlainObject(frame) || frame.v !== PROTOCOL_VERSION || typeof frame.type !== 'string') {
      fail('invalid frame');
      return;
    }
    onFrame(frame);
  }

  function push(chunk) {
    if (failed) {
      return;
    }
    const bytes = typeof chunk === 'string' ? Buffer.from(chunk, 'utf8') : Buffer.from(chunk);
    let start = 0;
    let newline = bytes.indexOf(NEWLINE, start);
    while (newline !== -1) {
      const tail = bytes.subarray(start, newline);
      const line = pending.length === 0 ? tail : Buffer.concat([...pending, tail]);
      pending = [];
      pendingBytes = 0;
      parseLine(line);
      if (failed) {
        return;
      }
      start = newline + 1;
      newline = bytes.indexOf(NEWLINE, start);
    }
    if (start < bytes.length) {
      const rest = bytes.subarray(start);
      pending.push(rest);
      pendingBytes += rest.length;
      if (pendingBytes > MAX_FRAME_BYTES) {
        fail('frame too large');
      }
    }
  }

  function end() {
    if (failed) {
      return;
    }
    if (pendingBytes > 0) {
      fail('incomplete frame');
    }
  }

  return { push, end };
}
