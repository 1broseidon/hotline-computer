# Computer 0.11.0-rc.7: ask-me keys (task-268)

Image `hotline-computer:0.11.0-rc.7`, built on the Mac with Apple's `container`
from this commit's tree, run with every capability dropped, agent name Rex,
screen 1600x1000, published on 127.0.0.1. Driven over MCP and the bearer doors
only. Acceptance scenes 1, 5 and 6 of `docs/design/credential-scenarios.html`.

The site is a sign-in page the computer serves to itself on
`http://localhost:8123/login.html` (`drive/login.html`). The passkey was made
through the arming as the desk would, then stored with `"use": "ask"`. The login
is named GITHUB_LOGIN to match the scene, but it is bound to
`http://localhost:8123`, not github.com. "The desk" in these shots is `curl` on
`GET /secrets/uses` and `POST /secrets/uses/answer` (`drive/uses.sh`).

## Shots

| File | What it shows |
| --- | --- |
| `01-new-passkey.png` | The set arrives with PROOF_PASSKEY: "New passkey for this site: PROOF_PASSKEY · Granted to Rex at the desk". |
| `02-new-login.png` | GITHUB_LOGIN added to the set: "New login for this site: GITHUB_LOGIN", its row marked new (scene 1). |
| `02b-after-arrival.png` | About ten seconds later the banner and the mark are gone. (From a second fresh run of the same image; the first run's frame here was a duplicate of 02.) |
| `03-passkey-waiting.png` | Rex clicks "Sign in with a passkey" (`browser click_ref`): "Waiting for approval at the desk", `PROOF_PASSKEY · localhost · for Rex`, time left, badge 1 on the mark (scene 5). |
| `04-passkey-approved.png` | Approve: "Approved · at the desk HH:MM"; the page signed in; record "Rex signed in with the passkey · approved at the desk"; note bottom right. |
| `05-passkey-declined.png` | Deny: "Declined at the desk · The site was told no."; the page is refused with "the person denied this use of PROOF_PASSKEY on localhost"; red record line and coral note. |
| `06-fill-waiting.png` | `browser fill` of `GITHUB_LOGIN.password` answered at once (0 s) with the waiting error; the card waits for GITHUB_LOGIN. The username fill before it went through (a username is never held). |
| `07-fill-approved.png` | After approve, the same fill again fills: record "Rex filled a login · approved at the desk", note. |
| `08-passkey-allowed.png` | Allow 10 min: "Approved · allowed 10 min · until HH:MM"; record "allowed 10 min". |
| `09-within-allowance.png` | The next passkey sign-in goes through with nothing held (`GET /secrets/uses` was `{"uses":[]}`); note "within the 10 min". |
| `10-record-within.png` | The card opened from the toolbar mark: the record line "within the 10 min". |
| `11-fill-declined.png` | A new fill held and denied: "Declined at the desk · Rex was told no."; record and note "you denied it". The retry then answered "The person denied this use of GITHUB_LOGIN on localhost. …". |
| `12-auto-fill.png` | GITHUB_LOGIN re-delivered as automatic (no restart): the fill types at once, nothing held, note "Rex filled a login on localhost" (scene 6). |
| `13-auto-record.png` | The record line for that automatic fill. |
| `14-expired.png` | An ask-me passkey left unanswered until Chromium's time limit: "Not answered in time"; the page refused with "nobody approved … in time"; the use gone from `GET /secrets/uses`. |

## Verified

- `make check`: fmt, clippy `-D warnings`, 151 unit tests pass (10 of them new:
  `holds::tests` six, two in `secrets`, two in `bridge`); the contract target
  skips without a container.
- `cargo test --test contract` against this image at the default 1920x1080 screen:
  1 passed (the whole container contract, now including the ask-me section:
  held passkey get approved then signs, 409 on the answered id and on an unknown
  id, 400 on a bad answer, denied then refused at the page, a page that aborts
  ends its use and its id answers 409, fill answers waiting at once with an id
  and types nothing, a retry before the answer names the same id, the retry
  after approve fills, allow lets a second sign-in through with nothing held,
  back to automatic the fill is not held).
- The same contract run against the 1600x1000 container passed every secrets and
  ask-me check, then failed later on the window-tiling check, which expects a
  1920x1080 screen. Not related to this change.
- The walk in `drive/scenes.sh` (helpers beside it; `setup.sh` expects a
  `put-login.json` made from `login.html`).

## Seen while driving

- Chromium hands the extension a request's time limit as it adjusted it: a page
  asking for 10 s or 120 s arrived as 180 s. A held passkey sign-in therefore
  waits about 3 minutes, not the page's nominal 10 s; the 5-minute cap was not
  reached.
