# Self-hosting stage 18: moving codegen.rs to our own syntax tree and minclif (acg / driver2)

Date: 2026-11-16

- `src/acg/`: codegen.rs (syn + Cranelift, 12,500 lines) moved as-is, method by method, onto our own syntax tree (parse.rs) and minclif.
  `clif.rs` builds an API with the same shape as Cranelift's (`b.ins().iadd(..)`, etc., with dispatch for i128 / floating point) on top of minclif, so
  the code that builds IR moved over almost unchanged. `mod.rs` holds the Cx definition and the core (memory, variables, function bodies, blocks); `p1` to `p8` hold the rest
  (expressions / calls / operators / methods / loops and macros / match and drop / formatting, hashing, comparison / other)
- `src/driver2.rs`: a driver that uses neither syn nor Cranelift. lexer / macros / prelude → ast_summary → Program → acg → minclif ELF .o → link.
  The instance fixed point, closures, statics, vtables, folding of instances with identical machine code, and the panic location table are the same as in driver.rs
- Enabled with `--features acg`. `ZUSTC_DRIVER2=1 zustc x.rs` uses this path. `tests/driver2.rs` compares probe / examples / acg against the current driver, and
  `tests/selfhost.sh` (at the time `SELFHOST_DRIVER2=1`; now always this path) builds every self-hosting stage (lexer to program, minclif, enc) with driver2 and compares with the rustc version (all match)
- What we learned while moving:
  - in minclif you could not go back to a block later and continue writing it (Cranelift allows this). It now places a jmp to the continuation when leaving
  - i128 arguments and return values use two registers (System V). i128 variables and block parameters are also mapped as 16 bytes
  - code generation duplicates and rewrites expressions, so type records looked up by address did not hit. The syn version looked them up by source position.
    The macro argument syntax trees read by type inference are now passed to code generation as-is, and a further "expression shape (Debug hash) → type" table is used for lookup
  - addresses of temporary expressions discarded by type inference were reused for new expressions in code generation, which then got the wrong type. All of inference's syntax trees are now kept until the end
  - closures with identical text (`|x| x.1.clone()` in 2 places) got the same number. Closures in bodies are now looked up by address
  - also fixed mistakes in our own parser (`&&x` patterns, `true` / `false` in patterns) and a type inference mistake for `clone()` on integers (also present in the syn version)
