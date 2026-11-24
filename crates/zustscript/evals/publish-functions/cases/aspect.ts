export type Viewport =
  | { kind: "hidden" }
  | { kind: "rect"; height: number; width: number };
