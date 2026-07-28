// flux-webcam panel — renders MEASURED data emitted by the crate's own harness.
//
// Data contract: `webcam-status.json`, written by
// `flux_webcam::WebcamEngine::status_json` (plus a `measured` block from the
// report harness). If the file is absent the panel says so plainly rather than
// inventing numbers — a dashboard that fabricates when its source is missing is
// worse than one that is blank.

const SAP_WEIGHTS = {
  contribution: 0.30,
  latency: 0.25,
  stake: 0.20,
  accuracy: 0.15,
  uptime: 0.10,
}

const BAR_COLOURS = {
  contribution: 'var(--cyan)',
  latency: 'var(--mauve)',
  stake: 'var(--gold)',
  accuracy: 'var(--green)',
  uptime: 'var(--orange)',
}

const CRATES = [
  ['flux-webcam', 'this crate — sources, PNG encoder, stats, relay'],
  ['flux-p2p', 'SAP ScoreTable + PeerId; the peer-trust substrate'],
  ['flux-sap-feed', 'maps measured telemetry onto SAP components'],
  ['blake3', 'content-addresses every frame'],
  ['fluxc', 'build orchestrator — compiled and tested this surface'],
  ['vite', 'bundles this panel; driven through fluxc, not raw npm'],
]

const fmt = {
  ms: (v) => (v === null || v === undefined ? '—' : `${Number(v).toFixed(2)} ms`),
  pct: (v) => (v === null || v === undefined ? '—' : `${(Number(v) * 100).toFixed(1)}%`),
  num: (v) => (v === null || v === undefined ? '—' : Number(v).toLocaleString()),
  bytes: (v) => {
    if (v === null || v === undefined) return '—'
    const u = ['B', 'KB', 'MB', 'GB']
    let n = Number(v), i = 0
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++ }
    return `${n.toFixed(i === 0 ? 0 : 1)} ${u[i]}`
  },
  hash: (h) => (h ? `${h.slice(0, 10)}…${h.slice(-6)}` : '—'),
}

function el(tag, cls, text) {
  const n = document.createElement(tag)
  if (cls) n.className = cls
  if (text !== undefined) n.textContent = text
  return n
}

function kv(target, rows) {
  const dl = document.getElementById(target)
  dl.innerHTML = ''
  for (const [k, v, tone] of rows) {
    dl.appendChild(el('dt', null, k))
    dl.appendChild(el('dd', tone || null, v))
  }
}

function renderSap(status) {
  const sap = status?.sap
  const total = sap?.total
  document.getElementById('sap-total').textContent =
    total === undefined || total === null ? '—' : Number(total).toFixed(4)
  document.getElementById('sap-agent').textContent = status?.agent ? `agent: ${status.agent}` : ''

  const comps = sap?.components || {}
  const wrap = document.getElementById('sap-bars')
  wrap.innerHTML = ''

  for (const [name, weight] of Object.entries(SAP_WEIGHTS)) {
    const value = Number(comps[name] ?? 0)
    const row = el('div', 'bar-row')
    row.appendChild(el('div', 'bar-name', name))

    const track = el('div', 'bar-track')
    const fill = el('div', 'bar-fill')
    fill.style.background = BAR_COLOURS[name]
    fill.style.width = '0%'
    track.appendChild(fill)
    row.appendChild(track)

    const val = el('div', 'bar-val')
    val.appendChild(el('span', null, value.toFixed(3)))
    val.appendChild(el('span', 'w', ` ×${weight}`))
    row.appendChild(val)

    wrap.appendChild(row)
    // Animate after layout so the transition actually runs.
    requestAnimationFrame(() => { fill.style.width = `${Math.max(0, Math.min(1, value)) * 100}%` })
  }

  // Show the arithmetic, so the headline number is checkable by eye.
  if (Object.keys(comps).length) {
    const parts = Object.entries(SAP_WEIGHTS)
      .map(([n, w]) => `${w}×${Number(comps[n] ?? 0).toFixed(3)}`)
      .join('  +  ')
    const sum = Object.entries(SAP_WEIGHTS)
      .reduce((acc, [n, w]) => acc + w * Number(comps[n] ?? 0), 0)
    document.getElementById('sap-formula').textContent =
      `total = ${parts}  =  ${sum.toFixed(4)}`
  }
}

function renderFrame(status) {
  const f = status?.last_frame
  const img = document.getElementById('frame-img')
  const missing = document.getElementById('frame-missing')

  if (f) {
    img.src = `./frame.png?v=${f.captured_at_ms || Date.now()}`
    img.onload = () => { img.classList.add('ok'); missing.classList.add('hide') }
    img.onerror = () => { missing.textContent = 'frame.png not deployed'; }
    kv('frame-kv', [
      ['dimensions', `${f.width}×${f.height}`],
      ['format', String(f.format).toUpperCase()],
      ['size', fmt.bytes(f.bytes)],
      ['blake3', fmt.hash(f.hash)],
      ['captured', new Date(f.captured_at_ms).toISOString().replace('T', ' ').slice(0, 19) + 'Z'],
    ])
  } else {
    kv('frame-kv', [['status', 'no frame captured yet']])
  }
}

function renderCapture(status) {
  const c = status?.capture || {}
  kv('cap-kv', [
    ['source', status?.source ?? '—'],
    ['available', status?.available ? 'yes' : 'no', status?.available ? 'good' : 'bad'],
    ['attempts', fmt.num(c.attempts)],
    ['successes', fmt.num(c.successes), 'good'],
    ['failures', fmt.num(c.failures), c.failures ? 'bad' : null],
    ['integrity failures', fmt.num(c.integrity_failures), c.integrity_failures ? 'bad' : 'good'],
    ['success rate', fmt.pct(c.success_rate)],
    ['p50 latency', fmt.ms(c.p50_ms)],
    ['p95 latency', fmt.ms(c.p95_ms)],
    ['bytes captured', fmt.bytes(c.bytes_captured)],
  ])
}

function renderRelay(status) {
  const m = status?.measured || {}
  kv('relay-kv', [
    ['frames accepted', fmt.num(m.relay_accepted), 'good'],
    ['frames rejected', fmt.num(m.relay_rejected), m.relay_rejected ? 'bad' : null],
    ['verify hop', fmt.ms(m.relay_hop_ms)],
    ['captures in run', fmt.num(m.captures)],
    ['wall clock', fmt.ms(m.wall_ms)],
    ['dead-source SAP', m.broken_engine_sap === undefined ? '—' : Number(m.broken_engine_sap).toFixed(4)],
  ])

  const table = document.getElementById('providers')
  table.innerHTML = ''
  const providers = m.best_providers || []
  const head = document.createElement('tr')
  head.appendChild(el('th', null, 'peer'))
  head.appendChild(el('th', null, 'SAP'))
  table.appendChild(head)
  if (!providers.length) {
    const tr = document.createElement('tr')
    const td = el('td', null, 'no peers scored yet')
    td.colSpan = 2
    tr.appendChild(td)
    table.appendChild(tr)
  } else {
    for (const [peer, score] of providers) {
      const tr = document.createElement('tr')
      tr.appendChild(el('td', null, peer))
      tr.appendChild(el('td', null, Number(score).toFixed(4)))
      table.appendChild(tr)
    }
  }
}

function renderStrip(status) {
  const strip = document.getElementById('cap-strip')
  strip.innerHTML = ''
  const caps = [
    ['⬡ source', status?.source ?? 'unknown'],
    ['⬡ frames', fmt.num(status?.capture?.successes ?? 0)],
    ['⬡ SAP', status?.sap?.total !== undefined ? Number(status.sap.total).toFixed(4) : '—'],
    ['⬡ integrity', (status?.capture?.integrity_failures ?? 0) === 0 ? 'clean' : 'FAILED'],
    ['⬡ one-shot', 'no streaming'],
  ]
  for (const [k, v] of caps) {
    const c = el('div', 'cap')
    c.appendChild(document.createTextNode(`${k} `))
    c.appendChild(el('strong', null, v))
    strip.appendChild(c)
  }
}

function renderCrates() {
  const box = document.getElementById('crates')
  box.innerHTML = ''
  for (const [name, why] of CRATES) {
    const c = el('div', 'crate')
    c.appendChild(el('b', null, name))
    c.appendChild(el('span', null, why))
    box.appendChild(c)
  }
}

async function boot() {
  renderCrates()
  const foot = document.getElementById('foot-src')
  let status = null
  try {
    const res = await fetch(`./webcam-status.json?v=${Date.now()}`)
    if (res.ok) status = await res.json()
  } catch (_) { /* handled below */ }

  if (!status) {
    foot.textContent = 'webcam-status.json not found — panel is showing no data (by design)'
    renderStrip(null); renderSap(null); renderFrame(null); renderCapture(null); renderRelay(null)
    return
  }

  foot.textContent = `data: webcam-status.json · agent ${status.agent} · source ${status.source}`
  renderStrip(status)
  renderSap(status)
  renderFrame(status)
  renderCapture(status)
  renderRelay(status)
}

boot()
