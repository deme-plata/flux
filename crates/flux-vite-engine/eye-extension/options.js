// External script (MV3 blocks inline <script> in extension pages → the inline
// version never wired the Save button; this fixes "nothing happens on input").
const $ = id => document.getElementById(id)
chrome.storage.local.get(['host', 'token'], v => {
  $('host').value = v.host || 'http://89.149.241.126:9789'
  $('token').value = v.token || ''
})
$('save').addEventListener('click', async () => {
  const h = $('host').value.trim().replace(/\/$/, '')
  const tok = $('token').value.trim()
  await chrome.storage.local.set({ host: h, token: tok, eyeToken: tok, eye_token: tok })
  try {
    const r = await fetch(h + '/health', { cache: 'no-store', headers: tok ? { 'Authorization': 'Bearer ' + tok } : {} })
    const j = await r.json()
    $('status').textContent = `✓ saved · server up · queue=${j.queue} · snap=${j.snap ? 'yes' : 'no'}`
    $('status').className = 'status show ok'
  } catch (e) {
    $('status').textContent = '✕ saved, but server unreachable: ' + (e.message || e)
    $('status').className = 'status show no'
  }
})
