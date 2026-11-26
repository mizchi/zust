"""Bundles zustc's own crates/minclif/src/lib.rs (the minimal backend in place of Cranelift) and the crates/ir/src/lib.rs it uses into a single-file program.
usage: python3 tests/selfhost/build_minclif.py > out/selfhost/minclif_main.rs
main builds the same fns as tests/minclif.rs (loops, recursion, variables, stack slots, 8 arguments, floating point, i128, overflow,
bit operations, indirect calls, data), writes ELF .o files to the 2 paths in its arguments, and prints the expected output of the 2nd."""
src = open("crates/minclif/src/lib.rs").read().replace("zust_ir::", "crate::ir::")
ir = open("crates/ir/src/lib.rs").read()
test = open("tests/minclif.rs").read()
a = test.index("/// sum(n)")
b = test.index("/// main: prints the results with printf")
fns = test[a:b].replace("zustc::minclif::", "minclif::")
# The fns for floating point, i128, overflow, bit operations, indirect calls and data
a2 = test.index("/// 浮動小数点・i128・オーバーフロー")
b2 = test.index("#[test]\nfn ext_ops()")
fns += test[a2:b2].replace("zustc::minclif::", "minclif::")
print("// 自動生成: tests/selfhost/build_minclif.py (zustc の crates/ir/src/lib.rs と crates/minclif/src/lib.rs をそのまま含む)\nmod ir {\n" + ir + "}\nmod minclif {\n" + src + "}\n")
print("use minclif::{FunctionBuilder, IntCC, Type, write_object};\n")
print(fns)
print('''fn main() {
    let args: Vec<String> = std::env::args().collect();
    let funcs = vec![sum(), fib(), misc()];
    let obj = write_object(&funcs);
    std::fs::write(&args[1], &obj).unwrap();
    // 2 つ目: ext の関数 (+ sum) とデータ。期待する出力を標準出力に
    let (mut f2, datas, want) = ext();
    f2.push(sum());
    let obj2 = minclif::write_object_full(&f2, &datas);
    std::fs::write(&args[2], &obj2).unwrap();
    for w in want {
        println!("{w}");
    }
}
''')
