# Computer 0.4.0 contract ledger

Observed: eight MCP tools in src/tools/mod.rs; shell exec/launch in src/tools/shell.rs; public HTTP /health, /, /ws, /mcp in src/serve.rs. Scope: existing MCP service; caller profile: agent, script, human operator. Surface pattern: eight named tools with action enums.

| Surface | Change | Risk | Compatibility | Verification |
| --- | --- | --- | --- | --- |
| shell exec | shared job owner, retained output | R2 | existing stdout/stderr/exit_code/duration_ms/truncated fields retained | unit and live contract |
| shell launch | add structured job result | R2 | remains launch with command/args/cwd; PID and job ID in JSON | live contract |
| shell lifecycle actions | add start/list/status/read/wait/write/cancel/show | R1 | additive actions | job lifecycle tests |
| state environment/guide discovery | add catalog/prepare/info/guide | R1 | existing actions retained | live contract and catalog smoke |
| files artifact actions | add download/extract/run | R1 | existing get/put/list retained | artifact integration |
| viewer clipboard | add explicit controlled paste | R1 | view-only default retained | lease and browser viewer tests |
| input/browser/windows | verify results, reject invalid operations | R2 | existing tool/action names retained | regression fixtures |

Implementation first modifies Dockerfile, src/lib.rs, src/tools/shell.rs, src/tools/mod.rs; creates src/jobs.rs and lifecycle tests. New execution replaces duplicated private spawning helpers. Existing authorization covers the requested replacement. No unrelated public removal.
