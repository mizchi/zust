// (the stand-in's types: only what the tests' TS consumers use)
export type ReactElement = { type: unknown; props: unknown; key: unknown };
export declare function useState<T>(init: T): [T, (v: T) => void];
