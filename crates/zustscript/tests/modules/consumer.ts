// TS code that uses two zustscript modules: a type of shapes.zsc goes into a function of app.zsc
import { total, view } from "./app.js";
import { area, type Shape, type Labeled } from "./shapes.js";
import { f64 } from "./zsc-runtime.js";

const c: Shape = { TAG: "Circle", _0: { x: f64(0), y: f64(0) }, _1: f64(1) };
const sum: f64 = total([c]);
const one: f64 = area(c);
const l: Labeled = { label: "c", shape: c };
view(l);
console.log(sum, one);
