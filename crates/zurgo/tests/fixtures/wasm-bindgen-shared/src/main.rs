// wasm-bindgen-shared (W0 of mizchi/zust#38): its build script and its API, without proc macros

fn main() {
    println!("schema {} {}", wasm_bindgen_shared::SCHEMA_VERSION, wasm_bindgen_shared::version().len() > 0);
    println!("{}", wasm_bindgen_shared::qualified_name(Some(&["a", "b"]), "c"));
    println!("{}", wasm_bindgen_shared::qualified_name(None::<&[&str]>, "only"));
    println!("{}", wasm_bindgen_shared::mangled_symbol("base", "h1"));
    println!("{}", wasm_bindgen_shared::module_mangled_symbol("base", "h1", "m::n"));
    println!("{}", wasm_bindgen_shared::module_path_hash("my_crate::m"));
    println!("{} {}", wasm_bindgen_shared::crate_name("foo-bar-abc123"), wasm_bindgen_shared::crate_hash("foo-bar-abc123"));
    println!("{} {} {}", wasm_bindgen_shared::new_function("Point"), wasm_bindgen_shared::free_function("Point"), wasm_bindgen_shared::unwrap_function("Point"));
    println!("{}", wasm_bindgen_shared::upcast_function("Child", "Parent"));
    println!("{} {}", wasm_bindgen_shared::free_function_export_name("plain_name"), wasm_bindgen_shared::free_function_export_name("[Symbol.iterator]"));
    println!("{}", wasm_bindgen_shared::struct_function_export_name("MyStruct", "a.b-c"));
    println!("{} {}", wasm_bindgen_shared::struct_field_get("S", "x"), wasm_bindgen_shared::struct_field_set("S", "y"));
    println!("{}", wasm_bindgen_shared::dynamic_union_variant("U", 3));
    println!("{}", wasm_bindgen_shared::escape_string("a\"b\\c\nd"));
    println!("{} {} {}", wasm_bindgen_shared::identifier::is_js_keyword("class"), wasm_bindgen_shared::identifier::is_non_value_js_keyword("typeof"), wasm_bindgen_shared::identifier::is_valid_ident("a1$"));
    println!("{} {}", wasm_bindgen_shared::identifier::to_valid_ident("9-lives"), wasm_bindgen_shared::identifier::to_valid_ident("ok_name"));
    println!("{} {} {}", wasm_bindgen_shared::tys::SEQ_MODE_VIEW, wasm_bindgen_shared::tys::SEQ_MODE_VEC, wasm_bindgen_shared::tys::SEQ_MODE_EXPLICIT);
}
