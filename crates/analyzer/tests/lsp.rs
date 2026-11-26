//! Actually starts `zust-analyzer` and checks that diagnostics come back over stdio.

use std::io::{BufReader, Write};
use std::process::{Command, Stdio};

use zust_analyzer::{read_msg, write_msg};
use serde_json::{Value, json};

#[test]
fn diagnostics_roundtrip() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_zust-analyzer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |v: Value| write_msg(&mut stdin, &v.to_string()).unwrap();
    let uri = "file:///t.rs";

    send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let r: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["capabilities"]["textDocumentSync"], 2);
    send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));

    // Open a version with a type error
    let bad = "fn f() -> u8 {\n    1i64\n}\nfn main() {}\n";
    send(json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": bad}}}));
    let d: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(d["method"], "textDocument/publishDiagnostics");
    let diags = d["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1, "{d}");
    assert_eq!(diags[0]["range"]["start"]["line"], 1);
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("mismatched types")
    );

    // Fixing it makes the diagnostic go away
    let good = "fn f() -> u8 {\n    1\n}\nfn main() {}\n";
    send(json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
        "params": {"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": good}]}}));
    let d: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(d["params"]["version"], 2);
    assert_eq!(d["params"]["diagnostics"].as_array().unwrap().len(), 0);

    send(json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}));
    let r: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(r["id"], 2);
    send(json!({"jsonrpc": "2.0", "method": "exit"}));
    stdin.flush().ok();
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// didChange with ranges (incremental sync): each change edits the text the one before it left, at UTF-16 columns;
/// one without a range is the full text
#[test]
fn incremental_changes() {
    use zust_analyzer::Server;
    let mut srv = Server::new();
    let uri = "file:///inc.rs";
    let diags = |r: Vec<String>| -> Vec<Value> {
        let d: Value = serde_json::from_str(&r[0]).unwrap();
        assert_eq!(d["method"], "textDocument/publishDiagnostics");
        d["params"]["diagnostics"].as_array().unwrap().clone()
    };
    let change = |srv: &mut Server, v: u32, changes: Value| {
        srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {"textDocument": {"uri": uri, "version": v}, "contentChanges": changes}}).to_string())
    };
    let range = |l0: u32, c0: u32, l1: u32, c1: u32| json!({"start": {"line": l0, "character": c0}, "end": {"line": l1, "character": c1}});
    let names = |srv: &mut Server| -> Vec<String> {
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 9, "method": "textDocument/documentSymbol", "params": {"textDocument": {"uri": uri}}}).to_string());
        let r: Value = serde_json::from_str(&r[0]).unwrap();
        r["result"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap().to_string()).collect()
    };
    // (`𝄞` is two UTF-16 units and four bytes, `é` one and two: `i64` starts at column 21)
    let text = "fn f() -> u8 {\n    let _s = \"𝄞é\"; 1i64\n}\nfn main() {}\n";
    let r = srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());
    assert_eq!(diags(r).len(), 1);
    // Deleting `i64` fixes the type error
    let r = change(&mut srv, 2, json!([{"range": range(1, 21, 1, 24), "text": ""}]));
    assert_eq!(diags(r).len(), 0, "{}", names(&mut srv).join(","));
    // Two changes in one message: the second's positions are in the text after the first
    let r = change(&mut srv, 3, json!([
        {"range": range(0, 0, 0, 0), "text": "fn added_a() {}\n"},
        {"range": range(5, 0, 5, 0), "text": "fn added_b() {}\n"},
    ]));
    assert_eq!(diags(r).len(), 0);
    assert_eq!(names(&mut srv), ["added_a", "f", "main", "added_b"]);
    // A replacement across lines, and an end past the end of its line (the line's end)
    let r = change(&mut srv, 4, json!([{"range": range(1, 3, 3, 99), "text": "g() {}"}]));
    assert_eq!(diags(r).len(), 0);
    assert_eq!(names(&mut srv), ["added_a", "g", "main", "added_b"]);
    // The full text, then a range in it
    let r = change(&mut srv, 5, json!([{"text": "fn main() {}\n"}, {"range": range(0, 0, 0, 0), "text": "fn h() -> u8 { 1i64 }\n"}]));
    assert_eq!(diags(r).len(), 1);
    assert_eq!(names(&mut srv), ["h", "main"]);
}

/// documentSymbol (the tree of an open document) and workspace/symbol (search by name in the open documents and the workspace's .rs files)
#[test]
fn symbols() {
    use zust_analyzer::Server;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lsp_symbols");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("src/geom.rs"), "pub struct Point { pub x: f64, pub y: f64 }\nimpl Point {\n    pub fn norm(&self) -> f64 { 0.0 }\n}\n").unwrap();
    // (nothing under target is looked at)
    std::fs::write(root.join("target/junk.rs"), "fn point_junk() {}\n").unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"]["capabilities"]["workspaceSymbolProvider"], true);
    assert_eq!(r["result"]["capabilities"]["documentSymbolProvider"], true);

    let uri = format!("{uri_root}/src/main.rs");
    let text = "mod util {\n    pub fn point_dist() -> u32 { 1 }\n}\nenum Shape { Circle(f64), Rect { w: f64 } }\nfn main() {}\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());

    // documentSymbol: the tree and positions (lines and columns are 0-based)
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol", "params": {"textDocument": {"uri": uri}}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    let syms = r["result"].as_array().unwrap();
    let names: Vec<&str> = syms.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["util", "Shape", "main"]);
    assert_eq!(syms[0]["kind"], 2);
    assert_eq!(syms[0]["children"][0]["name"], "point_dist");
    assert_eq!(syms[0]["children"][0]["selectionRange"]["start"], json!({"line": 1, "character": 11}));
    let vars: Vec<&str> = syms[1]["children"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(vars, ["Circle", "Rect"]);
    assert_eq!(syms[1]["children"][1]["children"][0]["name"], "w");

    // workspace/symbol (same rules as rust-analyzer): from the open document and from src/geom.rs, which is not open.
    // When a type matches, only types (`point` → struct Point; the fn point_dist and impls are not shown)
    let ask = |srv: &mut Server, id: u32, q: &str| -> Value {
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": id, "method": "workspace/symbol", "params": {"query": q}}).to_string());
        serde_json::from_str(&r[0]).unwrap()
    };
    let r = ask(&mut srv, 3, "point");
    let hits: Vec<(String, String)> = r["result"].as_array().unwrap().iter()
        .map(|s| (s["name"].as_str().unwrap().to_string(), s["location"]["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_string()))
        .collect();
    assert_eq!(hits, vec![("Point".to_string(), "geom.rs".to_string())], "{hits:?}");
    // When no type matches, from everything (fields are not shown)
    let r = ask(&mut srv, 31, "point_d");
    let pd = r["result"].as_array().unwrap().iter().find(|s| s["name"] == "point_dist").unwrap();
    assert_eq!(pd["containerName"], "util");
    assert_eq!(pd["kind"], 12);
    assert!(!r["result"].as_array().unwrap().iter().any(|h| h["name"] == "point_junk"), "{r}");
    // A query containing uppercase letters is case-sensitive
    assert_eq!(ask(&mut srv, 32, "POint")["result"].as_array().unwrap().len(), 0);

    // Those containing the characters in order (`nrm` → norm; the containerName of an impl member is the owning type), and rereading after the file is rewritten
    let r = ask(&mut srv, 4, "nrm");
    assert_eq!(r["result"][0]["name"], "norm");
    assert_eq!(r["result"][0]["containerName"], "Point");
    std::fs::write(root.join("src/geom.rs"), "pub struct Point2 {}\n// changed file\n").unwrap();
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "workspace/symbol", "params": {"query": "nrm"}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"].as_array().unwrap().len(), 0, "{r}");

    // The symbols of the edited text
    let text2 = "fn renamed_fn() {}\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
        "params": {"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": text2}]}}).to_string());
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 6, "method": "workspace/symbol", "params": {"query": "renamed"}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"][0]["name"], "renamed_fn");
}

/// definition / references: locals, methods (from the receiver's type), `use`d items, references across files
#[test]
fn definition_and_references() {
    nav(usize::MAX, "lsp_nav");
}

/// The same, with all the workspace's files made slim (the text thrown away and reread for references)
#[test]
fn definition_and_references_slim() {
    nav(0, "lsp_nav_slim");
}

fn nav(budget: usize, dir: &str) {
    use zust_analyzer::Server;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(dir);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/geom.rs"), "pub struct Point { pub x: f64 }\nimpl Point {\n    pub fn new() -> Point { Point { x: 0.0 } }\n    pub fn norm(&self) -> f64 { self.x }\n}\npub fn norm() {}\n").unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    srv.set_mem_budget(budget);
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"]["capabilities"]["definitionProvider"], true);
    assert_eq!(r["result"]["capabilities"]["referencesProvider"], true);
    let uri = format!("{uri_root}/src/main.rs");
    let text = "mod geom;\nuse geom::Point;\nfn main() {\n    let p = Point::new();\n    let n = p.norm();\n    helper(n);\n}\nfn helper(x: f64) -> f64 { x }\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());
    let mut id = 10;
    let mut ask = |method: &str, line: u32, ch: u32| -> Vec<(String, u64, u64)> {
        id += 1;
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"textDocument": {"uri": uri},
            "position": {"line": line, "character": ch}, "context": {"includeDeclaration": true}}}).to_string());
        let r: Value = serde_json::from_str(&r[0]).unwrap();
        let v = match &r["result"] {
            Value::Array(a) => a.clone(),
            Value::Null => vec![],
            o => vec![o.clone()],
        };
        let mut out: Vec<(String, u64, u64)> = v.iter()
            .map(|l| (l["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_string(), l["range"]["start"]["line"].as_u64().unwrap(), l["range"]["start"]["character"].as_u64().unwrap()))
            .collect();
        out.sort();
        out
    };
    let at = |f: &str, l: u64, c: u64| (f.to_string(), l, c);
    // The Point of `Point::new()` → the struct in geom.rs, new → the impl's method
    assert_eq!(ask("textDocument/definition", 3, 12), [at("geom.rs", 0, 11)]);
    assert_eq!(ask("textDocument/definition", 3, 19), [at("geom.rs", 2, 11)]);
    // `p.norm()`: the method of p's type (Point), not the fn of the same name geom::norm
    assert_eq!(ask("textDocument/definition", 4, 14), [at("geom.rs", 3, 11)]);
    // Locals
    assert_eq!(ask("textDocument/definition", 5, 11), [at("main.rs", 4, 8)]);
    assert_eq!(ask("textDocument/definition", 7, 28), [at("main.rs", 7, 10)]);
    // References: helper is the declaration and the call; Point across files (use, path, impl, return type, struct expression)
    assert_eq!(ask("textDocument/references", 7, 4), [at("main.rs", 5, 4), at("main.rs", 7, 3)]);
    let refs = ask("textDocument/references", 0, 12);
    assert!(refs.is_empty(), "{refs:?}"); // (the geom of `mod geom` is a module: no references are returned)
    let refs = ask("textDocument/references", 1, 11);
    assert_eq!(refs, [at("geom.rs", 0, 11), at("geom.rs", 1, 5), at("geom.rs", 2, 20), at("geom.rs", 2, 28), at("main.rs", 1, 10), at("main.rs", 3, 12)]);
}

/// hover and completion: signatures, doc comments, local types, `x.|` (members of the receiver's type), `Q::|`, the middle of a name
#[test]
fn hover_and_completion() {
    use zust_analyzer::Server;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lsp_hover");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/geom.rs"), "/// 平面の点\npub struct Point { pub x: f64, pub y: f64 }\nimpl Point {\n    /// 原点\n    pub fn origin() -> Point { Point { x: 0.0, y: 0.0 } }\n    pub fn norm(&self) -> f64 { self.x }\n    pub fn shifted(&self) -> Option<Point> { None }\n}\n").unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"]["capabilities"]["hoverProvider"], true);
    assert!(r["result"]["capabilities"]["completionProvider"].is_object());
    let uri = format!("{uri_root}/src/main.rs");
    let text = "mod geom;\nuse geom::Point;\nfn main() {\n    let p = Point::origin();\n    let q = p.shifted().unwrap();\n    let n = q.norm();\n    let v: Vec<Point> = vec![];\n    v.\n    Point::\n    no\n}\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());
    let mut id = 10;
    let mut ask = |method: &str, line: u32, ch: u32| -> Value {
        id += 1;
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"textDocument": {"uri": uri},
            "position": {"line": line, "character": ch}}}).to_string());
        let r: Value = serde_json::from_str(&r[0]).unwrap();
        r["result"].clone()
    };
    let hover = |v: Value| v["contents"]["value"].as_str().unwrap_or("").to_string();
    let labels = |v: Value| -> Vec<String> { v["items"].as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap().to_string()).collect() };
    // The definition's signature and doc comment
    let h = hover(ask("textDocument/hover", 3, 20));
    assert!(h.contains("pub fn origin() -> Point") && h.contains("原点") && h.contains("impl Point"), "{h}");
    let h = hover(ask("textDocument/hover", 1, 11));
    assert!(h.contains("pub struct Point") && h.contains("平面の点"), "{h}");
    // A local's type: Option<Point>::unwrap → Point
    assert!(hover(ask("textDocument/hover", 4, 8)).contains("q: Point"));
    assert!(hover(ask("textDocument/hover", 5, 8)).contains("n: f64"));
    // q.norm() is Point::norm
    assert!(hover(ask("textDocument/hover", 5, 15)).contains("pub fn norm(&self) -> f64"));
    // `p.|`: Point's members; `v.|`: methods of Vec (and slices)
    let c = labels(ask("textDocument/completion", 4, 14));
    assert!(c.contains(&"shifted".to_string()) && c.contains(&"x".to_string()) && !c.contains(&"push".to_string()), "{c:?}");
    let c = labels(ask("textDocument/completion", 7, 6));
    if std::env::var_os("ZUST_ANALYZER_NO_STD").is_none() && c.iter().any(|x| x == "len") {
        assert!(c.contains(&"push".to_string()) && c.contains(&"iter".to_string()), "{c:?}");
    }
    // `Point::|`: associated fns and what is not a field
    let c = labels(ask("textDocument/completion", 8, 11));
    assert!(c.contains(&"origin".to_string()), "{c:?}");
    // The middle of a name `no|`: not the local n, but those starting with / containing n (norm is a member of Point, so it does not appear)
    let c = labels(ask("textDocument/completion", 9, 6));
    assert!(!c.is_empty() && c.iter().all(|x| x.to_lowercase().contains('n')), "{c:?}");
}

/// rename (prepareRename, rewriting other files too, refusals with a reason), signatureHelp, documentHighlight
#[test]
fn rename_signature_highlight() {
    use zust_analyzer::Server;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lsp_rename");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/geom.rs"), "pub struct Point { pub x: f64 }\nimpl Point {\n    pub fn scale(&self, k: f64, d: f64) -> f64 { self.x * k + d }\n}\n").unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    let caps = &r["result"]["capabilities"];
    assert_eq!(caps["renameProvider"]["prepareProvider"], true);
    assert_eq!(caps["documentHighlightProvider"], true);
    assert!(caps["signatureHelpProvider"]["triggerCharacters"].as_array().unwrap().contains(&json!("(")));
    let uri = format!("{uri_root}/src/main.rs");
    let text = "mod geom;\nuse geom::Point;\nfn main() {\n    let p = Point { x: 1.0 };\n    let s = p.scale(2.0, p.x);\n}\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());
    let mut id = 10;
    let mut ask = |method: &str, line: u32, ch: u32, extra: Value| -> Value {
        id += 1;
        let mut p = json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": ch}});
        if let Value::Object(m) = extra {
            p.as_object_mut().unwrap().extend(m);
        }
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": p}).to_string());
        serde_json::from_str(&r[0]).unwrap()
    };
    // prepareRename: the name's range and the current name
    let r = ask("textDocument/prepareRename", 4, 15, json!({}));
    assert_eq!(r["result"]["placeholder"], "scale", "{r}");
    assert_eq!(r["result"]["range"]["start"]["character"], 14);
    // The field x: the definition and self.x in geom.rs, and `Point { x: .. }` and `p.x` in main.rs
    let r = ask("textDocument/rename", 4, 27, json!({"newName": "px"}));
    let ch = &r["result"]["changes"];
    let geom = ch.as_object().unwrap().iter().find(|(k, _)| k.ends_with("geom.rs")).map(|(_, v)| v.as_array().unwrap().len());
    assert_eq!(geom, Some(2), "{r}");
    assert_eq!(ch[&uri].as_array().unwrap().len(), 2, "{r}");
    assert!(ch[&uri].as_array().unwrap().iter().all(|e| e["newText"] == "px"));
    // When refusing: an error with a reason
    let r = ask("textDocument/rename", 4, 27, json!({"newName": "fn"}));
    assert_eq!(r["error"]["code"], -32803, "{r}");
    let r = ask("textDocument/prepareRename", 3, 22, json!({}));
    assert!(r["error"]["message"].as_str().is_some_and(|m| !m.is_empty()), "{r}");
    // signatureHelp: `p.scale(2.0, |p.x)` is the second (self is not counted)
    let r = ask("textDocument/signatureHelp", 4, 25, json!({}));
    let sg = &r["result"]["signatures"][0];
    assert_eq!(sg["label"], "pub fn scale(&self, k: f64, d: f64) -> f64", "{r}");
    assert_eq!(sg["parameters"].as_array().unwrap().len(), 2);
    assert_eq!(r["result"]["activeParameter"], 1);
    let l = sg["label"].as_str().unwrap();
    let p0 = &sg["parameters"][0]["label"];
    assert_eq!(&l[p0[0].as_u64().unwrap() as usize..p0[1].as_u64().unwrap() as usize], "k: f64");
    assert!(ask("textDocument/signatureHelp", 3, 10, json!({}))["result"].is_null());
    // documentHighlight: p in this file (the definition and the 2 uses)
    let r = ask("textDocument/documentHighlight", 3, 8, json!({}));
    assert_eq!(r["result"].as_array().unwrap().len(), 3, "{r}");
}

/// Compares type inference (the types in hover) with rust-analyzer's answers: iterator elements, type arguments from closure return values, associated type projections,
/// tuples, turbofish, methods of number types (inside core's macros). Needs std, so does nothing if it is not found
#[test]
fn type_inference_like_rust_analyzer() {
    use zust_analyzer::Server;
    if std::env::var_os("ZUST_ANALYZER_NO_STD").is_some() || std::process::Command::new("rustc").args(["--print", "sysroot"]).output().map(|o| !o.status.success()).unwrap_or(true) {
        return;
    }
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lsp_types");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"ty\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let text = "pub struct D { pub name: String, pub n: u8 }\npub fn f(ws: &[&D], uses: Vec<(String, Vec<String>)>, v: Vec<D>, o: Option<D>, r: Result<D, String>) {\n    let a = uses.iter().find(|u| u.0 == \"x\");\n    let b = a.map(|u| u.1[0].as_str());\n    let c = ws.get(0);\n    let d = c.and_then(|w| w.name.chars().next());\n    let e = v.iter().enumerate().filter(|(i, x)| x.n > 0 && *i > 0).count();\n    let g = v.iter().map(|x| x.n).max();\n    let h = 3u8.wrapping_sub(1);\n    let k = v.first();\n    let l = uses[0].0.len();\n    let m = o.as_ref().map(|x| &x.name);\n    let p = v.iter().rev().next();\n    let q: Vec<u8> = v.iter().map(|x| x.n).collect();\n    let s = r.as_ref().ok();\n    let t = v.len().checked_sub(1);\n    let w = uses.iter().map(|u| &u.1).flatten().count();\n    let y = v.iter().filter_map(|x| x.name.chars().next()).last();\n    let z = k.map(|x| x.n.count_ones());\n}\npub fn g(m: std::collections::HashMap<String, Vec<u8>>, s: &str, v: Vec<D>) -> Option<u8> {\n    let a1 = m.iter().map(|(k, x)| (k.len(), x.len())).next();\n    let a2 = m.get(\"x\").map(|x| x.len());\n    let a3 = s.split(',').map(|p| p.trim()).collect::<Vec<_>>();\n    let a4 = s.chars().rev().enumerate().last();\n    let a5 = v.iter().map(|x| x.n as u32).sum::<u32>();\n    let a6 = m.values().flatten().copied().max();\n    let a7 = s.parse::<u8>().ok();\n    let a8 = v.iter().position(|x| x.n == 0);\n    let a9 = m.keys().cloned().collect::<Vec<String>>();\n    let b1 = s.lines().next().unwrap_or(\"\");\n    let b2 = v.iter().find_map(|x| x.name.find('a'));\n    let b3 = 1u32.saturating_add(2).leading_zeros();\n    let b4 = (1i64).abs().pow(2);\n    let b5 = v.iter().zip(s.bytes()).map(|(x, c)| x.n + c).max();\n    let b6 = m.into_iter().filter(|(k, _)| !k.is_empty()).map(|(_, x)| x).next();\n    let c1 = m.iter();\n    let c2 = m.iter().map(|(k, x)| k.len());\n    let c3 = m.iter().map(|(k, x)| (k.len(), 1u8));\n    let c4 = m.iter().map(|e| e.0.len());\n    let c5 = m.iter().map(|(k, x)| x.len());\n    let c6 = m.iter().map(|(aa, bb)| bb.len());\n    let c7 = m.iter().map(|(aa, bb)| bb);\n    None\n}\n";
    std::fs::write(root.join("src/lib.rs"), text).unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let uri = format!("{uri_root}/src/lib.rs");
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());
    // (rust-analyzer's hover types)
    let want: &[(&str, &str)] = &[
        ("a", "Option<&(String, Vec<String>)>"),
        ("b", "Option<&str>"),
        ("c", "Option<&&D>"),
        ("d", "Option<char>"),
        ("e", "usize"),
        ("g", "Option<u8>"),
        ("h", "u8"),
        ("k", "Option<&D>"),
        ("l", "usize"),
        ("m", "Option<&String>"),
        ("p", "Option<&D>"),
        ("q", "Vec<u8>"),
        ("s", "Option<&D>"),
        ("t", "Option<usize>"),
        ("w", "usize"),
        ("y", "Option<char>"),
        ("z", "Option<u32>"),
        ("a1", "Option<(usize, usize)>"),
        ("a2", "Option<usize>"),
        ("a3", "Vec<&str>"),
        ("a4", "Option<(usize, char)>"),
        ("a5", "u32"),
        ("a6", "Option<u8>"),
        ("a7", "Option<u8>"),
        ("a8", "Option<usize>"),
        ("a9", "Vec<String>"),
        ("b1", "&str"),
        ("b2", "Option<usize>"),
        ("b3", "u32"),
        ("b4", "i64"),
        ("b5", "Option<u8>"),
        ("b6", "Option<Vec<u8>>"),
        ("c1", "Iter<'_, String, Vec<u8>>"),
        ("c2", "Map<Iter<'_, String, Vec<u8>>, impl FnMut((&String, &Vec<u8>)) -> usize>"),
        ("c3", "Map<Iter<'_, String, Vec<u8>>, impl FnMut((&String, &Vec<u8>)) -> (usize, u8)>"),
        ("c4", "Map<Iter<'_, String, Vec<u8>>, impl FnMut((&String, &Vec<u8>)) -> usize>"),
        ("c5", "Map<Iter<'_, String, Vec<u8>>, impl FnMut((&String, &Vec<u8>)) -> usize>"),
        ("c6", "Map<Iter<'_, String, Vec<u8>>, impl FnMut((&String, &Vec<u8>)) -> usize>"),
        ("c7", "Map<Iter<'_, String, Vec<u8>>, impl FnMut((&String, &Vec<u8>)) -> &Vec<u8>>")
    ];
    let mut id = 10;
    let mut bad = vec![];
    for (name, ty) in want {
        let (li, l) = text.lines().enumerate().find(|(_, l)| l.trim_start().starts_with(&format!("let {name} ")) || l.trim_start().starts_with(&format!("let {name}:"))).unwrap();
        let ch = l.find(&format!("let {name}")).unwrap() + 4;
        id += 1;
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": id, "method": "textDocument/hover", "params": {"textDocument": {"uri": uri}, "position": {"line": li, "character": ch}}}).to_string());
        let r: Value = serde_json::from_str(&r[0]).unwrap();
        let h = r["result"]["contents"]["value"].as_str().unwrap_or("").to_string();
        if !h.contains(&format!("{name}: {ty}")) {
            bad.push(format!("{name}: want {ty}, got {h:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
