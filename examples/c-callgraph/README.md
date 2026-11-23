# c-callgraph — a call graph with something to expand

Every other C example here is flat. `examples/c` is `main` calling nine
leaves, so the Calls tab has exactly one hop to draw: nothing is expandable,
and a test written against it would pass while exercising none of the
expansion logic (#100).

This fixture exists so that every state the call graph can draw actually
occurs in it.

| shape | functions | what it exercises |
| --- | --- | --- |
| depth | `main → stage_one → stage_two → stage_three → leaf` | more than one expansion before the chain ends |
| direct recursion | `countdown` | a node that must link back to itself rather than be drawn twice |
| mutual recursion | `ping ⇄ pong` | a cycle spanning two functions |
| indirect call | `dispatch` through a function-pointer table | candidate targets, which are hypotheses rather than observed calls |
| external call | `copy_len` → `strncpy`/`strlen` | a symbol whose object code is not in this evidence, so there is nothing to open |

12 functions, and 8 call edges that lead to a function with further calls of
its own.

## Building it

```sh
./examples/c-callgraph/build.sh
target/debug/butterfly ingest examples/c-callgraph/build \
  --source-root examples/c-callgraph -o /tmp/ev.json
target/debug/butterfly serve /tmp/ev.json
```

Open `main` and use the Calls tab: nodes with more behind them carry a dotted
border and a count.

## Why -O0

At `-O1` the optimiser inlines the chain and folds `dispatch`'s table, and
there is nothing left to expand — which is a true and interesting fact about
optimised builds, and exactly the wrong thing for a fixture whose purpose is
to have call structure. `examples/c/butterfly-build.toml` is the place to
look at what optimisation does to a call graph.

The arithmetic is deliberately uninteresting. The shape is the point.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 13 |
| source files | 1 |
| lines covered | 59 / 61 |
| branch outcomes | 5 / 6 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.

The branch outcomes are portable because `build.sh` pins `_FORTIFY_SOURCE`
off: with it on, some SDKs rewrite `copy_len`'s `strncpy` to
`__builtin___strncpy_chk`, whose object-size check adds branch outcomes that
others do not emit (Apple clang 17 and 21 disagreed 6 vs 8). The external
call the graph is demonstrating is unchanged; only the fortify wrapper is
normalised away.
