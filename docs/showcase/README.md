<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# The feature showcase

`index.html` is a generated page for people evaluating butterfly: what it
does, shown on the repository's own examples, with the commands to reproduce
each thing and its limits stated beside the claim. **Do not edit it, and do
not add images by hand.** Everything in this directory except this file is
written by one command:

```sh
scripts/showcase.sh
```

From a clean checkout that builds butterfly (`cargo build`), builds each
example, runs every command the page prints, serves the examples, drives the
viewer with Playwright, takes the screenshots, writes `index.html` and the
self-contained `butterfly-showcase.html` (images inlined, for mailing; not
committed), and opens both from `file://` with the network off to check them
at 1460, 1024 and 400 px in light and dark.

It needs `cargo`, `node`, `clang`, and the browser checks' Playwright
(`cd ui/browser && npm ci && npx playwright install chromium`). Universal
Ctags is needed for one section (V21).

## The rule

Nothing appears on the page unless the script can reproduce it (#150).

| file | what it is |
|---|---|
| `support/features.toml` | one entry per section: claim, limits, fixture, the commands, the screenshots and the interactions before each, or `pending` with the blocking issue |
| `support/matrix.toml` | the language and toolchain matrix (#151); rendered when present, "matrix not yet merged" when not |
| `scripts/showcase/capture.cjs` | runs the commands verbatim, records real output and exit status, takes the screenshots on top of `ui/browser/harness.cjs`; writes `capture.json` and `img/` |
| `scripts/showcase/facts.cjs` | reads every number the page quotes out of the evidence the capture run produced |
| `scripts/showcase/build.cjs` | writes the page from the three inputs above; `--single-file` inlines the images |
| `scripts/showcase/verify.cjs` | the `file://` checks |
| `ui/showcase_toml.test.cjs` | the rules `features.toml` is held to, run by CI with the viewer tests |

- **Numbers are never typed in.** A claim writes `{{c.gapsTotal}}`; the value
  comes from the evidence at capture time, a placeholder that does not
  resolve stops the build, and a test refuses a digit in a claim that is not
  a reference. If a README disagrees with the evidence, the page prints the
  evidence.
- **A section's numbers are its own.** A section may quote only evidence it
  names (its `fixtures`, its shots' fixtures, its `facts` keys), and the
  capture run records the values it read *beside that section* in
  `capture.json`. So a `--only` refresh, which leaves the other sections'
  records and screenshots alone, cannot rewrite their text: each section is
  filled from the numbers that were live when its screenshot was taken.
- **A caption is a claim.** Every screenshot states in `expect` what it must
  show, and the strings are checked **inside the crop that is published** —
  the `clip` rectangle, or the captured viewport — immediately before the
  shot is taken, not against the whole document. The match is
  case-insensitive (the viewer upper-cases headings in CSS) and
  whitespace-insensitive, but it is not a bare substring: a word boundary is
  required where the expectation begins or ends on a word character, so
  `28 gaps` is not satisfied by `128 gaps`. "Inside the crop" means inside
  the **painted** rectangle: the element's box cut down by every ancestor
  that clips it, so text scrolled out of one of the viewer's panes does not
  satisfy an expectation even though its rectangle is still on screen. A
  command must end with the exit status written beside it and contain what
  its own `expect` names, or the run fails. What is *not* checked: that the
  text is legible, or that the screenshot shows nothing else.
- **What the host cannot show is said, not skipped**: "not captured on this
  host: <reason>". A section waiting on another issue is `pending` and
  renders as "not yet demonstrated — tracked in #N".
- **The footer names** the commit, whether the work tree had uncommitted
  changes, the date, the host and the tool versions.

## Changing a section

Edit `support/features.toml`, then `scripts/showcase.sh --only V05` to refresh
that section (the page says when a capture was partial), and run the whole
script before committing. `--reuse-fixtures` on `capture.cjs` skips rebuilding
examples while you iterate on a selector; the page says that too.

`docs/**` changes skip CI by design (#147). `support/` and `scripts/` do not,
which is why the data and the generator live there.
