// A TS module using zsc's output: the .d.ts must type-check, and the numeric brands must keep i32 and f64 apart.
// (tests/examples.rs runs tsc on it when tsc is available)
import { sum$i32, sum$f64, i32, f64, mean, area, visible, type Todo, type Shape } from "./basics.js";
import { App, Row, stats, map_sum, histogram, type Sample } from "./app.js";
import type { ReactElement } from "react";

const a: i32 = sum$i32([i32(1), i32(2)]);
const asNumber: number = a; // a brand is still a number
const m: f64 = mean([f64(1.5), f64(2)]);
const t: Todo = { id: i32(1), title: "x", done: false };
const active: readonly Todo[] = visible([t], "Active");
const s: Shape = { TAG: "Circle", _0: { x: f64(0), y: f64(0) }, _1: f64(1) };
const r: f64 = area(s);
const total: f64 = sum$f64([m, r]);

const sample: Sample = { label: "a", values: [f64(1), f64(2.5)] };
const st = stats(sample);
const sq: f64 = map_sum(sample.values, (x) => f64(x * x));
const h: readonly i32[] = histogram(sample.values, i32(4));
const el: ReactElement = App({ samples: [sample] });
const row: ReactElement = Row({ sample, onPick: (l: string) => console.log(l) });

// @ts-expect-error: an f64 is not an i32
sum$i32([f64(1.5)]);
// @ts-expect-error: a plain number is not an i32 (make one with i32(..))
sum$i32([1, 2]);
// @ts-expect-error: an i32 is not an f64
const bad: f64 = a;
// @ts-expect-error: "Gone" is not a Filter
visible([t], "Gone");
// @ts-expect-error: a Todo's fields are readonly
t.done = true;
// @ts-expect-error: the array is readonly
active.push(t);

export { asNumber, total, st, sq, h, el, row, bad };
