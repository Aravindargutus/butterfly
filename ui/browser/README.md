# Real-browser checks for the viewer

What a DOM test in a VM cannot cover: event ordering, focus and hover
behaviour, layout overflow, and request races between the viewer and its own
API. Five of these (BF-QA-01..05) are regressions that were found by hand
during the QA pass for #68 and had no automated equivalent (#69).

## Running them

```sh
cd ui/browser
npm ci                              # pinned by package-lock.json
npx playwright install chromium     # the browser Playwright itself ships
npm test                            # or: node run.cjs [suite ...]
```

The workspace binary and the fixtures have to exist first:

```sh
cargo build --locked
./examples/c-per-test/build.sh                              # requirements, review
target/debug/butterfly build examples/c/butterfly-build.toml # variants
```

Exit codes: `0` every check passed, `1` a check failed, `2` the checks could
not run at all (Playwright absent, binary not built). A suite whose fixture
is missing is *reported* as a skip and never counted as a pass.

Set `BUTTERFLY_BIN` to test a binary elsewhere, and `BROWSER_CHECK_OUT` to
keep artifacts in a known directory instead of a temp one.

## Suites

| Suite | Covers |
| --- | --- |
| `requirements` | the requirements pane: text loading from the sidecar, Escape dismissal (BF-QA-02), click-to-pin (BF-QA-03), recovery from a transient API failure, requirement text never being read as markup, layout overflow at 1024–1920px, and every function against every tab |
| `review` | the review workflow against a writable server: save/reload/remove (BF-QA-04), and a failed save keeping the reviewer's draft (BF-QA-05) |
| `variants` | variant switching under concurrency (BF-QA-01): an out-of-order load never showing one build's evidence under another's label, and a failed switch leaving the write target on the build still displayed |
| `vars` | the Variables panel and its source marks (#53), against **hand-written evidence** rather than a compiled fixture: the panel's contents, the static-order wording, marks that survive a move onto a badge or the pointer leaving a focused row, a branch site marked on a line that never ran, the Variable marks toggle, and the unsupported-target state |
| `token` | the token page against a server with a real token (#65): a first login, then a 401 on a later read after the server restarts with a new token, and the new token (with `/` and the viewer's other shortcut keys in it) typed back in by keyboard, arriving intact with no page errors |
| `footer` | the footer against the overlay (#209), on **hand-written evidence** carrying a `measured` block: each overlay button, in the function view and in the file view, rewrites the footer; the footer names the overlay whose button is lit and no other; a block is never captioned as measured under a footer that says projected; a remembered Measured with no trace is not claimed |

## Things worth knowing before editing these

**The assertions are not the original QA scripts' assertions.** Those ran
against `artifacts/.product-deck-build/requirements-demo`, which was never
committed — they asserted on `LLR-ALT-001`, "altitude limiter",
`clamp_altitude`. These are rewritten against `examples/c-per-test`, the only
in-repo example carrying an `llr.yaml`. They are equivalent in *intent*, and
they have never caught the original regressions, because those regressions
were fixed before these existed. Treat them as cover going forward, not as
proof that the #68 fixes hold.

**No `page.waitForFunction`.** The viewer serves a strict
`script-src 'self'` with no `unsafe-eval` (`crates/view/src/lib.rs`), and
Playwright's waiter evaluates its polling wrapper as a string, which that
policy correctly refuses. Use `waitFor` from `harness.cjs`, which polls with
`page.evaluate` instead. The product should not have to weaken its own CSP to
be testable.

**`S` is not on `window`.** It is a top-level `const` in `app.js`, so it
lives in the global lexical environment: reachable by bare name inside
`page.evaluate`, but `window.S` is `undefined`.

**The viewer caches requirement text by ID for the session** (`REQ` in
`app.js`). A routed response will not reach the pane if an earlier check
already loaded that ID, so `open()` uses a fresh query token every time to
force a real page load.

**Writable checks get their own server and their own `--resolutions` path,**
per check. Sharing one makes each check depend on the previous one cleaning
up, and one early failure then cascades. The sidecar path is always explicit
and inside a temp directory: the default is the relative name
`resolutions.json`, resolved against the process's cwd, so a test could
otherwise write a review into the work tree.
