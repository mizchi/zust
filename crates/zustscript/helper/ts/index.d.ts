// zustscript-helper: the values a zustscript module passes between JS and wasm, in one stable byte format

/** The shape of a value: how it is written (the bytes carry no type tags) */
export type Schema =
  | "i32"
  | "u32"
  | "i64"
  | "f32"
  | "f64"
  | "bool"
  | "str"
  | readonly ["arr", Schema]
  | readonly ["rec", ReadonlyArray<readonly [string, Schema]>];

/** The bytes of `value`, a value of `schema` */
export declare function encode(schema: Schema, value: unknown): Uint8Array;

/** The value of `schema` that `bytes` hold */
export declare function decode(schema: Schema, bytes: Uint8Array): unknown;
