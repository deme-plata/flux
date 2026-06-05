// Flux Eye — service worker.
//
// Two responsibilities now (after v0.1.3 moved snapshot capture into the
// content-iso script):
//
// 1. Soft auto-updater — polls `flux-eye-version.json` from quillon.xyz on a
//    chrome.alarms tick, compares semver to the running manifest version, and
//    writes the result into chrome.storage.local so the popup can surface a
//    "🆕 update available" banner.
//
// 2. Settings bootstrap — initialise default `host` on install.
//
// Snapshot polling + command execution NO LONGER happen here. Content-iso.js
// owns those, bypassing the chrome.scripting host-permission gate that MV3
// imposes on background workers for non-active tabs.

const DEFAULT_HOST     = 'http://127.0.0.1:9789'
const VERSION_URL      = 'https://quillon.xyz/downloads/flux-eye-version.json'
const UPDATE_PERIOD_MIN = 30  // chrome.alarms minimum is 1 in production; 30 is friendly

const parseSemver = (s) => {
  const m = /^(\d+)\.(\d+)\.(\d+)/.exec(String(s || ''))
  if (!m) return [0, 0, 0]
  return [Number(m[1]), Number(m[2]), Number(m[3])]
}
const cmpSemver = (a, b) => {
  const [x1, y1, z1] = parseSemver(a)
  const [x2, y2, z2] = parseSemver(b)
  if (x1 !== x2) return x1 - x2
  if (y1 !== y2) return y1 - y2
  return z1 - z2
}

async function checkForUpdate() {
  try {
    const r = await fetch(VERSION_URL + '?ts=' + Date.now(), { cache: 'no-store' })
    if (!r.ok) return
    const j = await r.json()
    const remote = String(j.version || '')
    const local = chrome.runtime.getManifest().version
    if (cmpSemver(remote, local) > 0) {
      await chrome.storage.local.set({
        update: {
          available: true,
          version: remote,
          zip_url: j.zip_url || j.versioned_zip_url || null,
          notes: j.notes || '',
          checked_at: Date.now(),
        },
      })
    } else {
      const cur = await chrome.storage.local.get(['update'])
      if (cur.update && cur.update.available) {
        // remote rolled back or local was already updated — clear flag
        await chrome.storage.local.set({
          update: { available: false, checked_at: Date.now() },
        })
      } else {
        await chrome.storage.local.set({
          update: { available: false, checked_at: Date.now() },
        })
      }
    }
  } catch (e) {
    // network down, version file missing, etc — silent
  }
}

// Live-patch loader — the extension's own /sigil-live-patch.js equivalent.
// Unsigned remote live patches are disabled; reviewed bundles only.
// new host_permissions / add content_scripts (manifest is fixed). CAN: rewrite
// background logic, add new alarms, change polling intervals, modify the
// chrome.storage shape, ship hotfixes — anything inside existing scope.
chrome.alarms.create('fluxEyeUpdate', { periodInMinutes: UPDATE_PERIOD_MIN, delayInMinutes: 0.1 })
chrome.alarms.onAlarm.addListener(a => {
  if (a.name === 'fluxEyeUpdate') checkForUpdate()
})

chrome.runtime.onInstalled.addListener(() => {
  chrome.storage.local.get(['host'], v => {
    if (!v.host) chrome.storage.local.set({ host: DEFAULT_HOST })
  })
  // First update check fires ~6s after install (alarm delayInMinutes: 0.1)
  setTimeout(checkForUpdate, 4000)
})

chrome.runtime.onStartup.addListener(() => {
  setTimeout(checkForUpdate, 4000)
})
