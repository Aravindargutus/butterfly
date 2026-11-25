# Inspect an Ozone coverage report

From the repository root, run `./examples/ozone/build.sh`. The script reads
the supplied synthetic Ozone Code Profile Report and writes the importer's JSON
to `examples/ozone/build/profile.json`. This example runs offline with the
workspace dependencies installed. It needs neither a debug probe nor an
instrumented compiler build.

Both modules have a function named `update`. In `filter.c` it covered 7 of
10 instructions. In `gate.cpp` it covered 0 of 4. The separate C++
`operator|` function covered 3 of 3. For the selected scope the total is
3/5 source lines and 10/17 instructions. The following Code Profile Summary's
run counts and load belong to a different table.

To attach this report to compiler evidence after building the C example:

```sh
cargo run --locked -- ingest examples/c/build --source-root examples/c \
  --ozone-report examples/ozone/report.txt -o examples/ozone/build/evidence.json
cargo run --locked -- summary examples/ozone/build/evidence.json
```

The two inputs in this demonstration come from different programs. In a project,
supply the report that corresponds to your intended build and test campaign.
The importer supports English text reports and preserves Ozone's reported scope.
For a vendor-produced example, see [SEGGER's J-Trace PRO reference report](https://www.segger.com/products/debug-probes/j-trace/technology/live-code-coverage/).
