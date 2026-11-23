// TS code using tests/ts_import.zsc's output with its own types (tests/examples.rs: ts_records runs tsc on it):
// the .d.ts takes and returns model.ts's types, not copies of them
import { display, summary, promote, reply, same_user, dist, first_reply, publish, area, turn, label, tile, item_of, par, sur_first } from "./ts_import.js";
import { makeUser, type Doc, type Point, type Post, type Shape, type Status, type Tile, type User, type Shelf, type Sur } from "./ts/model.js";

const u: User = makeUser(7, "ana");
const p: Post = { author: u, title: "Hi", scores: [1, 2, 4.5], replies: [] };
const s: string = display(u) + summary(reply(p, u, "ok"));
const boss: User = promote(u);
const same: boolean = same_user(u, boss);
const pt: Point = { x: 3, y: 4 };
const d: number = dist(pt, { x: 0, y: 0 });
// a Reply from zsc fits model.ts's own (unexported) Reply: its arrays are as mutable as model.ts writes them
const withReply: Post = { ...p, replies: [first_reply(reply(p, u, "ok"))] };
// unions read as enums: TS's own values in and out
const doc: Doc = { title: "D", status: "draft", shapes: [{ kind: "circle", r: 1 }] };
const live: Doc = publish(doc);
const st: Status = live.status;
const sh: Shape = turn({ kind: "rect", w: 1, h: 2 });
const a: number = area(sh) + tile({ kind: "empty" });
const l: string = label("published");
// an unexported union with a named member: declared in the .d.ts with the member's object
const shelf: Shelf = { item: item_of({ item: { kind: "crate", n: 1 } }) };
const pq: "p" | "q" = par("q");
const sf: Sur = sur_first();
// @ts-expect-error: not a member of the union
label("archived");
// @ts-expect-error: a User needs every field of model.ts's User
display({ id: 1, name: "bo" });
