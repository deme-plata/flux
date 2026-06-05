// Flux Eye — content script.
//
// Installs error-capture hooks inside the wallet page so that JS exceptions
// and unhandled promise rejections accumulate in window.__fluxEyeErrors.
// The background snapshot reads that array when building each panel snapshot.
//
// Runs at document_start so we catch errors raised before React mounts.

(function () {
  const KEY = '__fluxEyeErrors'
  if (window[KEY]) return  // already installed
  window[KEY] = []
  const MAX = 100

  const push = (entry) => {
    try {
      const full = { ts: Date.now(), ...entry }
      window[KEY].push(full)
      if (window[KEY].length > MAX) window[KEY].splice(0, window[KEY].length - MAX)
      // Mirror to the ISOLATED-world content script that uploads snapshots.
      document.dispatchEvent(new CustomEvent('flux-eye-error', { detail: full }))
    } catch { /* noop */ }
  }

  // 1) Synchronous errors anywhere on the page.
  window.addEventListener('error', (e) => {
    push({
      kind: 'error',
      message: String(e.message || ''),
      filename: String(e.filename || ''),
      line: e.lineno || 0,
      col: e.colno || 0,
      stack: e.error && e.error.stack ? String(e.error.stack).split('\n').slice(0, 6).join('\n') : null,
    })
  }, true)

  // 2) Resource load errors (img/script/link/etc).
  window.addEventListener('error', (e) => {
    const t = e.target
    if (t && t !== window && t.tagName) {
      push({
        kind: 'resource',
        tag: String(t.tagName).toLowerCase(),
        url: String(t.src || t.href || ''),
        message: 'resource load failed',
      })
    }
  }, true)

  // 3) Promise rejections.
  window.addEventListener('unhandledrejection', (e) => {
    push({
      kind: 'unhandledrejection',
      message: String(e.reason && e.reason.message || e.reason || ''),
      stack: e.reason && e.reason.stack ? String(e.reason.stack).split('\n').slice(0, 6).join('\n') : null,
    })
  })

  // 4) Wrap console.error so wallets that swallow exceptions in catch{} still
  //    leak observability.
  const origConsoleError = console.error.bind(console)
  console.error = function (...args) {
    try {
      push({
        kind: 'console.error',
        message: args.map(a => {
          try { return typeof a === 'string' ? a : JSON.stringify(a) } catch { return String(a) }
        }).join(' ').slice(0, 800),
      })
    } catch { /* noop */ }
    return origConsoleError(...args)
  }

  // Mark install for diagnostic surface — both as a JS global AND as a DOM
  // data-attribute so any flux-family page (sigil wallet, os.html, garden,
  // ide, …) can detect presence without needing extension messaging.
  window.__fluxEyeInstalled = { v: '0.1.0', at: Date.now() }
  try {
    document.documentElement.dataset.fluxEye = '0.1.0'
    // Fire a CustomEvent so listeners that bind after install see it.
    document.dispatchEvent(new CustomEvent('flux-eye-ready', {
      detail: { v: '0.1.0', at: Date.now() }
    }))
  } catch { /* noop — document not ready */ }
})()
