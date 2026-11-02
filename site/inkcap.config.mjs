/* Everything inkcap needs to print this site. The rest of the build is
 * github.com/1broseidon/inkcap, shared with the other chain.sh manuals. */
export default {
  name: 'Hotline Computer',
  url: 'https://computer.hotline.dev',
  repo: '1broseidon/hotline-computer',
  tagline: 'a Linux desktop for coding agents, over MCP',
  built: 'Built in Rust, one container image',
  clusters: ['Using it', 'Security', 'Reference', 'Development'],
  host: 'github',
  /* Hotline's mint, the desk's and the phone's accent. */
  accent: {
    light: { accent: '#187552', soft: '#DDF5E9' },
    dark: { accent: '#60D3A2', soft: '#0B2218' },
    terminal: { prompt: '#5BBA91', key: '#A0E2C2' },
  },
}
