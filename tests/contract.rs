use std::collections::HashSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientInfo, Implementation};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use serde_json::json;

type Client = RunningService<rmcp::RoleClient, ClientInfo>;

#[tokio::test(flavor = "multi_thread")]
async fn image_honors_the_computer_contract() {
    let Some(base) = std::env::var("HOTLINE_COMPUTER_URL").ok() else {
        eprintln!("skipped: set HOTLINE_COMPUTER_URL to run the container contract test");
        return;
    };
    let token = std::env::var("HOTLINE_COMPUTER_TOKEN").unwrap_or_default();
    let health = reqwest::get(format!("{base}/health"))
        .await
        .expect("health request");
    assert!(health.status().is_success(), "/health is open");

    if !token.is_empty() {
        let naked = reqwest::Client::new()
            .post(format!("{base}/mcp"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .expect("unauthenticated request");
        assert_eq!(naked.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(
            naked.json::<serde_json::Value>().await.unwrap()["error"],
            "unauthorized"
        );
    }

    let client = connect(&base, &token, "contract").await;
    let listed = client.list_all_tools().await.expect("tools/list");
    let names: HashSet<_> = listed.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        HashSet::from([
            "capture", "input", "browser", "shell", "files", "windows", "wait", "state"
        ])
    );

    let shell = call(
        &client,
        "shell",
        json!({"command":"sh","args":["-c","echo computer-says-42"]}),
    )
    .await;
    assert!(
        text(&shell).contains("computer-says-42"),
        "{}",
        text(&shell)
    );

    let navigated = call(
        &client,
        "browser",
        json!({"action":"navigate","url":"data:text/html,<title>Proof</title><h1>Rust computer proof</h1>"}),
    )
    .await;
    assert!(!navigated.is_error.unwrap_or(false), "{}", text(&navigated));
    let page = call(&client, "browser", json!({"action":"text"})).await;
    assert!(
        text(&page).contains("Rust computer proof"),
        "{}",
        text(&page)
    );

    let capture = call(&client, "capture", json!({})).await;
    assert!(
        capture
            .content
            .iter()
            .any(|block| block.as_image().is_some())
    );
    assert!(
        text(&capture).contains("Rust computer proof"),
        "the accessibility tree reaches into the page:\n{}",
        text(&capture)
    );
    let windows = call(&client, "windows", json!({"action":"list"})).await;
    let window_list: Vec<serde_json::Value> =
        serde_json::from_str(&text(&windows)).expect("window JSON");
    assert!(
        !window_list.is_empty(),
        "Chromium is a visible desktop window"
    );

    // The person can close the browser from the viewer; the next browser
    // call must start a fresh one instead of waiting on the old one forever.
    let browser_pid = window_list
        .iter()
        .find(|w| {
            w["class"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("chromium")
        })
        .expect("browser window")["pid"]
        .as_u64()
        .expect("browser PID");
    let closed = call(
        &client,
        "shell",
        json!({"command":"sh","args":["-c",format!("kill {browser_pid}; sleep 2; echo closed")]}),
    )
    .await;
    assert!(text(&closed).contains("closed"), "{}", text(&closed));
    let reopened = call(
        &client,
        "browser",
        json!({"action":"navigate","url":"data:text/html,<h1>Back after close</h1>"}),
    )
    .await;
    assert!(!reopened.is_error.unwrap_or(false), "{}", text(&reopened));
    let page = call(&client, "browser", json!({"action":"text"})).await;
    assert!(text(&page).contains("Back after close"), "{}", text(&page));

    // Destroying the window from outside leaves Chromium running with pages
    // and no window. The desktop reports the loss and the next browser call
    // is a browser someone can see.
    let windows = call(&client, "windows", json!({"action":"list"})).await;
    let window_list: Vec<serde_json::Value> =
        serde_json::from_str(&text(&windows)).expect("window JSON");
    let browser_window = window_list
        .iter()
        .find(|w| {
            w["class"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("chromium")
        })
        .expect("browser window")["id"]
        .as_str()
        .expect("window id")
        .to_owned();
    let closed = call(
        &client,
        "windows",
        json!({"action":"close","window_id":browser_window}),
    )
    .await;
    assert!(!closed.is_error.unwrap_or(false), "{}", text(&closed));
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let reopened = call(
        &client,
        "browser",
        json!({"action":"navigate","url":"data:text/html,<h1>Back after window close</h1>"}),
    )
    .await;
    assert!(!reopened.is_error.unwrap_or(false), "{}", text(&reopened));
    // The X title trails the page load by a beat.
    let mut windows = String::new();
    for _ in 0..20 {
        windows = text(&call(&client, "windows", json!({"action":"list"})).await);
        if windows.contains("Back after window close") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        windows.contains("Back after window close"),
        "a browser window is on the desktop again: {windows}"
    );

    // Hotline mounts a persistent scratch volume here; the image must seed its
    // ownership so the non-root agent can follow the bundled clone recipe.
    let path = "/home/agent/src/contract/probe.txt";
    let put = call(
        &client,
        "files",
        json!({"action":"put","path":path,"content":"over-mcp"}),
    )
    .await;
    assert!(!put.is_error.unwrap_or(false), "{}", text(&put));
    let get = call(&client, "files", json!({"action":"get","path":path})).await;
    assert_eq!(text(&get), "over-mcp");
    let escaped = call(
        &client,
        "files",
        json!({"action":"get","path":"/etc/passwd"}),
    )
    .await;
    assert!(escaped.is_error.unwrap_or(false));
    assert!(text(&escaped).contains("path must be under"));

    let alice = connect(&base, &token, "alice").await;
    let mallory = connect(&base, &token, "mallory").await;
    let taken = call(&alice, "state", json!({"action":"control","duration":60})).await;
    assert!(text(&taken).contains("alice"));
    let blocked = call(&mallory, "input", json!({"action":"key","combo":"Escape"})).await;
    assert!(blocked.is_error.unwrap_or(false));
    assert!(text(&blocked).contains("alice"));
    let released = call(&alice, "state", json!({"action":"release"})).await;
    assert!(text(&released).contains("\"released\":true"));

    // The viewer: the page is open, the socket wants the token, the first
    // frame is the whole screen as PNG, watching costs the teammate nothing,
    // and a person who takes the screen holds it against the agent until
    // they hand it back.
    let page = reqwest::get(format!("{base}/"))
        .await
        .expect("viewer page")
        .text()
        .await
        .expect("viewer html");
    assert!(page.contains("<canvas"), "the viewer page is served at /");
    assert!(
        page.contains("Take control"),
        "the viewer page offers the screen rather than taking it"
    );
    // The Files panel behind the page: the home listed with the token, a
    // file saved as an attachment, nothing above the home, and no listing
    // without the token.
    let http = reqwest::Client::new();
    let listed = http
        .get(format!("{base}/files?token={token}"))
        .send()
        .await
        .expect("files listing");
    assert_eq!(listed.status(), 200);
    let listing: serde_json::Value = listed.json().await.expect("listing json");
    assert_eq!(listing["path"], listing["home"], "no path means the home");
    assert!(listing["entries"].is_array(), "{listing}");
    let put = call(
        &client,
        "files",
        json!({"action":"put","path":"/home/agent/src/viewer-download.txt","content":"saved on the person's computer"}),
    )
    .await;
    assert!(!put.is_error.unwrap_or(false), "{}", text(&put));
    let saved = http
        .get(format!(
            "{base}/files/download?token={token}&path=/home/agent/src/viewer-download.txt"
        ))
        .send()
        .await
        .expect("download");
    assert_eq!(saved.status(), 200);
    assert!(
        saved.headers()[reqwest::header::CONTENT_DISPOSITION]
            .to_str()
            .expect("ascii disposition")
            .contains("attachment; filename=\"viewer-download.txt\""),
        "{:?}",
        saved.headers()
    );
    assert_eq!(
        saved.text().await.expect("file body"),
        "saved on the person's computer"
    );
    let sent = http
        .post(format!(
            "{base}/files?token={token}&path=/home/agent/src/viewer-upload/notes.txt"
        ))
        .body("from the person's computer")
        .send()
        .await
        .expect("upload");
    assert_eq!(
        sent.status(),
        200,
        "{}",
        sent.text().await.unwrap_or_default()
    );
    let landed = call(
        &client,
        "files",
        json!({"action":"get","path":"/home/agent/src/viewer-upload/notes.txt"}),
    )
    .await;
    assert_eq!(text(&landed), "from the person's computer");
    let astray = http
        .post(format!("{base}/files?token={token}&path=/tmp/astray.txt"))
        .body("nowhere")
        .send()
        .await
        .expect("upload outside the home");
    assert_eq!(astray.status(), 400, "only the home takes a file");
    let above = http
        .get(format!("{base}/files?token={token}&path=/etc"))
        .send()
        .await
        .expect("listing outside the home");
    assert_eq!(above.status(), 400, "only the home is listed");
    // A caller that can set its own headers — the desk, not the viewer
    // page's browser fetches — may present the bearer as an Authorization
    // header instead of the query token, so it never rides in the URL.
    let listed_by_header = http
        .get(format!("{base}/files"))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .expect("files listing over a header");
    assert_eq!(
        listed_by_header.status(),
        200,
        "the header is an accepted alternative to the query token"
    );
    let sent_by_header = http
        .post(format!(
            "{base}/files?path=/home/agent/src/viewer-upload/from-header.txt"
        ))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body("carried in a header, not a URL")
        .send()
        .await
        .expect("upload over a header");
    assert_eq!(
        sent_by_header.status(),
        200,
        "{}",
        sent_by_header.text().await.unwrap_or_default()
    );
    let landed_by_header = call(
        &client,
        "files",
        json!({"action":"get","path":"/home/agent/src/viewer-upload/from-header.txt"}),
    )
    .await;
    assert_eq!(text(&landed_by_header), "carried in a header, not a URL");

    // The desk's cookie import, then the operator taking it back: a saved
    // login is uploaded and loaded the way the desk does it, and
    // `DELETE /logins/{name}` drops one domain from the browser and the
    // document, then the rest, after which the document is gone.
    let brought = json!({
        "name": "import-contract-Default", "browser": "chrome",
        "created_at": "2026-09-21T00:00:00Z",
        "cookies": [
            {"name": "session", "value": "one", "domain": ".example.com", "path": "/", "secure": true},
            {"name": "sid", "value": "two", "domain": "api.example.com", "path": "/", "secure": true},
            {"name": "other", "value": "three", "domain": ".example.org", "path": "/", "secure": true}
        ],
        "storage": {}
    });
    let uploaded = http
        .post(format!(
            "{base}/files?path=/home/agent/.hotline/logins/import-contract-Default.json"
        ))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(brought.to_string())
        .send()
        .await
        .expect("upload a saved login");
    assert_eq!(
        uploaded.status(),
        200,
        "{}",
        uploaded.text().await.unwrap_or_default()
    );
    let loaded = call(
        &client,
        "state",
        json!({"action":"login_load","name":"import-contract-Default"}),
    )
    .await;
    assert!(
        text(&loaded).contains("\"loaded\":true"),
        "{}",
        text(&loaded)
    );
    let unauthorized = http
        .delete(format!("{base}/logins/import-contract-Default"))
        .send()
        .await
        .expect("forget without a bearer");
    assert_eq!(
        unauthorized.status(),
        401,
        "the logins door wants the bearer"
    );
    let one_site = http
        .delete(format!("{base}/logins/import-contract-Default"))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .json(&json!({"domains": ["example.com"]}))
        .send()
        .await
        .expect("forget one domain");
    assert_eq!(one_site.status(), 200);
    let answered: serde_json::Value = one_site.json().await.expect("a forget answer");
    assert_eq!(answered["forgotten"], 2, "{answered}");
    assert_eq!(answered["kept"], 1, "{answered}");
    let saved_after = call(
        &client,
        "state",
        json!({"action":"login_save","name":"contract-after-forget"}),
    )
    .await;
    assert!(
        text(&saved_after).contains("\"saved\":true"),
        "{}",
        text(&saved_after)
    );
    let after = call(
        &client,
        "files",
        json!({"action":"get","path":"/home/agent/.hotline/logins/contract-after-forget.json"}),
    )
    .await;
    let after_text = text(&after);
    assert!(
        !after_text.contains("example.com"),
        "the browser dropped every example.com cookie: {after_text}"
    );
    assert!(
        after_text.contains("example.org"),
        "and kept the other site's: {after_text}"
    );
    let document = call(
        &client,
        "files",
        json!({"action":"get","path":"/home/agent/.hotline/logins/import-contract-Default.json"}),
    )
    .await;
    assert!(
        !text(&document).contains("example.com") && text(&document).contains("example.org"),
        "the saved login lost the domain too: {}",
        text(&document)
    );
    let the_rest = http
        .delete(format!("{base}/logins/import-contract-Default"))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .json(&json!({"domains": ["example.org"]}))
        .send()
        .await
        .expect("forget the rest");
    assert_eq!(the_rest.status(), 200);
    let gone = call(
        &client,
        "files",
        json!({"action":"get","path":"/home/agent/.hotline/logins/import-contract-Default.json"}),
    )
    .await;
    assert!(
        gone.is_error.unwrap_or(false),
        "a login with nothing left is removed: {}",
        text(&gone)
    );
    let nothing = http
        .delete(format!("{base}/logins/import-contract-Default"))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .expect("forget what is gone");
    assert_eq!(
        nothing.status(),
        404,
        "no saved login, nothing named: nothing to forget"
    );
    if !token.is_empty() {
        let wrong_header = http
            .get(format!("{base}/files"))
            .header(AUTHORIZATION, "Bearer not-the-token")
            .send()
            .await
            .expect("listing with the wrong header");
        assert_eq!(
            wrong_header.status(),
            401,
            "a wrong header is still refused"
        );
    }
    if !token.is_empty() {
        let refused = http
            .get(format!("{base}/files"))
            .send()
            .await
            .expect("listing without the token");
        assert_eq!(refused.status(), 401, "the listing wants the token");
    }
    // The person's stored secrets: the desk puts the whole set in with the
    // bearer, a job finds each by name, and nothing answers a value.
    let door = format!("{base}/secrets");
    let value = "contract-secret-value-0001";
    if !token.is_empty() {
        let naked = http
            .put(&door)
            .json(&json!({"CONTRACT_SECRET": value}))
            .send()
            .await
            .expect("put without the bearer");
        assert_eq!(naked.status(), 401, "the secrets door wants the bearer");
    }
    let taken = http
        .put(&door)
        .bearer_auth(&token)
        .json(&json!({"CONTRACT_SECRET": value}))
        .send()
        .await
        .expect("put secrets");
    assert_eq!(
        taken.status(),
        204,
        "{}",
        taken.text().await.unwrap_or_default()
    );
    let printed = call(
        &client,
        "shell",
        json!({"command":"sh","args":["-c","printf '%s' \"$CONTRACT_SECRET\""]}),
    )
    .await;
    assert!(
        text(&printed).contains("[redacted CONTRACT_SECRET]"),
        "a job finds the secret and the answer names it: {}",
        text(&printed)
    );
    assert!(
        !text(&printed).contains(value),
        "the value never comes back: {}",
        text(&printed)
    );
    let info = call(&client, "state", json!({"action":"info"})).await;
    let report: serde_json::Value = serde_json::from_str(&text(&info)).expect("info is JSON");
    assert_eq!(
        report["secrets"],
        json!([{"name": "CONTRACT_SECRET", "kind": "variable"}])
    );
    assert!(!text(&info).contains(value), "info names, never values");
    let asked = http
        .get(&door)
        .bearer_auth(&token)
        .send()
        .await
        .expect("get secrets");
    assert_eq!(asked.status(), 405, "nothing answers a value");
    let refused = http
        .put(&door)
        .bearer_auth(&token)
        .json(&json!({"contract-secret": value}))
        .send()
        .await
        .expect("put a name that is not a variable");
    assert_eq!(refused.status(), 400);
    let cleared = http
        .put(&door)
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .expect("clear secrets");
    assert_eq!(cleared.status(), 204);
    let gone = call(
        &client,
        "shell",
        json!({"command":"sh","args":["-c","printf '%s' \"${CONTRACT_SECRET-absent}\""]}),
    )
    .await;
    assert!(text(&gone).contains("absent"), "{}", text(&gone));

    // A login is typed by name onto its own site alone, and a passkey is the
    // teammate's own: made once while the person has armed the site, kept
    // by the desk, signing in by itself, gone when the grant goes. The site
    // is a page the computer serves to itself.
    let served = call(
        &client,
        "files",
        json!({"action":"put","path":"/home/agent/src/contract/proof/passkey.html","content":include_str!("passkey-proof.html")}),
    )
    .await;
    assert!(!served.is_error.unwrap_or(false), "{}", text(&served));
    let server = call(
        &client,
        "shell",
        json!({"action":"start","command":"python3","args":["-m","http.server","8123","--bind","127.0.0.1","--directory","/home/agent/src/contract/proof"],"label":"Serve the passkey proof"}),
    )
    .await;
    assert!(!server.is_error.unwrap_or(false), "{}", text(&server));
    let mut opened = String::new();
    for _ in 0..40 {
        let navigated = call(
            &client,
            "browser",
            json!({"action":"navigate","url":"http://localhost:8123/passkey.html"}),
        )
        .await;
        if !navigated.is_error.unwrap_or(false) {
            opened = text(
                &call(
                    &client,
                    "browser",
                    json!({"action":"eval","js":"document.title"}),
                )
                .await,
            );
            if opened.contains("Passkey proof") {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        opened.contains("Passkey proof"),
        "the proof page is served: {opened}"
    );
    let registration = format!("{base}/passkeys/registration");
    if !token.is_empty() {
        let naked = http
            .get(&registration)
            .send()
            .await
            .expect("registration without the bearer");
        assert_eq!(
            naked.status(),
            401,
            "the registration door wants the bearer"
        );
    }
    let idle = http
        .get(&registration)
        .bearer_auth(&token)
        .send()
        .await
        .expect("registration state")
        .json::<serde_json::Value>()
        .await
        .expect("registration JSON");
    assert_eq!(idle, json!({"state": "idle"}));
    // Armed for another site, this one may not make a passkey.
    let elsewhere = http
        .put(&registration)
        .bearer_auth(&token)
        .json(&json!({"rpId": "example.com"}))
        .send()
        .await
        .expect("arm for another site");
    assert_eq!(
        elsewhere.status(),
        200,
        "{}",
        elsewhere.text().await.unwrap_or_default()
    );
    let refused = text(&call(&client, "browser", json!({"action":"eval","js":"make()"})).await);
    assert!(
        refused.contains("refused NotAllowedError") && refused.contains("not armed"),
        "a passkey for a site that is not armed is refused: {refused}"
    );
    let armed = http
        .put(&registration)
        .bearer_auth(&token)
        .json(&json!({"rpId": "localhost"}))
        .send()
        .await
        .expect("arm for the proof site")
        .json::<serde_json::Value>()
        .await
        .expect("arming JSON");
    assert_eq!(armed["state"], "armed", "{armed}");
    assert_eq!(armed["rpId"], "localhost");
    assert!(armed["expiresAt"].as_i64().unwrap_or_default() > 0);
    // Under the arming, the site's request waits for the person: nothing
    // is made until the desk carries their answer back, and the door says
    // what the site asked for meanwhile.
    let parked = text(
        &call(
            &client,
            "browser",
            json!({"action":"eval","js":"window.making = make(); 'parked'"}),
        )
        .await,
    );
    assert!(parked.contains("parked"), "{parked}");
    let again = text(&call(&client, "browser", json!({"action":"eval","js":"make()"})).await);
    assert!(
        again.contains("refused NotAllowedError") && again.contains("waiting"),
        "one request before the person at a time: {again}"
    );
    let asked = http
        .get(&registration)
        .bearer_auth(&token)
        .send()
        .await
        .expect("registration state")
        .json::<serde_json::Value>()
        .await
        .expect("registration JSON");
    assert_eq!(asked["state"], "asked", "{asked}");
    assert_eq!(asked["rpId"], "localhost");
    assert_eq!(asked["ask"]["rpId"], "localhost");
    assert_eq!(asked["ask"]["origin"], "http://localhost:8123");
    assert_eq!(asked["ask"]["rpName"], "Passkey proof");
    assert_eq!(asked["ask"]["userName"], "teammate");
    assert_eq!(asked["ask"]["userDisplayName"], "The teammate");
    assert!(asked["ask"]["askedAt"].as_i64().unwrap_or_default() > 0);
    assert!(asked.get("credential").is_none(), "{asked}");
    let ask_id = asked["ask"]["id"].as_str().expect("ask id").to_owned();
    let answer = format!("{registration}/answer");
    // An answer to a request that is not waiting is refused.
    let stale = http
        .post(&answer)
        .bearer_auth(&token)
        .json(&json!({"id": "nope", "approved": true}))
        .send()
        .await
        .expect("answer a request that is not waiting");
    assert_eq!(
        stale.status(),
        409,
        "{}",
        stale.text().await.unwrap_or_default()
    );
    // Denied, the site hears no, nothing was made, and the arming is over.
    let denied = http
        .post(&answer)
        .bearer_auth(&token)
        .json(&json!({"id": ask_id, "approved": false}))
        .send()
        .await
        .expect("deny the request")
        .json::<serde_json::Value>()
        .await
        .expect("denial JSON");
    assert_eq!(denied, json!({"state": "idle"}), "{denied}");
    let refused = text(
        &call(
            &client,
            "browser",
            json!({"action":"eval","js":"window.making"}),
        )
        .await,
    );
    assert!(
        refused.contains("refused NotAllowedError") && refused.contains("did not approve"),
        "a denied request is refused to the site: {refused}"
    );
    let refused = text(&call(&client, "browser", json!({"action":"eval","js":"make()"})).await);
    assert!(
        refused.contains("not armed"),
        "a denial ends the arming: {refused}"
    );
    // Armed again and approved, the browser makes it.
    let armed = http
        .put(&registration)
        .bearer_auth(&token)
        .json(&json!({"rpId": "localhost"}))
        .send()
        .await
        .expect("arm for the proof site again")
        .json::<serde_json::Value>()
        .await
        .expect("arming JSON");
    assert_eq!(armed["state"], "armed", "{armed}");
    let parked = text(
        &call(
            &client,
            "browser",
            json!({"action":"eval","js":"window.making = make(); 'parked'"}),
        )
        .await,
    );
    assert!(parked.contains("parked"), "{parked}");
    let asked = http
        .get(&registration)
        .bearer_auth(&token)
        .send()
        .await
        .expect("registration state")
        .json::<serde_json::Value>()
        .await
        .expect("registration JSON");
    assert_eq!(asked["state"], "asked", "{asked}");
    let ask_id = asked["ask"]["id"].as_str().expect("ask id").to_owned();
    assert!(
        asked["ask"]["askedAt"].as_i64().unwrap_or_default() > 0,
        "{asked}"
    );
    let approved = http
        .post(&answer)
        .bearer_auth(&token)
        .json(&json!({"id": ask_id, "approved": true}))
        .send()
        .await
        .expect("approve the request")
        .json::<serde_json::Value>()
        .await
        .expect("approval JSON");
    assert!(
        approved["state"] == "approved" || approved["state"] == "registered",
        "{approved}"
    );
    assert_eq!(approved["ask"]["id"], ask_id);
    let made = text(
        &call(
            &client,
            "browser",
            json!({"action":"eval","js":"window.making"}),
        )
        .await,
    );
    assert!(
        made.starts_with("\"made "),
        "the approved request makes a passkey: {made}"
    );
    let registered = http
        .get(&registration)
        .bearer_auth(&token)
        .send()
        .await
        .expect("registration state")
        .json::<serde_json::Value>()
        .await
        .expect("registration JSON");
    assert_eq!(registered["state"], "registered", "{registered}");
    assert_eq!(registered["ask"]["id"], ask_id);
    let credential = registered["credential"].clone();
    assert_eq!(credential["rpId"], "localhost");
    assert_eq!(credential["userName"], "teammate");
    assert!(
        credential["privateKey"]
            .as_str()
            .is_some_and(|key| key.len() > 40)
    );
    let credential_id = credential["credentialId"]
        .as_str()
        .expect("credential id")
        .to_owned();
    let ended = http
        .delete(&registration)
        .bearer_auth(&token)
        .send()
        .await
        .expect("end the arming");
    assert_eq!(ended.status(), 204);
    // Disarmed, the page may not make another, and what it made is gone
    // until the desk delivers it back as a stored passkey.
    let refused = text(&call(&client, "browser", json!({"action":"eval","js":"make()"})).await);
    assert!(refused.contains("not armed"), "{refused}");
    let unsigned = text(&call(&client, "browser", json!({"action":"eval","js":"sign()"})).await);
    assert!(
        unsigned.contains("refused"),
        "a passkey nobody stored does not sign: {unsigned}"
    );
    let stored = http
        .put(&door)
        .bearer_auth(&token)
        .json(&json!({
            "PROOF_PASSKEY": credential,
            "PROOF": {"kind": "login", "sites": ["http://localhost:8123"], "username": "teammate",
                      "password": "correct horse battery staple", "totp": "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"},
            "PROOF_TOKEN": "contract-token-value-0002",
        }))
        .send()
        .await
        .expect("store the passkey and a login");
    assert_eq!(
        stored.status(),
        204,
        "{}",
        stored.text().await.unwrap_or_default()
    );
    let signed = text(&call(&client, "browser", json!({"action":"eval","js":"sign()"})).await);
    assert!(
        signed.contains("signed "),
        "the stored passkey signs in: {signed}"
    );
    let info = call(&client, "state", json!({"action":"info"})).await;
    let report: serde_json::Value = serde_json::from_str(&text(&info)).expect("info is JSON");
    assert_eq!(
        report["secrets"],
        json!([
            {"name": "PROOF", "kind": "login", "sites": ["http://localhost:8123"], "username": "teammate", "totp": true},
            {"name": "PROOF_PASSKEY", "kind": "passkey", "rpId": "localhost", "userName": "teammate"},
            {"name": "PROOF_TOKEN", "kind": "variable"},
        ])
    );
    assert!(!text(&info).contains("battery"), "info names, never values");
    let snapshot = text(&call(&client, "browser", json!({"action":"text"})).await);
    let field = |label: &str| {
        snapshot
            .lines()
            .find(|line| line.starts_with("[e") && line.contains(label))
            .and_then(|line| line.split(']').next())
            .map(|reference| reference.trim_start_matches('[').to_owned())
            .unwrap_or_else(|| panic!("no {label} field in {snapshot}"))
    };
    let (user, pass, code) = (field("User"), field("Password"), field("Code"));
    let filled = call(
        &client,
        "browser",
        json!({"action":"fill","ref":user,"secret":"PROOF.username"}),
    )
    .await;
    assert_eq!(text(&filled), format!("filled {user} with PROOF.username"));
    let filled = call(
        &client,
        "browser",
        json!({"action":"fill","ref":pass,"secret":"PROOF.password"}),
    )
    .await;
    assert_eq!(text(&filled), format!("filled {pass} with PROOF.password"));
    let filled = call(
        &client,
        "browser",
        json!({"action":"fill","ref":code,"secret":"PROOF.code"}),
    )
    .await;
    assert_eq!(text(&filled), format!("filled {code} with PROOF.code"));
    let typed = text(
        &call(
            &client,
            "browser",
            json!({"action":"eval","js":"[user.value, pass.value, code.value]"}),
        )
        .await,
    );
    let typed: Vec<String> = serde_json::from_str(&typed).unwrap_or_else(|_| panic!("{typed}"));
    assert_eq!(typed[0], "teammate");
    assert_eq!(
        typed[1], "[redacted PROOF.password]",
        "a value read back is redacted"
    );
    assert!(
        typed[2].len() == 6 && typed[2].chars().all(|c| c.is_ascii_digit()),
        "a code is six digits: {}",
        typed[2]
    );
    let both = call(
        &client,
        "browser",
        json!({"action":"fill","ref":user,"secret":"PROOF.username","text":"x"}),
    )
    .await;
    assert!(both.is_error.unwrap_or(false), "{}", text(&both));
    let unknown = call(
        &client,
        "browser",
        json!({"action":"fill","ref":user,"secret":"PROOF.token"}),
    )
    .await;
    assert!(text(&unknown).contains("not token"), "{}", text(&unknown));
    let passkey = call(
        &client,
        "browser",
        json!({"action":"fill","ref":user,"secret":"PROOF_PASSKEY"}),
    )
    .await;
    assert!(
        text(&passkey).contains("signs in by itself"),
        "{}",
        text(&passkey)
    );
    // The same page by another name is another site.
    let elsewhere = call(
        &client,
        "browser",
        json!({"action":"navigate","url":"http://127.0.0.1:8123/passkey.html"}),
    )
    .await;
    assert!(!elsewhere.is_error.unwrap_or(false), "{}", text(&elsewhere));
    let snapshot = text(&call(&client, "browser", json!({"action":"text"})).await);
    let pass = snapshot
        .lines()
        .find(|line| line.starts_with("[e") && line.contains("Password"))
        .and_then(|line| line.split(']').next())
        .map(|reference| reference.trim_start_matches('[').to_owned())
        .expect("password field");
    let refused = call(
        &client,
        "browser",
        json!({"action":"fill","ref":pass,"secret":"PROOF.password"}),
    )
    .await;
    assert!(refused.is_error.unwrap_or(false), "{}", text(&refused));
    assert!(
        text(&refused).contains("typed only on http://localhost:8123")
            && text(&refused).contains("http://127.0.0.1:8123"),
        "{}",
        text(&refused)
    );
    let variable = call(
        &client,
        "browser",
        json!({"action":"fill","ref":pass,"secret":"PROOF_TOKEN"}),
    )
    .await;
    assert_eq!(
        text(&variable),
        format!("filled {pass} with PROOF_TOKEN"),
        "a variable has no site to keep to"
    );
    let nothing_leaked = text(
        &call(
            &client,
            "browser",
            json!({"action":"eval","js":"pass.value"}),
        )
        .await,
    );
    assert_eq!(nothing_leaked, "\"[redacted PROOF_TOKEN]\"");
    // Revoked, the passkey is gone from the browser at once.
    let revoked = http
        .put(&door)
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .expect("revoke everything");
    assert_eq!(revoked.status(), 204);
    let navigated = call(
        &client,
        "browser",
        json!({"action":"navigate","url":"http://localhost:8123/passkey.html"}),
    )
    .await;
    assert!(!navigated.is_error.unwrap_or(false), "{}", text(&navigated));
    let unsigned = text(&call(&client, "browser", json!({"action":"eval","js":"sign()"})).await);
    assert!(
        unsigned.contains("refused"),
        "a revoked passkey no longer signs: {unsigned}"
    );
    assert!(!unsigned.contains(&credential_id), "{unsigned}");
    let stopped = call(
        &client,
        "shell",
        json!({"action":"cancel","job_id":serde_json::from_str::<serde_json::Value>(&text(&server)).map(|job| job["id"].as_str().unwrap_or_default().to_owned()).unwrap_or_default()}),
    )
    .await;
    assert!(!stopped.is_error.unwrap_or(false), "{}", text(&stopped));
    let ws_base = base.replacen("http", "ws", 1);
    if !token.is_empty() {
        let refused = tokio_tungstenite::connect_async(format!("{ws_base}/ws")).await;
        assert!(
            matches!(
                refused,
                Err(tokio_tungstenite::tungstenite::Error::Http(ref response))
                    if response.status() == 401
            ),
            "the socket refuses a missing token"
        );
    }
    let typing = call(
        &client,
        "browser",
        json!({"action":"navigate","url":"data:text/html,<title>Typing</title><textarea id=t autofocus></textarea>"}),
    )
    .await;
    assert!(!typing.is_error.unwrap_or(false), "{}", text(&typing));
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The agent types through XTEST: lower case, shifted letters, symbols on
    // shifted keys, and a character with no key of its own.
    let typed = call(
        &client,
        "input",
        json!({"action":"type","text":"Hi! a_b@c é"}),
    )
    .await;
    assert!(!typed.is_error.unwrap_or(false), "{}", text(&typed));
    let value = call(
        &client,
        "browser",
        json!({"action":"eval","js":"document.getElementById('t').value"}),
    )
    .await;
    assert!(
        text(&value).contains("Hi! a_b@c é"),
        "the agent's typing reached the page: {}",
        text(&value)
    );
    // A chord: select all, then a clipboard copy Chromium serves.
    let all = call(&client, "input", json!({"action":"key","combo":"Ctrl+A"})).await;
    assert!(!all.is_error.unwrap_or(false), "{}", text(&all));
    let copy = call(&client, "input", json!({"action":"key","combo":"ctrl+c"})).await;
    assert!(!copy.is_error.unwrap_or(false), "{}", text(&copy));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let read = call(&client, "input", json!({"action":"clipboard_read"})).await;
    assert!(
        text(&read).contains("Hi! a_b@c é"),
        "the agent reads what Chromium put on the clipboard: {}",
        text(&read)
    );
    // The other way: the agent owns the clipboard and Chromium pastes from it.
    let pasted = call(
        &client,
        "input",
        json!({"action":"paste","text":"pasted from the agent ✓"}),
    )
    .await;
    assert!(!pasted.is_error.unwrap_or(false), "{}", text(&pasted));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let value = call(
        &client,
        "browser",
        json!({"action":"eval","js":"document.getElementById('t').value"}),
    )
    .await;
    assert!(
        text(&value).contains("pasted from the agent ✓"),
        "Chromium pasted what the agent owns: {}",
        text(&value)
    );
    let written = call(
        &client,
        "input",
        json!({"action":"clipboard_write","text":"round trip"}),
    )
    .await;
    assert!(!written.is_error.unwrap_or(false), "{}", text(&written));
    let read = call(&client, "input", json!({"action":"clipboard_read"})).await;
    assert_eq!(text(&read), "round trip");

    // Window placement goes through the window manager: tile restores the
    // window from maximized and puts it where asked; focus is honoured.
    let windows = call(&client, "windows", json!({"action":"list"})).await;
    let window_list: Vec<serde_json::Value> =
        serde_json::from_str(&text(&windows)).expect("window JSON");
    let browser_window = window_list
        .iter()
        .find(|w| {
            w["class"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("chromium")
        })
        .expect("browser window")["id"]
        .as_str()
        .expect("window id")
        .to_owned();
    call(&client, "shell", json!({"action":"show"})).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let tiled = call(&client, "windows", json!({"action":"tile"})).await;
    assert!(!tiled.is_error.unwrap_or(false), "{}", text(&tiled));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let windows = call(&client, "windows", json!({"action":"list"})).await;
    let window_list: Vec<serde_json::Value> =
        serde_json::from_str(&text(&windows)).expect("window JSON");
    let bounds = |class: &str| {
        let w = window_list
            .iter()
            .find(|w| {
                w["class"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(class)
            })
            .unwrap_or_else(|| panic!("a {class} window: {windows:?}"));
        (
            w["bounds"][0].as_i64().expect("x"),
            w["bounds"][2].as_i64().expect("width"),
        )
    };
    let (browser_x, browser_width) = bounds("chromium");
    let (observer_x, _) = bounds("hotlineterminal");
    assert!(
        browser_x == 0 && (1240..=1320).contains(&browser_width),
        "the app takes the left two thirds of a 1920 screen: {windows:?}"
    );
    assert_eq!(
        observer_x, browser_width,
        "the observer takes the right third: {windows:?}"
    );
    let focused = call(
        &client,
        "windows",
        json!({"action":"focus","window_id":browser_window}),
    )
    .await;
    assert!(!focused.is_error.unwrap_or(false), "{}", text(&focused));
    // Repeated restore/resize cycles expose stale client geometry requests.
    for _ in 0..12 {
        let maximized = call(
            &client,
            "windows",
            json!({"action":"maximize","window_id":browser_window}),
        )
        .await;
        assert!(!maximized.is_error.unwrap_or(false), "{}", text(&maximized));
        let tiled = call(&client, "windows", json!({"action":"tile"})).await;
        assert!(!tiled.is_error.unwrap_or(false), "{}", text(&tiled));
    }
    let maximized = call(
        &client,
        "windows",
        json!({"action":"maximize","window_id":browser_window}),
    )
    .await;
    assert!(!maximized.is_error.unwrap_or(false), "{}", text(&maximized));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let (mut socket, _) = tokio_tungstenite::connect_async(format!("{ws_base}/ws?token={token}"))
        .await
        .expect("viewer socket");
    // The socket hears whose screen this is before it sees the screen.
    let first = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = socket.next().await.expect("a frame").expect("a frame");
            if message.is_binary() {
                return message;
            }
        }
    })
    .await
    .expect("a frame within five seconds");
    let bytes = first.into_data();
    let header: Vec<u16> = (0..6)
        .map(|index| u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]))
        .collect();
    assert_eq!(
        &header[..4],
        &[0, 0, header[4], header[5]],
        "the first frame is the whole screen"
    );
    assert_eq!(&bytes[12..16], b"\x89PNG", "the frame is a PNG");

    // A socket that has not taken the screen only watches: its input never
    // reaches the hands, so the teammate keeps a machine someone is looking at.
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"move","x":40,"y":40}).to_string(),
        ))
        .await
        .expect("send move");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let watched = call(&client, "input", json!({"action":"key","combo":"Escape"})).await;
    assert!(
        !watched.is_error.unwrap_or(false),
        "watching leaves the machine to the agent: {}",
        text(&watched)
    );

    // Paste is explicit and a view-only connection cannot change the clipboard.
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"paste","text":"denied"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(viewer_reply(&mut socket).await["t"], "error");
    call(&client, "browser", json!({"action":"eval","js":"document.getElementById('t').value='';document.getElementById('t').focus();setInterval(()=>document.body.style.backgroundColor = Date.now()%2 ? '#eee' : '#fff',100)"})).await;
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"control","take":true}).to_string(),
        ))
        .await
        .unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"paste","text":"host clipboard café 🐸\nsecond line"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(viewer_reply(&mut socket).await["ok"], true);
    let held = call(&client, "input", json!({"action":"key","combo":"Escape"})).await;
    assert!(held.is_error.unwrap_or(false), "{}", text(&held));
    assert!(text(&held).contains("person"));
    // Capture remains available while the viewer owns control.
    let captured = call(&client, "capture", json!({})).await;
    assert!(!captured.is_error.unwrap_or(false));
    let (mut newer, _) = tokio_tungstenite::connect_async(format!("{ws_base}/ws?token={token}"))
        .await
        .unwrap();
    newer
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"control","take":true}).to_string(),
        ))
        .await
        .unwrap();
    newer
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"paste","text":" + newer viewer"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(viewer_reply(&mut newer).await["ok"], true);
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"paste","text":"stale viewer must not paste"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(viewer_reply(&mut socket).await["t"], "error");
    socket.close(None).await.unwrap();
    let held = call(&client, "input", json!({"action":"key","combo":"Escape"})).await;
    assert!(
        held.is_error.unwrap_or(false),
        "closing the old viewer must not release the newer lease"
    );
    newer.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let typed = call(
        &client,
        "browser",
        json!({"action":"eval","js":"document.getElementById('t').value"}),
    )
    .await;
    assert_eq!(
        text(&typed),
        "\"host clipboard café 🐸\\nsecond line + newer viewer\""
    );
    // A viewer that goes away with a modifier down must not leave it down:
    // a person's host shortcut can take the page's focus between the two.
    let (mut vanishing, _) =
        tokio_tungstenite::connect_async(format!("{ws_base}/ws?token={token}"))
            .await
            .unwrap();
    for message in [
        json!({"t":"control","take":true}),
        json!({"t":"key","key":"Alt","down":true}),
    ] {
        vanishing
            .send(tokio_tungstenite::tungstenite::Message::text(
                message.to_string(),
            ))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    vanishing.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let keymap = call(
        &client,
        "shell",
        json!({"command":"python3","args":["-c",KEYMAP_QUERY]}),
    )
    .await;
    assert!(
        text(&keymap).contains("held keys: none"),
        "the hands let go when a viewer vanishes: {}",
        text(&keymap)
    );
    let (mut reconnected, _) =
        tokio_tungstenite::connect_async(format!("{ws_base}/ws?token={token}"))
            .await
            .unwrap();
    reconnected
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({"t":"paste","text":"reconnect must watch"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(viewer_reply(&mut reconnected).await["t"], "error");
    let handed = call(&client, "input", json!({"action":"key","combo":"Escape"})).await;
    assert!(
        !handed.is_error.unwrap_or(false),
        "disconnect releases control"
    );
    reconnected.close(None).await.unwrap();

    client.cancel().await.ok();
    alice.cancel().await.ok();
    mallory.cancel().await.ok();
}

/// Asks the X server which keys it considers down.
const KEYMAP_QUERY: &str = r#"
import ctypes
x = ctypes.CDLL('libX11.so.6')
x.XOpenDisplay.restype = ctypes.c_void_p
d = x.XOpenDisplay(None)
keys = (ctypes.c_char * 32)()
x.XQueryKeymap(ctypes.c_void_p(d), keys)
held = [byte * 8 + bit for byte in range(32) for bit in range(8) if keys[byte][0] & (1 << bit)]
print('held keys:', held or 'none')
"#;

async fn viewer_reply(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = socket
                .next()
                .await
                .expect("viewer response")
                .expect("viewer response");
            if message.is_text() {
                let reply: serde_json::Value =
                    serde_json::from_str(message.to_text().unwrap()).unwrap();
                // The bar's state and the hands' pointer are not replies.
                if reply["t"] != "state" && reply["t"] != "pointer" {
                    return reply;
                }
            }
        }
    })
    .await
    .expect("viewer reply within five seconds")
}

async fn connect(base: &str, token: &str, holder: &str) -> Client {
    let mut headers = HeaderMap::new();
    if !token.is_empty() {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    headers.insert(
        HeaderName::from_static("x-computer-holder"),
        HeaderValue::from_str(holder).unwrap(),
    );
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        http,
        StreamableHttpClientTransportConfig::with_uri(format!("{base}/mcp")),
    );
    ClientInfo::new(
        Default::default(),
        Implementation::new("hotline-computer-contract", "1"),
    )
    .serve(transport)
    .await
    .expect("MCP handshake")
}

async fn call(
    client: &Client,
    name: &str,
    arguments: serde_json::Value,
) -> rmcp::model::CallToolResult {
    client
        .peer()
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(arguments.as_object().expect("object").clone()),
        )
        .await
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|content| content.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}
