---
title: Desktop
description: What is on the computer's screen: the top bar, where windows go, the job observer, the Activity window and a terminal of your own.
cluster: Using it
reviewed: 2026-11-02
---

**The desktop is laid out so you can see what the agent is doing at a
glance: its app on the left, its jobs on the right, and the state of the
machine in the top bar.**

## Top bar

The 40px bar starts with the Hotline mark, the agent's name and its current
job. `HOTLINE_COMPUTER_AGENT` names the agent; otherwise the latest named MCP
client does. Window pills show each app's icon and title: click to focus,
right-click to close.

The right side shows the stored-secret count when there is one, the jobs
(`no jobs · 0 done`, `2 running · 5 done`, `1 failed · 2 running`), and
`agent at work`, `agent idle` or amber `you're driving`. The jobs chip opens
the job list. A tray appears when an app uses one, then the clock in `TZ`
(UTC otherwise).

The mark opens a menu:

| Key | Opens |
| --- | --- |
| **B** | The browser |
| **T** | A terminal of your own |
| **O** | The agent's job observer |
| **V** | Recent activity |
| **A** | About |

## Layout

Apps fill the work area inside 16px gutters, with a painted ring and a
subtle shadow; `HOTLINE_COMPUTER_GUTTER=0` makes them edge to edge. The
observer and Activity take the right third, split 60/40 when both are open,
and `windows tile` keeps the same gap. Window operations verify the focus,
geometry or disappearance they asked for, and a refused one reports the
windows that remain.

## Observer

The observer is a terminal window in the bar's colours that shows the
agent's jobs without anyone typing into it. Each job is a block headed by
its label, or its command line when it has none, with its id, start time
and directory underneath, and a ✓ or ✗ line naming it again when it ends.
Closing or reopening the observer never stops a job.

Jobs keep 4 MiB of output each and report truncation; the newest 64 are
kept and 16 run at once. Cancelling or a deadline kills the job's process
group. After a restart, unfinished jobs are marked `interrupted` and are not
resumed.

## Activity

**V** opens the newest 500 completed tool calls from
`~/.hotline/activity.jsonl`, each with a short redacted target and a ✓ or ✗.
It leaves out typed text, scripts, file contents and command lines. The file
lives in the agent's home and the agent can edit it, so it is a display, not
an audit log.

## Your terminal

**T** opens an interactive bash in the bottom third of the right column, so
it never covers the agent's app: for signing in to something or unblocking a
stuck step. It starts in the workspace with the environment the agent
prepared there. What you type is not a job: it is not kept, and the agent
sees only what is on the screen.

It is the system's bash with Hotline's own rc file, rewritten to
`~/.hotline/bashrc` each time it opens. Your own `~/.bashrc` runs after it.
From there, `hotline-computer prepare --packages go gopls` prepares the
workspace by hand, and `hotline-computer packages` lists common Nixpkgs
names. `~/.local/bin`, `~/go/bin` and `~/.cargo/bin` lead the PATH, so what
you install for yourself runs by name.

There is no file manager: a folder, from the browser's **Show in folder**,
`xdg-open` or `hotline-computer open DIR`, opens as a fresh terminal in it.
