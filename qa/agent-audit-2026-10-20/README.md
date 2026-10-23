# Independent agent audit, 2026-10-20

The computer is useful outside Hotline today. Its strongest parts are the shared visible desktop, semantic browser actions, native accessibility plus pixels, managed shell jobs, and reproducible project environments. The remaining portability work is principally in the tool contract: client identity, stable references, bounded responses, validation, deadlines, and independent onboarding.

## What actually ran

- Published `ghcr.io/1broseidon/hotline-computer:0.10.3`, already cached locally. `state info` reported release commit `7d75305d59b1d09f334c4d7d1149a9312beb7022`, matching this checkout.
- Fresh isolated container `hotline-agent-audit-20261020` on Apple Container 1.5.0, Linux ARM64, 4 CPUs, 4 GiB memory, 1 GiB shared memory, capabilities dropped. Localhost port 18787 and a fresh token; no user workspace, credentials, or existing computer was mounted.
- An independent Python standard-library Streamable HTTP client, with real MCP initialization, session headers, JSON/SSE responses, and image decoding. No Hotline application adapter.
- 92 recorded tool calls across all eight tools, plus initialization/listing and HTTP probes. This is a count of calls, not 92 independent passing test cases.
- Manual agent-directed browser and native UI interactions, then scripted boundary probes. Screenshots were inspected visually.
- The isolated container was stopped and removed after evidence collection. The original user computers were left alone.

This was an audit of the published running image, not a rebuild. Docker itself is not installed on this host, so Docker-specific behavior, x86_64, cold registry pulls, and the complete repository acceptance suite were not tested. A host `cargo test --locked --offline` could not resolve an uncached `atspi` dependency. No claim is made that the Rust test suite ran. No cross-model or full cross-harness matrix was run.

## What worked well

| Workflow | Observed outcome | Evidence |
| --- | --- | --- |
| Independent connection | Eight tools discovered; version and embedded guide available without a host adapter | [tools](tools.json), [info](info.json), [guide](guide.json) |
| Browser form | Entered `Zoë 审核`, selected Pro, checked terms, submitted, and verified rendered result. Fill/select/check reported retained values and validity. Individual actions took 16–60 ms in these local fixtures | [fill](form-fill.json), [verification](form-verify.json), [screenshot](form-screen-0.png) |
| Native desktop | Created a GTK app, read its accessibility tree, typed Unicode with XTEST, clicked Save, verified the label, and tiled it beside the observer | [batch](native-type-and-save.json), [tiling](native-tile.json), [screenshot](native-final-0.png) |
| Managed shell | Retry reused the same job ID; conflicting reuse was refused; stdin/EOF worked; cancellation retained output; exit 17 and stderr were explicit | `job-*` and `pipe-*` records |
| Output management | A 20,015-byte stdout stream returned its last 100 bytes, including the final marker, plus a retained-log pointer and truncation indication | [output](job-output.json) |
| Project environment | Prepared Python 3.12 from a manifest in 9.357 seconds with an initially unprepared workspace and the image's bundled Nix cache. A warm prepare answered in 3 ms. A named compile check exited 0; a named web run provided a ready URL and the browser button worked | [prepare log](project-prepare-log.json), [warm prepare](project-prepare-warm.json), [test](project-test-result.json), [app](project-app.json), [screenshot](project-final-0.png) |
| Recovery | Closing Chromium was followed by successful automatic relaunch in 205 ms. Graceful container restart retained files, output, and the Python environment; the stopped running job was recorded as `cancelled` | [browser recovery](browser-reopen.json), `restart-*` records |
| Boundaries that worked | Unauthenticated MCP and file requests returned 401; `/health` returned 200. Out-of-home file read was refused. Distinct custom holder headers enforced leases | [HTTP](http-boundaries.json), [path refusal](file-outside-home.json), [lease refusal](lease-named-b-write.json) |

The native tree is compact and actionable. Screenshot coordinate mappings remove a common source of agent mistakes. The shell job lifecycle is especially valuable: the agent can reconnect and reason about execution without owning a terminal UI session.

## Reproduced problems, ordered by consequence

### 1. Shell and supervisor secrets are not isolated

Inside this empty test computer, an ordinary shell job could read `/proc/1/environ` and detect `HOTLINE_COMPUTER_TOKEN`. It could also fetch `http://127.0.0.1:9222/json/version`, which advertised a DevTools WebSocket. The probe returned only booleans and status; it did not print the token or access any real user credentials.

Evidence: [boundary-check.json](boundary-check.json). Source: `src/browser.rs` starts Chromium's debugging listener; `src/boot.rs`, `src/lib.rs`, and `src/jobs.rs` govern process identity/environment.

This confirms two access paths identified in the existing launch punch list. A general-purpose shell and the supervisor currently share too much authority for a claim that the agent cannot access the computer's control credentials. Define the trusted-computer threat model explicitly; stronger protection needs an OS-enforced boundary between workload processes and supervisor/browser secrets. Moving the debugging port alone is not a complete credential boundary. Full passkey extraction and other exploit chains were not tested.

### 2. Standard MCP sessions collapse into one lease owner

Two separate MCP clients with distinct client names and session IDs omitted `X-Computer-Holder`. Client A acquired a lease. Client B successfully wrote a file during it and then released A's lease. Both were identified as `anonymous`. With distinct custom header values, B's mutation was correctly refused.

Evidence: `lease-anonymous-*.json` versus `lease-named-*.json`. Source: `src/serve.rs:50`.

Use a stable server-issued/session-bound identity by default, and make any reconnect ownership mechanism explicit. A custom optional display name should not be the sole ownership mechanism. This is coordination between equally authorized clients, not a claim that an attacker without the bearer can connect.

### 3. Old browser references can target the wrong control

The first snapshot assigned `e2` to Original action. After inserting a new button and taking another snapshot, `e2` referred to Different action. Calling the original reference returned `clicked e2` and triggered `DIFFERENT`.

The guide says references last for one snapshot, so this probe deliberately used an expired reference. The defect is silent reuse rather than rejection. Another client's nominally read-only `browser text` can cause the same invalidation.

Evidence: `components-text.json`, `components-refreshed.json`, `components-stale-click.json`, [result](components-stale-result.json). Source: `src/browser.rs:300`.

Include snapshot generation and page identity in references, keep mappings on the server, and reject expired references. This is more robust than relying on every model to track an implicit lifetime perfectly.

### 4. Browser semantics miss visible embedded controls

`browser text` returned only the outer heading and button. Both an iframe form and an open shadow-root form were visible, and `capture`'s native accessibility tree found their controls. Neither received browser references.

Evidence: [browser text](components-text.json), [capture tree](components-screen.json), [screenshot](components-screen-0.png). Source: the top-document `querySelectorAll` in `src/browser.rs:308`.

Add frame-aware and shadow-aware discovery and actions, or clearly report those unsupported regions and direct the agent to the accessibility/coordinate fallback. The fallback works as an inspection path, but the advertised semantic path is incomplete for component-based sites.

### 5. Response sizes and deadlines are not uniformly bounded

A fixture of 1,000 text paragraphs returned **354,904 bytes** from `browser text`. There is no caller-controlled page/character limit or cursor. `state info` returned 4,081 bytes and `state guide` 20,752 bytes, both repeating the catalog; the complete tool listing was 13,499 bytes. These sizes are measured UTF-8/JSON bytes, not token estimates.

A batch containing two 26-second wait steps took **52.274 seconds**, exceeding the guide's 50-second ceiling. Both steps failed, but the overall MCP response had `isError:false`; the failures were available only inside its JSON text. Source also permits a 60-second browser action and a 120-second machine-lock wait, so a per-operation timeout is not an end-to-end call deadline.

Evidence: [long page](long-page-text.json), [batch](batch-call-budget.json). Source: `src/browser.rs:327`, `src/tools/input.rs:318`, `src/lease.rs:11`.

Add bounded/filterable snapshots, chunked file reads, a compact info response, and one deadline covering queue admission plus execution. Return explicit overall/partial batch status. Long work should retain the existing job-ID continuation pattern.

### 6. Invalid or omitted arguments can have surprising effects

- `input {"action":"click"}` returned `clicked`. Missing coordinates default to `(0,0)` instead of being rejected. Source: `src/tools/input.rs:19`.
- The input schema advertises `capture_after` default `final`; an ordinary move without that argument returned only `moved`. Explicit `final` returned the tree. Batch applies the default differently. Source: `src/tools/input.rs:69`.
- `capture {"mode":"png"}` saved under `/tmp`; passing the returned path to `files get` was refused because files must be under `/home/agent`. Save default artifacts under home, or return a downloadable resource. Source: `src/tools/capture.rs:66`.
- A relative put into new top-level `audit/hello.txt` failed with `path has no parent`; absolute paths worked. The earlier relative `src/audit-project/...` writes worked because `src` already existed. Normalize relative paths to home consistently, or require absolute paths clearly.

Evidence: `input-missing-coordinates.json`, `input-default-capture.json`, `input-explicit-capture.json`, `capture-png-default.json`, `capture-png-get.json`, `file-put.json`, `file-put-absolute.json`.

Validate fields per action before mutation. Errors should state the missing field and provide a valid example. Defaults should be implemented consistently, not just advertised in the schema.

### 7. Origin validation is absent on the tested MCP route

A request with the correct test bearer and `Origin: https://audit-untrusted.example` was accepted with HTTP 200. No-token requests were refused, so this is not evidence of a tokenless exploit against the authenticated instance. It does demonstrate the missing Origin rejection required by the [MCP Streamable HTTP specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).

Evidence: [HTTP probes](http-boundaries.json). Source: `src/serve.rs:119`. Tokenless startup defaults were inspected in source but not separately launched in this audit.

## What I would change for harness and model independence

1. Fix the dangerous ambiguities first: expired references, omitted coordinates, and session ownership. Add the reproductions above to the contract tests.
2. Publish a precise result contract. Most results are JSON encoded inside text, and none of the eight tools had output schemas or annotations. Text is useful for broad compatibility; add `structuredContent` and stable success/error codes alongside it. MCP supports both [structured and unstructured results](https://modelcontextprotocol.io/specification/2025-11-25/server/tools). These are improvements, not a claim that text-only results violate MCP.
3. Make schemas action-specific where supported, with equivalent runtime validation everywhere. Eight names keep discovery small, but each exposes a large collection of unrelated optional fields. A short call example per action helps models more than a long prose description. Test omissions, explicit nulls, retries, unknown fields, and partial batches through independent clients.
4. Keep manifests as a useful optional project workflow. The bundled guide's instruction to add every build/launch command to a manifest is an orchestration preference, not a requirement for a computer tool. Separate an essential short operating guide from optional project policy so another harness can retain its own planning and environment conventions.
5. Add a published-image quickstart and verified client connection recipes, including bearer configuration, session behavior, image handling, and first calls. Document an optional stdio bridge for clients that need one. Core HTTP operation already works without Hotline. Secret/passkey provisioning still uses additional HTTP routes and desk-oriented instructions; expose/document an independent management API separately from agent tools.
6. Build an interoperability gate using at least one non-Rust MCP SDK and real supported harnesses. This audit establishes independent protocol operation, not compatibility with every model or host. Include text-only clients, multimodal clients, smaller context budgets, reconnects, concurrent clients, and timeout cancellation. Initialization negotiated 2024-11-05, 2025-03-26, 2025-06-18, and 2025-11-25; a request for 2026-07-28 negotiated down to 2025-11-25 in this image. These were negotiation checks, not complete tests of each protocol version.

## Reproduction and handoff

`client.py` is the independent client. Set `AUDIT_URL` to an isolated test computer and `AUDIT_TOKEN_FILE` to a private env file containing `HOTLINE_COMPUTER_TOKEN=...`. Its default URL is localhost:18787. `client.py init` lists tools; `client.py TOOL JSON LABEL` records one call. `probes.py` contains the jobs, clients, browser, prepare, native, HTTP, context, recovery, and boundary probes. Some probes expect earlier setup; these scripts document the exploratory audit, not a standalone acceptance gate.

`trace.jsonl` is the chronological record; named JSON files hold the latest run for a label. The trace includes initial failures and corrected follow-ups. Image bytes are saved separately as PNGs. Temporary client sessions and the test bearer are not stored in this directory.

No tracked implementation files were changed. The pre-existing `LAUNCH-PUNCHLIST.md` was left untouched. Project Recoil and Brainfile state were unavailable/uninitialized, so this QA record carries the handoff. Next implementation step: turn the session-owner, stale-reference, and missing-coordinate reproductions into contract assertions, then fix those behaviors before broadening the model/harness matrix.
