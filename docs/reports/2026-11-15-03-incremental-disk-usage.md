# Disk usage of incremental builds

Date: 2026-11-15

- The cache (`.crcache`) does not hold per-function machine code. The machine code is in the output `.o`, so the cache only stores (length, hash),
  and the next build walks the symtab of the previous `.o` to take out the same machine code (if the hash does not match, or there is no `.o`, it recompiles).
  Only functions that are called from nowhere and were not placed in the `.o` are written to the cache together with their machine code.
- The `.text` of the `.o` that is kept is an ELF standard compressed section (`SHF_COMPRESSED`, zstd level 1) (`src/elfz.rs`).
  - wild decompresses `SHF_COMPRESSED` in any input section when reading, so the compressed `.o` is passed as-is.
  - ld.lld 18 crashes on compressed `SHF_ALLOC` sections, and ld.bfd fails with "reloc against ...: error 4" (in the gABI, compression is for non-ALLOC sections).
    For these two, an uncompressed `.o` is passed as a temporary file (`<out>.tmp.o`, deleted after linking).
- The `.o` combines everything into one `.text`. Which functions to place is decided by following relocations and vtables from main / exports (instead of `--gc-sections`).

Changes on gen1000 (310 KB of source):

| | Initial | After removing machine code from the cache | After compressing `.text` | After packing the index and the panic table |
|---|---:|---:|---:|---:|
| `.crcache` | 642 KiB | 116 KiB | 116 KiB | **38 KiB** |
| `.o` | 757 KiB | 631 KiB | 153 KiB | **67 KiB** |
| Total | 1399 KiB | 747 KiB | 270 KiB | **105 KiB** (the rustc incremental directory is 22156 KiB) |
| Binary | 553 KiB | 553 KiB | 553 KiB | 519 KiB |

- The cache file compresses its body with zstd, and writes keys as the low 64 bits and lengths as variable-length integers (an index entry goes from 20 bytes to about 9 bytes. Compression also shrinks the item summaries and panic locations to 1/3).
- The panic table became a byte sequence of variable-length integers, and it refers only to the start of `.text` (function positions are counted by the builder itself when making the `.o`, and checked against the symtab).
  Before, it was 24 bytes of relocation + 16 bytes of header per function, and 8 bytes per location. The table is also in the binary, so the binary also became 6% smaller.
  The handler (`__exr_on_trap`) reads the sequence from the front on panic.
- The `.o` compresses not only `.text` but also the PROGBITS loaded into memory (the panic table, strings) (wild decompresses all of them when reading).

`python3 bench/run.py 1000` (with wild 0.10.0 on PATH):

| tool | cold ms | no-op ms | 1-function edit ms | peak RSS (cold) | binary | cache + .o |
|---|---:|---:|---:|---:|---:|---:|
| zustc (wild) | 124 | 17 | **28** | 19.5 MiB | 554 KiB | **270 KiB** |
| zustc (ld.lld, uncompressed temporary `.o`) | 151 | 18 | 41 | 59.0 MiB | 553 KiB | 270 KiB |

- Compression goes from 587 KB → 69 KB in 0.3 ms (`compress` in `--time`). Linking with wild, including decompression, takes 6–9 ms.
