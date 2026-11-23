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

// unions, read as enums: of string literals (the strings are the values), and of objects with a tag field
// (each variant's fields by name); a member may be an object type of the file (`Square`)
export type Status = "draft" | "published";
export type Shape =
  | { kind: "circle"; r: number }
  | { kind: "rect"; w: number; h: number };
export interface Doc {
  title: string;
  status: Status;
  shapes: Shape[];
}
interface Square { kind: "square"; side: number }
export type Tile = Square | { kind: "empty" };

// a tag field and no data: objects, compared by content
export type Flag = { kind: "on" } | { kind: "off" };
// a union the file does not export, with a named member: the .d.ts writes it with the member's object
interface Crate { kind: "crate"; n: number }
type Item = Crate | { kind: "nothing" };
export interface Shelf { item: Item }
// an escaped tag ("\u0064one" is "done"), a union in parentheses, and one exported by a list
export type Esc = "\u0064one" | "todo";
export type Par = ("p" | "q");
type Loc = "l1" | "l2";
export type { Loc };
// (a surrogate pair written as two escapes is one character; a line continuation adds none)
export type Sur = "\uD835\uDC9Ca" | "b\
c";
// (a re-export names another module's type: the local `Hid` stays unexported)
type Hid = "h";
export type { Hid } from "./cr.js";

// not read: not imported, and not in the subset
export interface Box<T> { value: T }
export function makeUser(id: number, name: string): User {
  return { id, name, admin: false, tags: [] };
}
