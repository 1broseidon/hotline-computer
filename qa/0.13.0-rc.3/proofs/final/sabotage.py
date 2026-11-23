"""Writes deliberate breaks into a copy of the tree (never a release): each
turns off the guard a new probe is about. usage: sabotage.py DIR [NAME...]
(no names: every break)."""
import sys, os
os.chdir(sys.argv[1])
only = set(sys.argv[2:])
B = "// SABOTAGE (rc3-probes broken build, never a release): "
OFF = 'std::env::var_os("HOTLINE_SABOTAGE_OFF")'
def edit(name, p, old, new):
    if only and name not in only:
        return
    s = open(p).read(); assert old in s, (name, p, old[:60]); open(p, "w").write(s.replace(old, new, 1))
    print("broke", name)
edit("withhold", "src/signin/mod.rs", '''    pub async fn withhold_origin(&self, origin: &str, name: &str, values: &[String]) {
        let running''', f'''    pub async fn withhold_origin(&self, origin: &str, name: &str, values: &[String]) {{
        {B}the extension's fill withholds nothing.
        if {OFF}.is_none() {{
            let _ = (origin, name, values);
            return;
        }}
        let running''')
edit("bfcache", "src/signin/mod.rs", '''            "--disable-features=BackForwardCache,MediaRouter,OptimizationHints,Translate",''',
     f'''            {B}the back-forward cache is on.
            "--disable-features=MediaRouter,OptimizationHints,Translate",''')
edit("stopped", "src/signin/mod.rs", '''        let Some(running) = running.filter(|running| !running.cdp.is_closed()) else {
            self.revoke_later(sites);''', f'''        let Some(running) = running.filter(|running| !running.cdp.is_closed()) else {{
            {B}a revocation while stopped is dropped.
            if {OFF}.is_none() {{
                return Ok((0, 0));
            }}
            self.revoke_later(sites);''')
edit("running", "src/signin/mod.rs", '''        if !gone.is_empty() {
            cdp.call("Storage.clearCookies", json!({})).await?;''', f'''        {B}a revocation while running leaves the cookies.
        if !gone.is_empty() && {OFF}.is_some() {{
            cdp.call("Storage.clearCookies", json!({{}})).await?;''')
edit("persist", "src/signin/mod.rs", '''fn persist_revoked(path: &std::path::Path, sites: &[Origin]) {
''', f'''fn persist_revoked(path: &std::path::Path, sites: &[Origin]) {{
    {B}revocations are not written down.
    if {OFF}.is_none() {{
        let _ = (path, sites);
        return;
    }}
''')
edit("persist", "src/signin/mod.rs", '''fn read_revoked(path: &std::path::Path) -> Vec<Origin> {
''', f'''fn read_revoked(path: &std::path::Path) -> Vec<Origin> {{
    {B}nor read back.
    if {OFF}.is_none() {{
        let _ = path;
        return Vec::new();
    }}
''')
edit("redact", "src/secrets.rs", '''    fn redact_at<'a>(&self, text: &'a str, now: u64) -> Cow<'a, str> {
''', f'''    fn redact_at<'a>(&self, text: &'a str, now: u64) -> Cow<'a, str> {{
        {B}nothing is redacted.
        if {OFF}.is_none() {{
            let _ = now;
            return Cow::Borrowed(text);
        }}
''')
edit("forget", "src/holds.rs", '''    pub fn forget(&self, names: &[String]) {
        if names.is_empty() {''', f'''    pub fn forget(&self, names: &[String]) {{
        {B}answers outlive their key.
        if names.is_empty() || {OFF}.is_none() {{''')
edit("minting", "src/keyproxy/conn.rs", '''        if swapped && self.table().refused(&names, &self.origin, parts.uri.path()) {''',
     f'''        {B}no minting route is refused.
        if swapped && {OFF}.is_some() && self.table().refused(&names, &self.origin, parts.uri.path()) {{''')
edit("owner", "src/keyproxy/conn.rs", '''            && !check(peer, job.clone()).await''', f'''            {B}the owner check is skipped.
            && {OFF}.is_some()
            && !check(peer, job.clone()).await''')
edit("stray", "src/split/broker.rs", '''        app.signin = Some(crate::signin::split::for_broker(&app));''', f'''        app.signin = Some(crate::signin::split::for_broker(&app));
        {B}a stray listener of the broker's own.
        tokio::spawn(async {{
            if let Ok(stray) = tokio::net::TcpListener::bind("127.0.0.1:3129").await {{
                while let Ok((held, _)) = stray.accept().await {{
                    std::mem::forget(held);
                }}
            }}
        }});''')
edit("landlock", "src/jobs.rs", '''#[cfg(target_os = "linux")]
fn a_domain_of_its_own() {
''', f'''#[cfg(target_os = "linux")]
fn a_domain_of_its_own() {{
    {B}a job gets no domain of its own.
    if std::env::var_os("HOTLINE_SABOTAGE_OFF").is_none() {{
        return;
    }}
''')
