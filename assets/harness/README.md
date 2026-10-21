Harness marks, drawn beside the agent's name on the desktop bar and in the
viewer when an agent connects through one of these harnesses
(`agent::harness` matches the name its MCP client gives). They identify the
client that connected, nothing more; every mark belongs to its owner.

| Mark | Source | Licence of the SVG |
|---|---|---|
| `claude` | Simple Icons 16.35.0, `claude` | CC0-1.0 |
| `codex` | thesvg, `codex-openai` mono | MIT |
| `cursor` | thesvg, `cursor` mono | CC0-1.0 |
| `opencode` | thesvg, `opencode` mono | MIT |
| `copilot` | thesvg, `github-copilot` mono | MIT |
| `windsurf` | thesvg, `windsurf` mono | CC0-1.0 |
| `cline` | thesvg, `cline` mono | CC0-1.0 |
| `roocode` | thesvg, `roocode` mono | MIT |
| `zed` | thesvg, `zed` default | MIT |

thesvg is https://github.com/glincker/thesvg at commit 8d07be9f. Only marks
it lists as MIT or CC0 are used. Kilo Code is named but has no mark: its
mark does not read at 14px.

Each SVG here is the source with its title dropped and every fill set to
`currentColor`, so the bar and the viewer can tint it. The PNGs are 14px
alpha masks for the bar: each SVG rendered at 14x14 in white on a
transparent ground. The viewer inlines the same SVGs (`MARKS` in
`src/viewer.html`), so change both together.

The thesvg marks are used under its licence:

    MIT License

    Copyright (c) 2025 thesvg.org

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.
