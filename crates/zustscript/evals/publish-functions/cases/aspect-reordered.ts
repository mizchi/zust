export type Viewport =
  | { kind: "hidden" }
  | { kind: "rect"; width: number; height: number };
