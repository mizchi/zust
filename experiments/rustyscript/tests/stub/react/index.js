// A stand-in for React in the tests: enough to render to a string (no reconciler, no effects)
export const useState = (init) => [init, (v) => { globalThis.__stateSets = (globalThis.__stateSets || []).concat([v]); }];
