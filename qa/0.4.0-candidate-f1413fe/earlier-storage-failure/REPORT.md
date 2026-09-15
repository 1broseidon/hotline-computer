# Computer acceptance results

Overall: PARTIAL / native acceptance BLOCKED by computer storage exhaustion. No native launch or room/Settings/Computer inspection was possible.

## Identity
Computer 0.4.0, development revision b7cd1fa, Linux aarch64, Mesa software rendering. This is a development build, not an official release. Running guide has the same identity; SHA-256 6a533dc21a499aac1a8af5965a650a6b2827d73ab144cc8bc6adb98b7d60b113. Catalog Nixpkgs ef34387ddd751e1ab8857adf4676492d32eb24ec.

## Verified
- Selenium: synthetic text/password/textarea, dropdown, datalist, attachment, checkbox/radio, color/date/range. Submitted result displayed Form submitted / Received! Disabled and readonly controls retained defaults.
- Local form: simple submission echoed synthetic values. Wizard accepted QA-040 with Team, Toad QA, 2026-09-14, attachment.txt, Linux and macOS. Back to contact retained values, then Review showed the expected payload before confirmation/submission.
- ketch: GitHub v0.16.2 Linux arm64 release archive downloaded using files download; release checksums.txt retained. Archive SHA-256 61e0a7ef16350534c586d7e2755a490333334fc40bdeac01c092bf77bf030b8c, verified=true. Extracted with files extract to /home/agent/qa/ketch; executable reports ketch v0.16.2. Local Python server on 127.0.0.1:8765 served the retained /home/agent/qa/fixture/index.html. JSON title/content assertions passed; no API credentials used. Server stopped after testing.
- Repo cloned and checked out exact f3c17b78d85b4cc235455862d9c9241579157b39 (Toad 0.11.0). Read README, AGENTS, development instructions, Makefile and Tauri config. Prepared rust-tauri successfully. Bun frozen install and production frontend build/typecheck passed.
- Observer closed/reopened during CLI compilation (job 000001a0a3113128-15cf143c, PID 484) and native app compilation (000001a0a31487dd-d7a5e740, PID 30573); IDs, PIDs and start times persisted. Browser text and screenshots ran during compilation.

## Failures and friction
- browser fill with value instead of text silently reported success with empty retained values. Corrected using text, inspected values and resubmitted Selenium.
- bash -lc resets prepared PATH; bash -c retained catalog tools.
- rust-tauri did not include cargo tauri, a documented repository prerequisite. cargo install tauri-cli --version ^2 --locked failed after 562.193 seconds: signal 9, job error 'save job ... No space left on device (os error 28)'. No assertion that disk alone caused signal 9.
- Native cargo build --locked -p toad-desktop with debug symbols and incremental compilation disabled failed exit 101 after 356.055 seconds; ring and aws-lc-sys C compilation reported No space left on device. Final df: 23G total, 23G used, 142M available, 100%. Native binary not produced; no app was launched. The documented make build path first stopped on missing CLI; direct native compilation was an additional diagnostic attempt.
- Native UI remains UNTESTED, including isolated application data launch. No success inferred from process starts.
- Recoil unavailable in computer (exit 127). Later required handoff attempt was rejected by automatic approval review as prohibited teammate contact; not retried.
- Tracked repository files unchanged (git diff --exit-code passed); only prepared-environment .toad/ metadata untracked. All execution stayed inside Computer MCP; no host shell, delegation, publication or teammate contact.

## Timing
Job timestamps measure downloads/builds separately from MCP round trips: environment preparation 27.344s; checksums download 0.890s; 8,059,545-byte ketch archive download+verification 0.954s; frontend build/typecheck 18.767s; CLI install attempt 562.193s; native compile attempt 356.055s (overlapping). Local scrape command initially 46ms. Measured version probe round trip 3951ms vs reported execution 450ms, difference 3501ms includes tool transport/dispatch/observer overhead; it is not compilation. These are single observations, not benchmarks.

## Evidence
All paths relative to /home/agent/qa/live-agent-evidence:
- computer-info.json, computer-guide.json, observations.txt, tool-timing.json
- selenium-result.png, wizard-review.png, wizard-result.png, wizard-result.json
- ketch-checksums.txt, ketch.tar.gz, ketch-scrape.json, attachment.txt
- observer-before-close.png, observer-reopened.png, app-build-observer-reopened.png
- browser-during-compilation.png, browser-during-app-build.png
- native-build-failure.png, native-failure.json
- jobs/: retained managed job records/stdout/stderr; job-records.json is an earlier snapshot. Final status snapshot in final-jobs.json.
Screenshots saved via capture mode=png at original desktop resolution. Native build failure is an environment acceptance failure, not a demonstrated source defect. More computer storage is required before retrying native acceptance.
