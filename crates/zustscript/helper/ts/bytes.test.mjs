import assert from "node:assert/strict";
import test from "node:test";
import { asBytes, readBytes, writeBytes } from "zustscript-helper/bytes";

test("asBytes shares just the visible range, measured in bytes", () => {
  const words = new Uint16Array([1, 2, 3]);
  const part = words.subarray(1, 2);
  const bytes = asBytes(part);
  assert.equal(bytes.buffer, words.buffer);
  assert.equal(bytes.byteOffset, part.byteOffset);
  assert.equal(bytes.length, 2);
  bytes.fill(255);
  assert.deepEqual([...words], [1, 65535, 3]);
  assert.equal(asBytes(part.subarray(1)).length, 0);
});

test("writes honor view offsets and reads own their bytes after reuse and growth", () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const source = Uint8Array.of(9, 0, 255, 8);
  const part = new DataView(source.buffer, 1, 2);
  writeBytes(memory, 7, part);
  source.fill(1);
  const owned = readBytes(memory, 7, 2);
  assert.deepEqual([...readBytes(memory, 6, 4)], [0, 0, 255, 0]);
  writeBytes(memory, 7, Buffer.from([3, 4, 5]).subarray(1));
  memory.grow(1);
  assert.deepEqual([...owned], [0, 255]);
  assert.deepEqual([...readBytes(memory, 7, 2)], [4, 5]);
  const end = memory.buffer.byteLength;
  writeBytes(memory, end, new Uint8Array(0));
  assert.equal(readBytes(memory, end, 0).length, 0);
  assert.throws(() => readBytes(memory, end, 1), RangeError);
  assert.throws(() => writeBytes(memory, end, source), RangeError);
});
