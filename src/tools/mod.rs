mod browser;
mod capture;
pub(crate) mod files;
mod input;
mod shell;
pub(crate) mod state;
mod wait;
mod windows;

use std::sync::Arc;

use rmcp::model::{ContentBlock, JsonObject, Tool};
use serde_json::{Value, json};

use crate::App;

pub const NAMES: [&str; 8] = [
    "capture", "input", "browser", "shell", "files", "windows", "wait", "state",
];

pub type ToolResult = Result<Vec<ContentBlock>, String>;

/// The longest any call waits before answering. MCP clients commonly give a
/// tool a minute; an answer after that is lost, and the agent sees an error
/// for work that went fine. Longer work is a job to wait on again.
pub const CALL_BUDGET_MS: u64 = 50_000;

pub fn descriptors(home: &str) -> Vec<Tool> {
    vec![
        Tool::new(
            "capture",
            "See the screen — the way in for native apps (web content reads better through browser text). Default returns a screenshot plus the AT-SPI accessibility tree as structured text: windows, elements, roles, screen coordinates, values, and states; elements hidden from view and unnamed grouping containers are left out. window (an id, or part of one title) reads only that window and shows only its pixels; region [x, y, width, height] shows only that part of the screen at full size, for reading small text. The line after the image says how its pixels map to screen coordinates. mode=image returns the picture alone; mode=png saves it on the computer and returns the path. There is no OCR.",
            schema(json!({
                "type":"object","properties":{
                    "mode":{"type":"string","enum":["tree","image","png"],"description":"tree (default): screenshot plus accessibility tree; image: screenshot only; png: save a raw PNG on the computer"},
                    "window":{"type":"string","description":"A window id from windows list, or text from exactly one window's title"},
                    "region":{"type":"array","items":{"type":"integer"},"minItems":4,"maxItems":4,"description":"x, y, width, height on the screen"},
                    "path":{"type":"string","description":"png only: optional output path"},"settle_ms":{"type":"integer","minimum":0,"maximum":2000,"default":100,"description":"Allow application painting to catch up before grabbing pixels."}
                },"additionalProperties":false
            })),
        ),
        Tool::new(
            "input",
            "Drive the mouse, keyboard, and clipboard on the desktop — for native apps, with screen coordinates from capture. For the web browser, prefer browser text and refs. type is per-character and returns once the focused field has taken all of it; paste sets the clipboard and presses Ctrl+V, the better way for long text. scroll takes direction (up, down, left, right) and clicks notches, 3 by default. batch runs up to 10 steps, each an object with action and that action's fields, under one machine lock. capture_after returns the focused window's accessibility tree.",
            schema(json!({
                "type":"object","properties":{
                    "action":{"type":"string","enum":["click","double_click","right_click","move","drag","scroll","type","key","paste","clipboard_read","clipboard_write","batch"]},
                    "x":{"type":"integer"},"y":{"type":"integer"},"x2":{"type":"integer"},"y2":{"type":"integer"},
                    "clicks":{"type":"integer"},"direction":{"type":"string","enum":["up","down","left","right"]},"text":{"type":"string"},"combo":{"type":"string"},
                    "steps":{"type":"array","items":{"type":"object"}},"stop_on_error":{"type":"boolean","default":true},
                    "settle_ms":{"type":"integer","minimum":0,"default":40},"capture_after":{"type":"string","enum":["final","each","none"],"default":"final"},
                    "capture_on_error":{"type":"boolean","default":true}
                },"required":["action"],"additionalProperties":false
            })),
        ),
        Tool::new(
            "browser",
            "The managed Chromium, semantically. text returns an accessibility-style page snapshot with element refs; click_ref, fill, select, check, and hover act on those refs. Refs are stable within one snapshot. fill types text, or one of the person's stored secrets by name (secret instead of text): a login's field is typed only on that login's own sites and the value never appears in the answer. A stored passkey signs in by itself when a site asks for one. Also provides navigation, history, tabs, uploads, dialogs, and downloads.",
            schema(json!({
                "type":"object","properties":{
                    "action":{"type":"string","enum":["navigate","text","links","eval","click_ref","fill","select","check","hover","tabs","tab_new","tab_select","tab_close","upload","dialog_accept","dialog_dismiss","downloads","back","forward","reload"]},
                    "url":{"type":"string"},"js":{"type":"string"},"ref":{"type":"string"},"button":{"type":"string"},
                    "text":{"type":"string","description":"For fill, the text to type (an explicit empty string clears the field) unless secret is given. Also used by dialog_accept."},"secret":{"type":"string","description":"For fill: a stored secret to type instead of text, by name. NAME for a variable; NAME.username, NAME.password or NAME.code (the six TOTP digits of this moment) for a login, which is typed only on a page of the login's own sites. state info lists them. The value never appears in the answer."},"value":{"type":"string","description":"Single option value for select."},"values":{"type":"array","items":{"type":"string"},"description":"Native select values; multiple selections require a multiple select."},"uncheck":{"type":"boolean"},"index":{"type":"integer","minimum":0},"path":{"type":"string"}
                },"required":["action"],"additionalProperties":false
            })),
        ),
        Tool::new(
            "shell",
            "Run managed commands without holding the desktop while they execute. run starts a named entry from the workspace manifest's runs (name; cwd inside the workspace; args are appended), the way to build, test or launch a prepared project. A desktop run answers when its window is up, with the window and a screenshot; a web run gets $PORT and answers when it listens, with its url; both wait up to wait_ms (default 45000, at most 50000 so the answer beats a client's one-minute tool timeout), and ready with the job_id waits again. exec runs command with args, or a whole shell line in command alone; it waits up to 45s and retains stdout/stderr even on timeout. Its answer holds the end of stdout and of stderr, with each stream's full size; when a stream is longer than that, more says where all of it is, and read with the job_id pages through it. start/launch return a durable job ID immediately. list/status/read/wait inspect jobs; write sends stdin (optional EOF); cancel kills and reaps the process group. show opens or focuses the Alacritty observer, with an optional job_id to inspect one job; commands are visible without keyboard simulation. Output is retained across observer closure and reconnect. Use request_id to retry a start safely. pty=true provides a controlling terminal without keyboard simulation.",
            schema(json!({
                "type":"object","properties":{
                    "action":{"type":"string","enum":["exec","start","launch","run","ready","list","status","read","wait","write","cancel","show"],"default":"exec"},
                    "name":{"type":"string","description":"For run: the manifest run to start, such as build, test or app; state manifest lists them"},
                    "command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string","default":home},
                    "env":{"type":"object","additionalProperties":{"type":"string"}},
                    "label":{"type":"string","description":"The job's name as a person reads it in the desktop's jobs list and terminal: a short task in plain words, such as 'Run the unit tests' or 'Install Python 3.12', not the command. Without one the job is named by its command line."},
                    "request_id":{"type":"string"},"pty":{"type":"boolean","default":false},
                    "timeout":{"type":"integer","minimum":1,"description":"Execution deadline in seconds. exec defaults to 30, maximum 45; async jobs have no default deadline."},
                    "job_id":{"type":"string"},"text":{"type":"string"},"eof":{"type":"boolean"},"cursor":{"type":"integer","minimum":0},
                    "wait_ms":{"type":"integer","minimum":0,"maximum":50000},"max_output":{"type":"integer","minimum":1,"maximum":1048576,"description":"For exec, how many bytes of the end of stdout and of stderr to answer with (default 8192). For read, a page's size (default 32768)."}
                },"additionalProperties":false
            })),
        ),
        Tool::new(
            "files",
            format!(
                "Move files across the machine boundary over MCP. get returns text, or base64 when bytes are not UTF-8; put accepts UTF-8 or encoding=base64; list shows a directory. Paths stay under {home}. Cap 50MB for get/put. download fetches URL or a GitHub release asset into path with optional SHA-256 verification. extract expands an archive into a new destination. run downloads and/or executes a script with its interpreter. These actions return managed jobs with retained progress; artifact and expanded size cap 1 GiB."
            ),
            schema(json!({
                "type":"object","properties":{
                    "action":{"type":"string","enum":["get","put","list","download","extract","run"]},"path":{"type":"string"},"content":{"type":"string"},
                    "url":{"type":"string"},"repo":{"type":"string","description":"GitHub owner/repo"},"version":{"type":"string","description":"GitHub release tag or latest"},"asset":{"type":"string","description":"Exactly one asset must match; supports {arch}=arm64/amd64 and {os}=linux"},
                    "sha256":{"type":"string"},"destination":{"type":"string"},"interpreter":{"type":"string","enum":["bash","sh","python3"]},"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"env":{"type":"object","additionalProperties":{"type":"string"}},"request_id":{"type":"string"},"encoding":{"type":"string","enum":["utf8","text","base64"]}
                },"required":["action","path"],"additionalProperties":false
            })),
        ),
        Tool::new(
            "windows",
            "Manage desktop windows: list them with IDs, classes, bounds, and focus; focus, close, maximize or restore; tile a primary_id/observer_id pair, or auto-tile with Chromium left and the rest stacked right. Tiling respects minimum sizes and the desktop work area and verifies the resulting geometry.",
            schema(json!({
                "type":"object","properties":{
                    "action":{"type":"string","enum":["list","focus","close","maximize","tile"]},"window_id":{"type":"string"},"unmaximize":{"type":"boolean"},"primary_id":{"type":"string"},"observer_id":{"type":"string"}
                },"required":["action"],"additionalProperties":false
            })),
        ),
        Tool::new(
            "wait",
            "Poll every 500ms until text appears in the desktop accessibility tree or the managed browser page. Returns when found or after timeout; use it after input or navigation to verify the expected state.",
            schema(json!({
                "type":"object","properties":{"text":{"type":"string"},"timeout":{"type":"integer","minimum":1,"maximum":50,"default":10}},
                "required":["text"],"additionalProperties":false
            })),
        ),
        Tool::new(
            "state",
            "info identifies the running release and lists the person's stored secrets by name and kind: a variable is an environment variable in every shell job, a login is typed into a form on its own sites with browser fill secret, a passkey signs in by itself when a site asks the browser for one; guide returns its bundled skill and checksum; catalog describes generic Nix preparation, the default Nixpkgs pin, and common package names by purpose. manifest describes a workspace's .hotline/manifest.json: an example and what this image offers when there is none, otherwise the composed result, its runs, hooks, services and whether it changed. prepare composes the image base with that file (a manifest argument is merged into the file first; packages and flake still work alone), or reuses the saved workspace definition. After it succeeds, create hooks run once, then services and start hooks. Returns a managed job or ready=true for a package cache hit. Shell cwd inherits the prepared environment. Repository shell hooks run during preparation; their exported variables are retained. control leases the desktop to this holder until release or expiry; other holders' mutations are refused. Only the holder can release it. login_* manages browser cookies/storage by name. snapshot_* archives/restores home.",
            schema(json!({
                "type":"object","properties":{
                    "action":{"type":"string","enum":["info","guide","catalog","manifest","prepare","control","release","login_save","login_load","login_list","login_delete","snapshot_save","snapshot_load","snapshot_list","snapshot_delete"]},
                    "name":{"type":"string","description":"Login or snapshot name; not an environment preset"},
                    "workspace":{"type":"string","description":"Directory under computer home for state prepare and state manifest"},
                    "manifest":{"type":"object","description":"For prepare: merged into the workspace's .hotline/manifest.json (lists concatenate, maps merge) before preparing. Keys: packages, flake, platform, env, services, hooks, runs; state manifest shows an example"},
                    "upgrade":{"type":"boolean","default":false,"description":"For prepare: take this image's newer base (pin, platforms, services) instead of the one the workspace was composed against"},
                    "packages":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":128,"description":"Nixpkgs attributes selected for this project; mutually exclusive with flake"},
                    "flake":{"type":"string","description":"Local flake directory relative to workspace or absolute under computer home, optionally #devShell; e.g. . or .#dev. Mutually exclusive with packages"},
                    "duration":{"type":"integer","minimum":1,"maximum":600,"default":300}
                },"required":["action"],"additionalProperties":false
            })),
        ),
    ]
}

fn schema(value: Value) -> Arc<JsonObject> {
    Arc::new(value.as_object().expect("schemas are objects").clone())
}

pub async fn call(app: &App, name: &str, arguments: Value, holder: &str) -> ToolResult {
    let result = match name {
        "capture" => capture::call(app, arguments).await,
        "input" => input::call(app, arguments, holder).await,
        "browser" => browser::call(app, arguments, holder).await,
        "shell" => shell::call(app, arguments, holder).await,
        "files" => files::call(app, arguments, holder).await,
        "windows" => windows::call(app, arguments, holder).await,
        "wait" => wait::call(app, arguments).await,
        "state" => state::call(app, arguments, holder).await,
        _ => Err(format!("unknown tool {name:?}")),
    };
    // Every answer leaves through here, so this is where the value of a
    // secret the person stored is taken out of it (see `crate::secrets`).
    app.secrets.redact_result(result)
}

pub fn text(value: impl Into<String>) -> Vec<ContentBlock> {
    vec![ContentBlock::text(value)]
}

pub fn json_text(value: impl serde::Serialize) -> ToolResult {
    serde_json::to_string(&value)
        .map(text)
        .map_err(|error| error.to_string())
}

pub fn action_error(tool: &str, action: &str, actions: &[&str]) -> String {
    format!("{tool}: unknown action {action:?} (one of: {actions:?})")
}
