<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Limits component: low-level requirements

The README's Markdown example, kept here as a file so that the showcase
(support/features.toml, V11) has a document to run `butterfly requirements`
on. It restates LLR-LIM-001 -- the requirement the YAML and CSV commands read
too -- and LLR-LIM-003 of examples/c-per-test/llr.yaml.

## LLR-LIM-001

An altitude presented for limiting shall be returned unchanged when it lies
within the envelope, and shall be returned as the nearer bound when it lies
outside it.

```yaml
type: low-level
status: implemented
traceability:
  parent: HLR-LIM-001
  code: [src/limits.c::clamp_alt]
  tests: [src/tests.c::t_clamp, src/tests.c::t_ceiling]
```

## LLR-LIM-003

Every fault code the component can report shall map to a stable name,
including codes outside the defined set.

```yaml
type: derived
status: implemented
traceability:
  code: [src/limits.c::fault_name]
  tests: [src/tests.c::t_faults]
safety:
  related: true
  failure_condition: minor
```
