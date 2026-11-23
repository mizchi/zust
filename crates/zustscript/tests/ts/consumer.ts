// TS code using tests/ts_import.zsc's output with its own types (tests/examples.rs: ts_records runs tsc on it):
// the .d.ts takes and returns model.ts's types, not copies of them
import { display, summary, promote, reply, same_user, dist } from "./ts_import.js";
import { makeUser, type Point, type Post, type User } from "./ts/model.js";

const u: User = makeUser(7, "ana");
const p: Post = { author: u, title: "Hi", scores: [1, 2, 4.5], replies: [] };
const s: string = display(u) + summary(reply(p, u, "ok"));
const boss: User = promote(u);
const same: boolean = same_user(u, boss);
const pt: Point = { x: 3, y: 4 };
const d: number = dist(pt, { x: 0, y: 0 });
// @ts-expect-error: a User needs every field of model.ts's User
display({ id: 1, name: "bo" });
