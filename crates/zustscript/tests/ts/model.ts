// A TS module whose record types tests/ts_import.zsc reads with `import type` (tests/examples.rs: ts_records).
// Only the types the import needs are read: the rest of the file (functions, unions, generics) is skipped.

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

interface Reply {
  by: User;
  text: string;
}

export interface Point {
  x: number;
  y: number;
}

// not read: not imported, and not in the subset
export type Status = "draft" | "published";
export interface Box<T> { value: T }
export function makeUser(id: number, name: string): User {
  return { id, name, admin: false, tags: [] };
}
