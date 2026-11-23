// A TS module whose record types tests/ts_import.zsc reads with `import type` (tests/examples.rs: ts_records).
// Only the types the import needs are read: the rest of the file (functions, unions, generics) is skipped.

// (a regular expression's brackets are not structure, and a division is not one)
const closing = /[}\]]/g;
const half = 4 / 2;
// (after the condition of an `if`, `/` starts a regular expression; after another `)`, it divides)
if (half) /[}]/.test("}");
const quarter = (half) / 2;
// (a property named like a keyword is not one: this `/` divides)
const obj = { if: (n: number) => n };
function f() { return obj.if(4) / 2; }
// (`for await (..)` is a condition too: a regular expression follows it)
async function g(xs: AsyncIterable<string>) { for await (const x of xs) /[}]/.test(x); }

/** a user */
export interface User {
  readonly id: number;
  name: string;
  admin: boolean;
  tags: string[];
}

export type Post = {
  author: User;
  title: string;
  scores: readonly number[];
  replies: Array<Reply>;
};

// not exported: the .d.ts declares it again with these members (a mutable array stays mutable)
interface Reply {
  by: User;
  text: string;
  tags: string[];
  grid: number[][];
}

// (members separated by line ends, as TS allows)
export interface Point {
  x: number
  y: number
}

// not read: not imported, and not in the subset
export type Status = "draft" | "published";
export interface Box<T> { value: T }
export function makeUser(id: number, name: string): User {
  return { id, name, admin: false, tags: [] };
}
