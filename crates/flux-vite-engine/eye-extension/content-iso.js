// Flux Eye — ISOLATED-world content script: the poller + uploader.
//
// THIS FILE WAS MISSING from v0.2.6 — the manifest only registered the MAIN-world
// error hook (content.js), so nothing ever polled /cmd or posted /snapshot. That
// is why "Queue depth —, Last snap —" and the eye could never snap. This restores
// the documented behaviour:
//   • poll  GET  {host}/cmd      every 1.5s → execute {id, fn, args} → POST /result
//   • snap  POST {host}/snapshot every 3.5s → latest wallet panel state + errors
// All requests carry the bearer token (required now the server binds 0.0.0.0).

(function () {
  if (window.__fluxEyeIsoInstalled) return
  window.__fluxEyeIsoInstalled = true

  const DEFAULT_HOST = 'http://89.149.241.126:9789'
  let HOST = DEFAULT_HOST
  let TOKEN = ''

  // MAIN-world content.js mirrors page errors to us via CustomEvent.
  const errors = []
  document.addEventListener('flux-eye-error', (e) => {
    try { errors.push(e.detail); if (errors.length > 100) errors.splice(0, errors.length - 100) } catch {}
  })

  const loadCfg = () => new Promise((resolve) => {
    try {
      chrome.storage.local.get(['host', 'token'], (v) => {
        HOST = (v && v.host ? String(v.host) : DEFAULT_HOST).replace(/\/$/, '')
        TOKEN = v && v.token ? String(v.token) : ''
        resolve()
      })
    } catch { resolve() }
  })

  const hdrs = (extra) => Object.assign(
    { 'Content-Type': 'application/json' },
    TOKEN ? { 'Authorization': 'Bearer ' + TOKEN } : {},
    extra || {}
  )

  // ── command executor ────────────────────────────────────────────────────
  const txt = (el) => (el && (el.innerText || el.textContent) || '').trim().slice(0, 4000)
  const visible = (el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 }

  function snapshotState() {
    // Serialize the wallet's visible panel text + a compact DOM outline + errors.
    const panels = []
    document.querySelectorAll('[class*="panel"], [class*="card"], main, section').forEach((el) => {
      if (visible(el)) { const t = txt(el); if (t) panels.push(t) }
    })
    return {
      url: location.href,
      title: document.title,
      ts: Date.now(),
      eye_installed: !!window.__fluxEyeInstalled,
      panel_text: panels.slice(0, 40),
      body_text: txt(document.body).slice(0, 8000),
      errors: errors.slice(-25),
    }
  }

  function execCommand(fn, args) {
    args = args || {}
    switch (fn) {
      case 'snapshot':
      case 'snap':
        return snapshotState()
      case 'query': {
        const els = Array.from(document.querySelectorAll(args.selector || 'body'))
        return { count: els.length, items: els.slice(0, 20).map((e) => ({ tag: e.tagName.toLowerCase(), text: txt(e).slice(0, 600) })) }
      }
      case 'click': {
        const el = document.querySelector(args.selector || '')
        if (!el) return { ok: false, error: 'no match for ' + args.selector }
        el.click()
        return { ok: true, clicked: args.selector }
      }
      case 'click_snap': {
        const el = document.querySelector(args.selector || '')
        if (el) el.click()
        return { ok: !!el, clicked: args.selector || null, after: snapshotState() }
      }
      default:
        return { ok: false, error: 'unknown fn: ' + fn }
    }
  }

  // ── poll + snapshot loops ───────────────────────────────────────────────
  async function pollOnce() {
    try {
      const r = await fetch(HOST + '/cmd', { headers: hdrs(), cache: 'no-store' })
      if (!r.ok) return
      const c = await r.json()
      if (c.idle) return
      let result
      try { result = execCommand(c.fn, c.args) } catch (e) { result = { ok: false, error: String(e) } }
      await fetch(HOST + '/result', { method: 'POST', headers: hdrs(), body: JSON.stringify({ id: c.id, result }) })
    } catch { /* host unreachable — stay quiet */ }
  }

  async function snapOnce() {
    try {
      await fetch(HOST + '/snapshot', { method: 'POST', headers: hdrs(), body: JSON.stringify(snapshotState()) })
    } catch { /* noop */ }
  }

  loadCfg().then(() => {
    try { chrome.storage.onChanged.addListener(() => loadCfg()) } catch {}
    setInterval(pollOnce, 1500)
    setInterval(snapOnce, 3500)
    pollOnce(); snapOnce()
  })
})()
