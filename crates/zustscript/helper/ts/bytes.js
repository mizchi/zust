export function asBytes(view) {
  return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
}

export function writeBytes(memory, offset, source) {
  const target = new Uint8Array(memory.buffer, offset, source.byteLength);
  target.set(asBytes(source));
}

export function readBytes(memory, offset, length) {
  return new Uint8Array(memory.buffer, offset, length).slice();
}
