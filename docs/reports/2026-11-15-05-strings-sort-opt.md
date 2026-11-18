# Making strings and sort faster with `-O`

Date: 2026-11-15

| | Before | After | rustc -O |
|---|---:|---:|---:|
| strings `-O` | 141 ms | **73 ms** | 34 ms |
| sort `-O` | 550 ms | **195 ms** | 190 ms |
| strings (debug) | 259 ms | 113 ms | (rustc debug 121 ms) |
| sort (debug) | 725 ms | 252 ms | (rustc debug 1983 ms) |

- Formatting (callgrind showed integer formatting at 41%):
  - For integers in `{}` (decimal, no width or sign specifier), fill the digits from the back and append them all at once (no more temporary String and per-character push; division by 10 becomes a multiplication)
  - The String of `format!` reserves the length of the fixed parts + 8 bytes per argument up front
  - `Vec::truncate` only changes the length if the elements need no drop (added `core::mem::needs_drop`). `extend_from_slice` is a memcpy
  - Arrays filled with an integer constant (`[0u8; 24]`) are written 8 bytes at a time, and copies of aggregates with small alignment are also 8 bytes at a time (before, 1 byte at a time)
  - `split`'s next copies the fields into locals before looping
- sort:
  - `sort` on integers of 64 bits or less, and `sort_by_key` with an integer key, use a stable LSD radix sort (digits whose byte is the same in all elements are skipped).
    sort_by_key calls the key function only once per element (std calls it per comparison, so the number of calls differs from std).
    (I also tried computing the keys once and then sorting indices or (key, index) pairs, but the random reads and writes of the permutation made it slower)
  - Per-type branches in the prelude (`if __exr_int_kind::<T>() == 0` / `needs_drop::<T>()`) are decided by the type, so the unused side is not compiled (and no instances are created)
- Size: programs that sort integers get the radix sort added (sorting's binary +42%, ordering's cache +49%). Programs that use formatting also grow a little. The budgets were re-recorded.
- What remains in the cache: the panic location table (4–5 bytes per location), relocations, item summaries (21 KB), the entry index (20 bytes per entry, about 40 KB). Integers are variable-length, and offsets and lines are written as differences from the previous value.

zustc binaries have no symbols (stripping gives the same size). rustc links std statically, so even after strip it is 335 KiB or more. zustc compiles only the parts of the prelude that are used and links libc dynamically, so its binaries are 1/10–1/30 of rustc's.
