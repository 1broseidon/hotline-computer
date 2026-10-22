// node mcp.mjs <tool> '<json args>' — one MCP call against $HC_URL with $HC_TOKEN.
const base = process.env.HC_URL, token = process.env.HC_TOKEN;
const head = { 'content-type': 'application/json', accept: 'application/json, text/event-stream', authorization: `Bearer ${token}` };
async function rpc(body, session) {
  const r = await fetch(`${base}/mcp`, { method: 'POST', headers: { ...head, ...(session ? { 'mcp-session-id': session } : {}) }, body: JSON.stringify(body) });
  const text = await r.text();
  const data = text.split('\n').filter((l) => l.startsWith('data:')).map((l) => l.slice(5)).filter(Boolean).pop() || text;
  return { session: r.headers.get('mcp-session-id'), json: data ? (() => { try { return JSON.parse(data); } catch { return data; } })() : null };
}
const init = await rpc({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'pork-chop-drive', version: '1' } } });
const s = init.session;
await rpc({ jsonrpc: '2.0', method: 'notifications/initialized' }, s);
const [tool, args = '{}'] = process.argv.slice(2);
const out = await rpc({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: tool, arguments: JSON.parse(args) } }, s);
const result = out.json?.result;
if (!result) { console.log(JSON.stringify(out.json)); process.exit(1); }
for (const block of result.content || []) {
  if (block.type === 'image') {
    const file = process.env.HC_OUT || `/tmp/hc7/shot-${Date.now()}.png`;
    (await import('node:fs')).writeFileSync(file, Buffer.from(block.data, 'base64'));
    console.log(`[image -> ${file}]`);
  } else console.log(block.text);
}
if (result.isError) process.exit(2);
