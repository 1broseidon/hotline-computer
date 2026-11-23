"""Writes the deliberate breaks into a copy of the tree (never a release):
each one turns off the guard a new probe is about."""
import sys, os
os.chdir(sys.argv[1])
def edit(p, old, new):
    s = open(p).read(); assert old in s, (p, old[:60]); open(p, "w").write(s.replace(old, new, 1))
B = "// SABOTAGE (rc3-probes broken build, never a release): "
OFF = 'std::env::var_os("HOTLINE_SABOTAGE_OFF")'
# A3s-x, F1a-x, F1b-x, F6-x: the extension's fill withholds nothing.
edit("src/signin/mod.rs", '''    pub async fn withhold_origin(&self, origin: &str, name: &str, values: &[String]) {
        let running''', f'''    pub async fn withhold_origin(&self, origin: &str, name: &str, values: &[String]) {{
        {B}the extension's fill withholds nothing.
        if {OFF}.is_none() {{
            let _ = (origin, name, values);
            return;
        }}
        let running''')
# F1b-x: the back-forward cache is on.
edit("src/signin/mod.rs", '''            "--disable-features=BackForwardCache,MediaRouter,OptimizationHints,Translate",''',
     f'''            {B}the back-forward cache is on.
            "--disable-features=MediaRouter,OptimizationHints,Translate",''')
# F8i, F8ir: a revocation while stopped is dropped.
edit("src/signin/mod.rs", '''        let Some(running) = running.filter(|running| !running.cdp.is_closed()) else {
            self.0''', f'''        let Some(running) = running.filter(|running| !running.cdp.is_closed()) else {{
            {B}a revocation while stopped is dropped.
            if {OFF}.is_none() {{
                return Ok((0, 0));
            }}
            self.0''')
# F8r: a revocation while running leaves the cookies.
edit("src/signin/mod.rs", '''        if !gone.is_empty() {
            cdp.call("Storage.clearCookies", json!({})).await?;''', f'''        {B}a revocation while running leaves the cookies.
        if !gone.is_empty() && {OFF}.is_some() {{
            cdp.call("Storage.clearCookies", json!({{}})).await?;''')
# F1a-x: nothing is redacted.
edit("src/secrets.rs", '''    fn redact_at<'a>(&self, text: &'a str, now: u64) -> Cow<'a, str> {
''', f'''    fn redact_at<'a>(&self, text: &'a str, now: u64) -> Cow<'a, str> {{
        {B}nothing is redacted.
        if {OFF}.is_none() {{
            let _ = now;
            return Cow::Borrowed(text);
        }}
''')
# AskRevoke: answers outlive their key.
edit("src/holds.rs", '''    pub fn forget(&self, names: &[String]) {
        if names.is_empty() {''', f'''    pub fn forget(&self, names: &[String]) {{
        {B}answers outlive their key.
        if names.is_empty() || {OFF}.is_none() {{''')
# F2m: no minting route is refused.
edit("src/keyproxy/conn.rs", '''        if swapped && self.table().refused(&names, &self.origin, parts.uri.path()) {''',
     f'''        {B}no minting route is refused.
        if swapped && {OFF}.is_some() && self.table().refused(&names, &self.origin, parts.uri.path()) {{''')
# F2x: the owner check is skipped.
edit("src/keyproxy/conn.rs", '''            && !check(peer, job.clone()).await''', f'''            {B}the owner check is skipped.
            && {OFF}.is_some()
            && !check(peer, job.clone()).await''')
# X8: a stray listener of the broker's own.
edit("src/split/broker.rs", '''        app.signin = Some(crate::signin::split::for_broker(&app));''', f'''        app.signin = Some(crate::signin::split::for_broker(&app));
        {B}a stray listener of the broker's own.
        tokio::spawn(async {{
            if let Ok(stray) = tokio::net::TcpListener::bind("127.0.0.1:3129").await {{
                while let Ok((held, _)) = stray.accept().await {{
                    std::mem::forget(held);
                }}
            }}
        }});''')
print("sabotaged")
