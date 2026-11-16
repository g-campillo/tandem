import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_FRAME_BYTES,
  PROTOCOL_VERSION,
  createFrameReader,
  encodeFrame,
} from '../src/frames.mjs';

function collect() {
  const frames = [];
  const errors = [];
  const reader = createFrameReader(
    (frame) => frames.push(frame),
    (error) => errors.push(error),
  );
  return { reader, frames, errors };
}

test('protocol constants', () => {
  assert.equal(PROTOCOL_VERSION, 1);
  assert.equal(MAX_FRAME_BYTES, 8 * 1024 * 1024);
});

test('encodeFrame adds v: 1 and a trailing newline', () => {
  const line = encodeFrame({ type: 'hello', id: 1 });
  assert.ok(line.endsWith('\n'));
  assert.equal(line.indexOf('\n'), line.length - 1);
  assert.deepEqual(JSON.parse(line), { v: 1, type: 'hello', id: 1 });
});

test('encodeFrame throws on an oversized frame', () => {
  const text = 'x'.repeat(MAX_FRAME_BYTES);
  assert.throws(() => encodeFrame({ type: 'event', text }), {
    name: 'RangeError',
    message: 'frame too large',
  });
});

test('encodeFrame counts UTF-8 bytes, not characters', () => {
  // Each "é" is two bytes, so half the limit in characters is exactly the limit in bytes
  // before the JSON envelope is added.
  const text = 'é'.repeat(MAX_FRAME_BYTES / 2);
  assert.throws(() => encodeFrame({ type: 'event', text }), RangeError);
});

test('a frame round trips through the reader', () => {
  const { reader, frames, errors } = collect();
  const frame = { type: 'event', event: 'text_delta', text: 'héllo → wörld' };
  reader.push(encodeFrame(frame));
  reader.end();
  assert.deepEqual(frames, [{ v: 1, ...frame }]);
  assert.deepEqual(errors, []);
});

test('several frames in one chunk', () => {
  const { reader, frames, errors } = collect();
  reader.push(
    Buffer.from(
      encodeFrame({ type: 'hello', id: 1 }) +
        encodeFrame({ type: 'interrupt', id: 2 }) +
        encodeFrame({ type: 'shutdown', id: 3 }),
    ),
  );
  assert.deepEqual(
    frames.map((frame) => frame.type),
    ['hello', 'interrupt', 'shutdown'],
  );
  assert.deepEqual(errors, []);
});

test('one frame split across chunks, including inside a multi-byte character', () => {
  const { reader, frames, errors } = collect();
  const bytes = Buffer.from(encodeFrame({ type: 'event', text: 'a€b🙂c' }));
  const euro = bytes.indexOf(Buffer.from('€'));
  const smile = bytes.indexOf(Buffer.from('🙂'));
  const cuts = [3, euro + 1, euro + 2, smile + 1, smile + 3, bytes.length - 1];
  let start = 0;
  for (const cut of cuts) {
    reader.push(bytes.subarray(start, cut));
    assert.equal(frames.length, 0);
    start = cut;
  }
  reader.push(bytes.subarray(start));
  reader.end();
  assert.deepEqual(frames, [{ v: 1, type: 'event', text: 'a€b🙂c' }]);
  assert.deepEqual(errors, []);
});

test('string chunks are accepted', () => {
  const { reader, frames } = collect();
  reader.push('{"v":1,"type":"hel');
  reader.push('lo","id":1}\n');
  assert.deepEqual(frames, [{ v: 1, type: 'hello', id: 1 }]);
});

test('empty lines are skipped', () => {
  const { reader, frames, errors } = collect();
  reader.push('\n\n{"v":1,"type":"hello","id":1}\n\n{"v":1,"type":"shutdown","id":2}\n\n');
  reader.end();
  assert.deepEqual(
    frames.map((frame) => frame.type),
    ['hello', 'shutdown'],
  );
  assert.deepEqual(errors, []);
});

for (const [name, line] of [
  ['non-JSON', 'not json'],
  ['an array', '[1,2,3]'],
  ['a JSON string', '"hello"'],
  ['null', 'null'],
  ['v: 2', '{"v":2,"type":"hello","id":1}'],
  ['missing v', '{"type":"hello","id":1}'],
  ['missing type', '{"v":1,"id":1}'],
  ['a non-string type', '{"v":1,"type":7,"id":1}'],
]) {
  test(`${name} calls onError once and stops further frames`, () => {
    const { reader, frames, errors } = collect();
    reader.push('{"v":1,"type":"hello","id":1}\n');
    reader.push(`${line}\n{"v":1,"type":"shutdown","id":2}\n`);
    reader.push('{"v":1,"type":"shutdown","id":3}\n');
    reader.push('also not json\n');
    reader.end();
    assert.deepEqual(
      frames.map((frame) => frame.id),
      [1],
    );
    assert.equal(errors.length, 1);
    assert.ok(errors[0] instanceof Error);
    assert.equal(errors[0].message, 'invalid frame');
  });
}

test('a line over 8 MiB errors once', () => {
  const { reader, frames, errors } = collect();
  const text = 'x'.repeat(MAX_FRAME_BYTES);
  reader.push(`{"v":1,"type":"event","text":"${text}"}\n`);
  reader.push('{"v":1,"type":"hello","id":1}\n');
  reader.end();
  assert.deepEqual(frames, []);
  assert.equal(errors.length, 1);
  assert.equal(errors[0].message, 'frame too large');
});

test('an unterminated buffer over 8 MiB errors without waiting for a newline', () => {
  const { reader, frames, errors } = collect();
  const chunk = 'x'.repeat(1024 * 1024);
  for (let index = 0; index <= 8; index += 1) {
    reader.push(chunk);
  }
  assert.equal(errors.length, 1);
  assert.equal(errors[0].message, 'frame too large');
  reader.push('\n{"v":1,"type":"hello","id":1}\n');
  reader.end();
  assert.deepEqual(frames, []);
  assert.equal(errors.length, 1);
});

test('a line of exactly 8 MiB is accepted', () => {
  const { reader, frames, errors } = collect();
  const envelope = '{"v":1,"type":"event","text":""}';
  const text = 'x'.repeat(MAX_FRAME_BYTES - envelope.length);
  const line = `{"v":1,"type":"event","text":"${text}"}`;
  assert.equal(Buffer.byteLength(line), MAX_FRAME_BYTES);
  reader.push(`${line}\n`);
  reader.end();
  assert.equal(frames.length, 1);
  assert.deepEqual(errors, []);
});

test('end() with a partial line errors', () => {
  const { reader, frames, errors } = collect();
  reader.push('{"v":1,"type":"hello","id":1}\n{"v":1,"type":"shut');
  reader.end();
  assert.equal(frames.length, 1);
  assert.equal(errors.length, 1);
  assert.equal(errors[0].message, 'incomplete frame');
});

test('end() with nothing left over is silent', () => {
  const { reader, errors } = collect();
  reader.push('{"v":1,"type":"hello","id":1}\n');
  reader.end();
  assert.deepEqual(errors, []);
});
