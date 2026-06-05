const $ = (id) => document.getElementById(id)

const TARGETS = [
  'https://sigilgraph.quillon.xyz/',
  'https://quillon.xyz/sigil-wallet/',
  'http://flux/',
  'http://flux:',
  'http://localhost/',
  'http://localhost:',
  'http://127.0.0.1/',
  'http://127.0.0.1:',
]

// Host patterns the extension wants permission to inspect.
const WALLET_ORIGINS = [
  'https://sigilgraph.quillon.xyz/*',
  'https://quillon.xyz/*',
]

const DEFAULT_HOST = 'https://sigileye.quillon.xyz'

const bridgeConfig = async () => {
  const { host, eyeToken, eye_token } = await chrome.storage.local.get(['host', 'eyeToken', 'eye_token'])
  const token = eyeToken || eye_token || ''
  const headers = token ? { 'X-Sigil-Eye-Token': token } : {}
  return {
    host: (host || DEFAULT_HOST).replace(/\/$/, ''),
    headers,
  }
}

const bridgeFetch = async (path, init = {}) => {
  const cfg = await bridgeConfig()
  return fetch(cfg.host + path, {
    ...init,
    headers: {
      ...cfg.headers,
      ...(init.headers || {}),
    },
  })
}

const checkPermissions = () => new Promise(r => {
  chrome.permissions.contains({ origins: WALLET_ORIGINS }, granted => r(!!granted))
})

const refresh = async () => {
  const { host: h } = await bridgeConfig()
  $('host').textContent = h

  try {
    const r = await bridgeFetch('/health', { cache: 'no-store' })
    const j = await r.json()
    $('srv').textContent = '✓ up'
    $('srv').className = 'v ok'
    $('q').textContent = String(j.queue ?? 0)
    $('snap').textContent = j.snap ? '✓ present' : '— none'
    $('snap').className = 'v ' + (j.snap ? 'ok' : 'no')
  } catch (e) {
    $('srv').textContent = '✕ down'
    $('srv').className = 'v no'
    $('q').textContent = '—'
    $('snap').textContent = '—'
  }

  chrome.tabs.query({}, tabs => {
    const hit = tabs.find(t => t.url && TARGETS.some(p => t.url.startsWith(p)))
    if (hit) { $('tab').textContent = '✓ open'; $('tab').className = 'v ok' }
    else     { $('tab').textContent = '✕ none'; $('tab').className = 'v no' }
  })

  // Site-access state — show the grant button when missing.
  const granted = await checkPermissions()
  if (granted) {
    $('perm').textContent = '✓ granted'; $('perm').className = 'v ok'
    $('grantBtn').style.display = 'none'
  } else {
    $('perm').textContent = '✕ click 🔓'; $('perm').className = 'v no'
    $('grantBtn').style.display = 'block'
  }

  // Update banner — surface available version from background storage.
  try {
    const { update } = await chrome.storage.local.get(['update'])
    if (update && update.available) {
      $('updateBanner').style.display = 'block'
      $('updateVer').textContent = 'v' + update.version
      $('updateNotes').textContent = (update.notes || '').slice(0, 140)
      $('updateBtn').dataset.url = update.zip_url || 'https://quillon.xyz/downloads/flux-eye-extension.zip'
    } else {
      $('updateBanner').style.display = 'none'
    }
  } catch { /* storage unavailable */ }
}

$('updateBtn')?.addEventListener('click', () => {
  const url = $('updateBtn').dataset.url
  if (url) chrome.tabs.create({ url })
  chrome.tabs.create({ url: 'chrome://extensions' })
})

// ── click + snap presets ──
//
// Each preset clicks the ribbon button identified by `data-id`, waits 800ms
// for the panel slide animation to complete, then snapshots the full state
// (panel + ribbon button audit + computed transform / display / visibility).
// The snapshot is auto-tagged with the preset name so I can tell which click
// the snapshot came from without needing a manual comment.
document.querySelectorAll('.cs').forEach((b) => {
  b.addEventListener('click', async (e) => {
    const btn = e.currentTarget
    const id = btn.dataset.id
    const panel = btn.dataset.panel
    btn.disabled = true
    const origText = btn.textContent
    btn.textContent = '⌛ ...'
    try {
      const tabs = await chrome.tabs.query({})
      const tab = tabs.find(t => t.url && TARGETS.some(p => t.url.startsWith(p)))
      if (!tab) { btn.textContent = '✕ no tab'; setTimeout(() => { btn.textContent = origText; btn.disabled = false }, 1500); return }

      // step 1: click the ribbon button
      const [clickRes] = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        world: 'MAIN',
        args: [id],
        func: (bid) => {
          const b = document.querySelector('#' + bid)
          if (!b) return { found: false, ribbon_buttons: Array.from(document.querySelectorAll('.sigil-ribbon button')).map(x => ({ id: x.id, cls: x.className, txt: (x.textContent || '').trim().slice(0, 8) })) }
          const rect = b.getBoundingClientRect()
          b.click()
          return { found: true, rect: { x: Math.round(rect.x), y: Math.round(rect.y), w: Math.round(rect.width), h: Math.round(rect.height) } }
        },
      })

      // step 2: wait for slide animation
      await new Promise(r => setTimeout(r, 800))

      // step 3: comprehensive snapshot
      const [snapRes] = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        world: 'MAIN',
        args: [panel, id, clickRes.result, ($('snapComment')?.value || '').slice(0, 200)],
        func: (p, bid, click, comment) => {
          const bd = document.querySelector('.sigil-' + p + '-bd')
          const innerSel = '.sigil-' + p + '-in'
          const inner = bd && (bd.querySelector(innerSel) || bd.querySelector('.sigil-' + p))
          const cs = bd ? getComputedStyle(bd) : null
          const ics = inner ? getComputedStyle(inner) : null
          const rect = bd && bd.getBoundingClientRect()
          const irect = inner && inner.getBoundingClientRect()
          // ALL panels (every one, not just the clicked one) so we see the broader state
          const all = ['settings', 'ab', 'mine', 'dag', 'mint', 'tok']
          const panels = {}
          for (const px of all) {
            const xbd = document.querySelector('.sigil-' + px + '-bd')
            if (!xbd) { panels[px] = { exists: false }; continue }
            const xinner = xbd.querySelector('.sigil-' + px + '-in') || xbd.querySelector('.sigil-' + px)
            const xcs = getComputedStyle(xbd)
            const xics = xinner ? getComputedStyle(xinner) : null
            const xrect = xbd.getBoundingClientRect()
            const xirect = xinner && xinner.getBoundingClientRect()
            panels[px] = {
              exists: true,
              open: xbd.classList.contains('open'),
              bd_display: xcs.display,
              bd_z: xcs.zIndex,
              bd_rect: { x: Math.round(xrect.x), y: Math.round(xrect.y), w: Math.round(xrect.width), h: Math.round(xrect.height) },
              inner_display: xics ? xics.display : null,
              inner_visibility: xics ? xics.visibility : null,
              inner_width: xics ? xics.width : null,
              inner_transform: xics ? xics.transform : null,
              inner_rect: xirect ? { x: Math.round(xirect.x), y: Math.round(xirect.y), w: Math.round(xirect.width), h: Math.round(xirect.height) } : null,
              inner_textBytes: xinner ? (xinner.textContent || '').length : 0,
            }
          }
          // Ribbon button audit
          const rb = document.querySelector('.sigil-ribbon')
          const ribbon = rb ? {
            childCount: rb.children.length,
            buttons: Array.from(rb.querySelectorAll('button')).map(b => ({
              id: b.id || null,
              cls: b.className,
              txt: (b.textContent || '').trim().slice(0, 10),
              rect: (() => { const r = b.getBoundingClientRect(); return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) } })(),
              pointer_events: getComputedStyle(b).pointerEvents,
              visibility: getComputedStyle(b).visibility,
            })),
          } : { exists: false }
          return {
            ts: Date.now(),
            href: location.href,
            comment: comment || null,
            preset: { clicked: bid, target_panel: p, click: click },
            panels,
            ribbon,
            theme: document.documentElement.dataset.theme || null,
          }
        },
      })

      // step 4: post to server
      await bridgeFetch('/snapshot', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(snapRes.result),
      })

      // step 5: show result inline
      const opened = snapRes.result?.panels?.[panel]?.open
      btn.textContent = opened ? '✓ opened' : '✕ closed'
      setTimeout(() => { btn.textContent = origText; btn.disabled = false }, 1800)
    } catch (e) {
      btn.textContent = '✕ ' + String(e.message || e).slice(0, 8)
      setTimeout(() => { btn.textContent = origText; btn.disabled = false }, 2000)
    }
  })
})

$('grantBtn').addEventListener('click', () => {
  // Must be inside a user gesture — popup button click qualifies.
  chrome.permissions.request({ origins: WALLET_ORIGINS }, granted => {
    if (granted) {
      $('grantBtn').textContent = '✓ access granted'
      setTimeout(() => { $('grantBtn').style.display = 'none'; refresh() }, 900)
    } else {
      $('grantBtn').textContent = '✕ denied — try chrome://extensions'
      setTimeout(() => { $('grantBtn').textContent = '🔓 grant site access' }, 2400)
    }
  })
})

// Persist whatever's in the comment field across popup opens, and DON'T
// auto-clear it after a snapshot — Viktor wants the same comment to ride along
// while he clicks through multiple snaps.
;(async () => {
  try {
    const { lastComment } = await chrome.storage.local.get(['lastComment'])
    if (lastComment && $('snapComment')) $('snapComment').value = lastComment
  } catch {}
})()
const persistComment = async () => {
  try {
    const v = $('snapComment')?.value || ''
    await chrome.storage.local.set({ lastComment: v.slice(0, 200) })
  } catch {}
}
$('snapComment')?.addEventListener('input', persistComment)

// ── 📡 Listen for commands ─────────────────────────────────────────────────
//
// While the popup is open AND the user toggled "listen" ON, poll /cmd every
// 1.5s. When a command arrives, execute it via chrome.scripting on the wallet
// tab, post the result back to /result.
//
// Supported cmd types (same wire shape as the dev-loop expects):
//   { fn: 'snapshot', args: null }       → full panel snapshot, no click
//   { fn: 'click',    args: { selector } } → document.querySelector(sel).click()
//   { fn: 'eval',     args: { expr } }   -> disabled unless allowEvalCommands=true
//
// Persisted toggle: chrome.storage.local.listening (boolean).

let LISTEN_TIMER = null

const setListenStatus = (txt, color) => {
  const el = $('listenStatus')
  if (!el) return
  el.textContent = txt
  el.style.color = color || '#64748b'
}

const setListenBtn = (on) => {
  const b = $('listenBtn')
  if (!b) return
  if (on) {
    b.textContent = '🔴 stop listening'
    b.style.background = 'linear-gradient(135deg, rgba(244,63,94,0.30), rgba(244,63,94,0.10))'
    b.style.color = '#f87171'
    b.style.borderColor = 'rgba(244,63,94,0.55)'
  } else {
    b.textContent = '📡 listen for cmds'
    b.style.background = 'rgba(2,6,23,0.55)'
    b.style.color = '#c084fc'
    b.style.borderColor = 'rgba(192,132,252,0.40)'
  }
}

const executeCmdInWallet = async (cmd, tabId) => {
  if (cmd.fn === 'snapshot') {
    const [r] = await chrome.scripting.executeScript({
      target: { tabId },
      world: 'MAIN',
      args: [($('snapComment')?.value || '').slice(0, 200), cmd],
      func: (cmt, cmd) => {
        const panels = ['settings', 'ab', 'mine', 'dag', 'mint', 'tok']
        const out = { ts: Date.now(), href: location.href, comment: cmt || null, via: 'listen', cmd_id: cmd.id, panels: {} }
        for (const p of panels) {
          const bd = document.querySelector('.sigil-' + p + '-bd')
          if (!bd) { out.panels[p] = { exists: false }; continue }
          const inner = bd.querySelector('.sigil-' + p + '-in') || bd.querySelector('.sigil-' + p)
          const cs = getComputedStyle(bd)
          const ics = inner ? getComputedStyle(inner) : null
          const rect = bd.getBoundingClientRect()
          const irect = inner ? inner.getBoundingClientRect() : null
          out.panels[p] = {
            exists: true,
            open: bd.classList.contains('open'),
            bd_display: cs.display,
            bd_rect: { x: Math.round(rect.x), y: Math.round(rect.y), w: Math.round(rect.width), h: Math.round(rect.height) },
            inner_display: ics ? ics.display : null,
            inner_visibility: ics ? ics.visibility : null,
            inner_width: ics ? ics.width : null,
            inner_transform: ics ? ics.transform : null,
            inner_rect: irect ? { x: Math.round(irect.x), y: Math.round(irect.y), w: Math.round(irect.width), h: Math.round(irect.height) } : null,
            inner_textBytes: inner ? (inner.textContent || '').length : 0,
          }
        }
        return out
      },
    })
    return r?.result
  }
  if (cmd.fn === 'click') {
    const [r] = await chrome.scripting.executeScript({
      target: { tabId },
      world: 'MAIN',
      args: [cmd.args?.selector || ''],
      func: (sel) => {
        const el = document.querySelector(sel)
        if (!el) return { ok: false, error: 'no match for ' + sel }
        el.click()
        return { ok: true, clicked: sel }
      },
    })
    return r?.result
  }
  if (cmd.fn === 'eval') {
    const { allowEvalCommands } = await chrome.storage.local.get(['allowEvalCommands'])
    if (!allowEvalCommands) return { ok: false, error: 'eval commands disabled' }
    const [r] = await chrome.scripting.executeScript({
      target: { tabId },
      world: 'MAIN',
      args: [cmd.args?.expr || 'null'],
      func: (expr) => {
        try {
          // eslint-disable-next-line no-new-func
          const fn = new Function('return (' + expr + ')')
          return { ok: true, value: fn() }
        } catch (e) { return { ok: false, error: String(e && e.message || e) } }
      },
    })
    return r?.result
  }
  return { ok: false, error: 'unknown fn: ' + cmd.fn }
}

const pollOnce = async () => {
  try {
    const r = await bridgeFetch('/cmd', { cache: 'no-store' })
    if (!r.ok) { setListenStatus('srv ' + r.status, '#f87171'); return }
    const cmd = await r.json()
    if (cmd.idle) { setListenStatus('listening…', '#4ade80'); return }
    setListenStatus('exec ' + cmd.fn, '#fbbf24')
    const tabs = await chrome.tabs.query({})
    const tab = tabs.find(t => t.url && TARGETS.some(p => t.url.startsWith(p)))
    if (!tab) {
      await bridgeFetch('/result', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: cmd.id, error: 'no matching tab open' }),
      })
      setListenStatus('no tab', '#f87171')
      return
    }
    let result, error
    try { result = await executeCmdInWallet(cmd, tab.id) }
    catch (e) { error = String(e && e.message || e) }
    await bridgeFetch('/result', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: cmd.id, result, error }),
    })
    setListenStatus('✓ ' + cmd.fn, '#4ade80')
  } catch (e) {
    setListenStatus('net err', '#f87171')
  }
}

const startListen = () => {
  if (LISTEN_TIMER) return
  setListenBtn(true)
  setListenStatus('listening…', '#4ade80')
  pollOnce()
  LISTEN_TIMER = setInterval(pollOnce, 1500)
  try { chrome.storage.local.set({ listening: true }) } catch {}
}
const stopListen = () => {
  if (LISTEN_TIMER) { clearInterval(LISTEN_TIMER); LISTEN_TIMER = null }
  setListenBtn(false)
  setListenStatus('idle', '#64748b')
  try { chrome.storage.local.set({ listening: false }) } catch {}
}

$('listenBtn')?.addEventListener('click', () => {
  if (LISTEN_TIMER) stopListen()
  else startListen()
})

// On popup open, restore listening state if it was on before
;(async () => {
  try {
    const { listening } = await chrome.storage.local.get(['listening'])
    if (listening) startListen()
  } catch {}
})()

// ── 🛰 Live popup patch ────────────────────────────────────────────────────
// Unsigned remote popup patches are disabled.
// Remote JavaScript is not executed in the popup context.
// presets, bug fixes — without a reinstall.
// Unsigned remote popup patches are disabled. Ship reviewed extension bundles
// instead of executing remote JavaScript inside the popup context.

$('snapBtn').addEventListener('click', async () => {
  $('snapBtn').textContent = '⌛ snapshotting…'
  const comment = ($('snapComment')?.value || '').slice(0, 200)
  await persistComment()
  const tabs = await chrome.tabs.query({})
  const tab = tabs.find(t => t.url && TARGETS.some(p => t.url.startsWith(p)))
  if (!tab) { $('snapBtn').textContent = '✕ no wallet tab'; setTimeout(() => $('snapBtn').textContent = '⚡ snapshot now', 1200); return }
  try {
    const [r] = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: 'MAIN',
      args: [comment],
      func: (cmt) => {
        const panels = ['settings', 'ab', 'mine', 'dag', 'mint', 'tok']
        const out = { ts: Date.now(), href: location.href, comment: cmt || null, panels: {} }
        for (const p of panels) {
          const bd = document.querySelector('.sigil-' + p + '-bd')
          out.panels[p] = bd ? {
            exists: true,
            open: bd.classList.contains('open'),
            display: getComputedStyle(bd).display,
            childCount: bd.children.length,
            htmlBytes: bd.outerHTML.length,
          } : { exists: false }
        }
        // ribbon button presence — proves the click handler should be attached
        const rb = document.querySelector('.sigil-ribbon')
        if (rb) {
          out.ribbon = {
            childCount: rb.children.length,
            buttons: Array.from(rb.querySelectorAll('button')).map(b => ({
              id: b.id || null,
              cls: b.className,
              txt: (b.textContent || '').trim().slice(0, 12),
              has_click: !!b.onclick || b.hasAttribute('data-click-bound'),
            })),
          }
        }
        return out
      }
    })
    await bridgeFetch('/snapshot', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(r.result),
    })
    // Intentionally NOT clearing snapComment so it rides along across snaps.
    $('snapBtn').textContent = '✓ snapshotted'
    setTimeout(() => $('snapBtn').textContent = '⚡ snapshot now', 1200)
    refresh()
  } catch (e) {
    $('snapBtn').textContent = '✕ ' + (e.message || e)
    setTimeout(() => $('snapBtn').textContent = '⚡ snapshot now', 1800)
  }
})

$('optBtn').addEventListener('click', () => chrome.runtime.openOptionsPage())

refresh()
setInterval(refresh, 2500)
