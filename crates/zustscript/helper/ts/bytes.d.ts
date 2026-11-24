/** A fixed-length byte view sharing the input's visible storage, in native byte order. */
export declare function asBytes<V extends ArrayBufferView>(view: V): Uint8Array & { readonly buffer: V["buffer"] };

/** Copy the visible bytes into an allocated region of Wasm linear memory at a byte offset. */
export declare function writeBytes(memory: WebAssembly.Memory, offset: number, source: ArrayBufferView): void;

/** Copy an allocated region into JS-owned bytes before Rust reuses or releases it. */
export declare function readBytes(memory: WebAssembly.Memory, offset: number, length: number): ReturnType<Uint8Array["slice"]>;
