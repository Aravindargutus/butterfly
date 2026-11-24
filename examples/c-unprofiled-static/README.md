# c-unprofiled-static — a call that really does leave the covered source set

The companion to [`c-header-inline`](../c-header-inline), and its converse.
That example is about calls the call graph *can* bind, which must therefore
not be reported as `helper-call` gaps. This one is about a call it cannot
bind, which must therefore keep its gap even though a function of the callee's
name is covered somewhere else.

Three translation units define a function called `scale`:

| unit | linkage | instrumented | llvm-cov name |
|---|---|---|---|
| `src/ext.c` | external | yes | `scale` |
| `src/covered.c` | internal | yes | `covered.c:scale` |
| `src/quiet.c` | internal | **no** | *(no record at all)* |

`quiet.c`'s copy carries `__attribute__((no_profile_instrument_function))`,
so the coverage export has nothing for it while `beta`, in the same unit,
is profiled normally. `beta` calls it, and that call is the one the tool has
to get right.

The symbol table calls all three `scale`. The bare name is therefore contested
and cannot resolve on its own, so the resolver asks the DWARF which unit
emitted the code at the call's target address: `quiet.c`. No covered function
of `quiet.c` is named `scale`, so the call is **not** in the covered source
set — and the fact that `ext.c` exports a covered `scale` does not change that.
The call graph says so already (`beta → scale` with `functionId: null`) and
the image findings account for the unattributed 24 bytes.

Before #237 the gap check took the name-only fallback here anyway: `scale` was
in the covered set's name list because of `ext.c`, so `beta`'s helper-call gap
disappeared while the call graph still called the edge unbound — the two
disagreeing about the same call, which is exactly what #206 set out to stop.
The fallback is still there, but only for a call with no address or no debug
info to resolve through, where a name is all the evidence there is.

`crates/ingest/tests/dup_unprofiled_static.rs` pins it.

## Build and look

```sh
./build.sh
butterfly ingest build --source-root . -o build/evidence.json
butterfly serve build/evidence.json
```

`beta` carries one `helper-call` gap, class `unknown`, on its call to `scale`.
`main`'s three calls all bind (`scale`, `covered_use`, `beta`); only its
`printf` is a gap, class `libc`.
