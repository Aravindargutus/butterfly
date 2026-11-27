# Security

## Reporting

Open a GitHub security advisory on this repository. Please do not file a public
issue for anything exploitable.

## butterfly does not run your build

butterfly reads build output. It never executes your build script, your test
runner, or your compiler on your behalf from the viewer.

This is a deliberate limit, not a missing feature. `build.sh` is arbitrary
code by definition — it is yours, and it can do anything you can do. Any HTTP
endpoint that runs it is a remote-code-execution primitive, and no amount of
guarding changes what the endpoint fundamentally is.

The concrete attack it would open, on the default configuration:

`butterfly serve` listens on loopback with no credentials, which is the right
default for a local tool. `guard_host` rejects requests whose `Host` header is
not a loopback name, which stops DNS rebinding — an attacker's domain
resolving to `127.0.0.1`. It does **not** stop a page you happen to be
visiting from calling `127.0.0.1` directly: that request carries
`Host: 127.0.0.1:8787`, which is legitimately on the allowlist. There is no
CORS layer and no `Origin` check, so the browser's same-origin policy would
stop that page *reading the response* — but a simple `POST` (form-encoded or
`text/plain`) is sent without a preflight, and the side effect happens anyway.

For a read-only endpoint that side effect is nothing. For an endpoint that
spawns `build.sh` it is code execution, triggered by any web page open in the
browser of anyone running a viewer. So the viewer has no such endpoint, and
adding one requires revisiting this document first.

What the viewer does instead: it shows you the command and you run it. See
[`templates/build.sh`](templates/build.sh).

## What the viewer exposes

The evidence file embeds the **complete text of every analysed source file**.
Serving a viewer serves your source code.

| Listener | Reads | Writes (review decisions) |
|---|---|---|
| loopback (default) | open | open |
| non-loopback, no token | refuses to start | — |
| non-loopback, `--allow-unauthenticated-remote` | open | refused |
| any, with a token | bearer token | bearer token |
| any, with `--read-only` | as above | refused |

Off loopback, reads need a token as well as writes. The evidence is the
complete source, and the report and freshness routes do real work per request.
An open network listener has to be asked for by name.

The expensive routes (report, freshness, gates) run at most four at a time.
Past that they answer `503` with `Retry-After`. Their results are cached, and
no request is held open for more than a minute. Per-client rate limiting
belongs in the reverse proxy in front, along with TLS.

The parsed evidence is kept in memory beside the JSON, so that it is never
reparsed per request. Measured on the bench fixtures, the process is resident
at about 3–4× the evidence file, within 4% of a build that freed the parse
after startup. Per-request copies of `/api/evidence` are a separate change
(#104).

A token comes from exactly one of `--token-file <path>`, `BUTTERFLY_TOKEN`, or
`--token generate`. A literal `--token <secret>` still works but is deprecated,
because it is visible in the process list and kept in shell history. The viewer
never puts a token in a request target: a minted one is handed over in the URL
fragment (`#token=`), which browsers do not send, and a `?token=` URL is
refused.

A non-loopback address needs two flags: `--allow-remote` to bind it at all,
and `--insecure-http` to accept that the built-in server has no TLS. Without
TLS the bearer token and the source cross the network in clear text. Prefer an
SSH tunnel:

```
ssh -N -L 8787:127.0.0.1:8787 <host>
```

or a TLS-terminating reverse proxy. `--insecure-http` then acknowledges the
plain hop between the proxy and butterfly. A proxy on the same host needs
neither flag, because butterfly can stay on loopback.

## Untrusted input

Coverage exports and object files are parsed as untrusted input. Source paths
inside a coverage export are attacker-controlled, so ingest will not read a
file resolving outside `--source-root` unless you pass
`--allow-external-source`.

## No privilege escalation

butterfly runs as you. It does not install anything, does not write outside the
paths you name, and has no setuid, sudo, or daemon component. If you find it
doing otherwise, that is a bug worth reporting.
