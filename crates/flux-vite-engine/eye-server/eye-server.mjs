// sigil-eye — minimal HTTP bridge between a Chrome extension and the SIGIL dev loop.
//
// Flow:
//   Extension polls  GET  /cmd        → returns { id, fn, args } or { idle: true }
//   Extension posts  POST /result     → body { id, result } stored on disk
//   Dev loop writes  POST /cmd        → body { fn, args } queues a command
//   Dev loop reads   GET  /result/:id → returns the matching result or { pending: true }
//   Snapshot         POST /snapshot   → extension's auto-snapshot (latest only)
//   Snapshot read    GET  /snapshot   → returns the most recent snapshot
//
// Listens on 127.0.0.1:9789 — q-flux proxies wss://sigilgraph.quillon.xyz/sigil-eye/* to here.

import http from 'node:http'
import fs from 'node:fs'
import path from 'node:path'
import { createHmac, randomUUID, timingSafeEqual } from 'node:crypto'

const STATE_DIR = '/home/orobit/sigil-eye/state'
fs.mkdirSync(STATE_DIR, { recursive: true })

const SNAP_PATH = `${STATE_DIR}/snapshot.json`
const QUEUE_PATH = `${STATE_DIR}/queue.json`
const RESULT_DIR = `${STATE_DIR}/results`
fs.mkdirSync(RESULT_DIR, { recursive: true })

const readQueue = () => { try { return JSON.parse(fs.readFileSync(QUEUE_PATH, 'utf8')) } catch { return [] } }
const writeQueue = (q) => {
  const tmp = `${QUEUE_PATH}.${process.pid}.tmp`
  fs.writeFileSync(tmp, JSON.stringify(q))
  fs.renameSync(tmp, QUEUE_PATH)
}

const PORT = parseInt(process.env.SIGIL_EYE_PORT || '9789', 10)
const HOST = process.env.SIGIL_EYE_HOST || '127.0.0.1'
const TOKEN = process.env.SIGIL_EYE_TOKEN || ''
const MAX_BODY_BYTES = parseInt(process.env.SIGIL_EYE_MAX_BODY_BYTES || '1048576', 10)
const ALLOWED_ORIGINS = (process.env.SIGIL_EYE_CORS_ORIGINS || '')
  .split(',')
  .map(s => s.trim())
  .filter(Boolean)

const isLoopbackHost = (host) => (
  host === '127.0.0.1' ||
  host === 'localhost' ||
  host === '::1'
)

if (!TOKEN && !isLoopbackHost(HOST)) {
  console.error('sigil-eye refusing non-loopback bind without SIGIL_EYE_TOKEN')
  process.exit(1)
}

const cors = (req, res) => {
  const origin = req.headers.origin || ''
  if (ALLOWED_ORIGINS.includes(origin)) {
    res.setHeader('Access-Control-Allow-Origin', origin)
    res.setHeader('Vary', 'Origin')
  } else if (!TOKEN) {
    res.setHeader('Access-Control-Allow-Origin', '*')
  }
  res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization, X-Sigil-Eye-Token, X-Sigil-Eye-Ts, X-Sigil-Eye-Signature')
}

const readBody = (req, maxBytes = MAX_BODY_BYTES) => new Promise((resolve, reject) => {
  let buf = ''
  req.on('data', c => {
    buf += c
    if (Buffer.byteLength(buf) > maxBytes) {
      reject(new Error(`body too large; limit ${maxBytes} bytes`))
      req.destroy()
    }
  })
  req.on('end', () => resolve(buf))
  req.on('error', reject)
})

const safeEqual = (a, b) => {
  const aa = Buffer.from(String(a || ''))
  const bb = Buffer.from(String(b || ''))
  return aa.length === bb.length && timingSafeEqual(aa, bb)
}

const hmacFor = (req, body) => {
  const ts = String(req.headers['x-sigil-eye-ts'] || '')
  const msg = [req.method, req.url, ts, body || ''].join('\n')
  return 'sha256=' + createHmac('sha256', TOKEN).update(msg).digest('hex')
}

const isAuthorized = (req, body = '') => {
  if (!TOKEN) return true
  const bearer = String(req.headers.authorization || '').replace(/^Bearer\s+/i, '')
  if (bearer && safeEqual(bearer, TOKEN)) return true
  const token = req.headers['x-sigil-eye-token']
  if (token && safeEqual(token, TOKEN)) return true

  const ts = Number(req.headers['x-sigil-eye-ts'] || 0)
  if (!Number.isFinite(ts) || Math.abs(Date.now() - ts) > 5 * 60 * 1000) return false
  const sig = req.headers['x-sigil-eye-signature']
  return sig && safeEqual(sig, hmacFor(req, body))
}

const writeJson = (res, status, value) => {
  res.writeHead(status, { 'Content-Type': 'application/json' })
  res.end(JSON.stringify(value))
}

const resultPath = (id) => {
  if (!/^[A-Za-z0-9_-]{1,80}$/.test(String(id || ''))) return null
  const p = path.join(RESULT_DIR, `${id}.json`)
  return p.startsWith(RESULT_DIR + path.sep) ? p : null
}

const server = http.createServer(async (req, res) => {
  const t = new Date().toISOString().slice(11, 19)
  const peer = (req.socket && req.socket.remoteAddress) || '-'
  console.error(`[${t}] ${peer} ${req.method} ${req.url}`)
  cors(req, res)
  if (req.method === 'OPTIONS') { res.writeHead(204); res.end(); return }

  // Extension polls for next command
  if (req.method === 'GET' && req.url === '/cmd') {
    if (!isAuthorized(req)) return writeJson(res, 401, { error: 'unauthorized' })
    const q = readQueue()
    if (q.length === 0) {
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ idle: true }))
      return
    }
    const next = q.shift()
    writeQueue(q)
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify(next))
    return
  }

  // Extension posts a result
  if (req.method === 'POST' && req.url === '/result') {
    const body = await readBody(req).catch(e => {
      writeJson(res, 413, { error: String(e.message || e) })
      return null
    })
    if (body === null) return
    if (!isAuthorized(req, body)) return writeJson(res, 401, { error: 'unauthorized' })
    try {
      const { id, result, error } = JSON.parse(body)
      const p = resultPath(id)
      if (!p) return writeJson(res, 400, { error: 'invalid result id' })
      fs.writeFileSync(p, JSON.stringify({ ts: Date.now(), id, result, error }))
      res.writeHead(204); res.end(); return
    } catch (e) {
      res.writeHead(400); res.end(JSON.stringify({ error: String(e) })); return
    }
  }

  // Dev loop reads a result
  if (req.method === 'GET' && req.url.startsWith('/result/')) {
    if (!isAuthorized(req)) return writeJson(res, 401, { error: 'unauthorized' })
    const id = decodeURIComponent(req.url.slice('/result/'.length))
    const p = resultPath(id)
    if (!p) return writeJson(res, 400, { error: 'invalid result id' })
    if (!fs.existsSync(p)) { res.writeHead(202); res.end(JSON.stringify({ pending: true })); return }
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(fs.readFileSync(p))
    return
  }

  // Dev loop queues a command
  if (req.method === 'POST' && req.url === '/cmd') {
    const body = await readBody(req).catch(e => {
      writeJson(res, 413, { error: String(e.message || e) })
      return null
    })
    if (body === null) return
    if (!isAuthorized(req, body)) return writeJson(res, 401, { error: 'unauthorized' })
    try {
      const cmd = JSON.parse(body)
      const id = cmd.id || randomUUID().slice(0, 8)
      if (!resultPath(id)) return writeJson(res, 400, { error: 'invalid command id' })
      const q = readQueue()
      q.push({ id, ...cmd })
      writeQueue(q)
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ id, queued: true, depth: q.length }))
      return
    } catch (e) {
      res.writeHead(400); res.end(JSON.stringify({ error: String(e) })); return
    }
  }

  // Extension posts an auto-snapshot
  if (req.method === 'POST' && req.url === '/snapshot') {
    const body = await readBody(req).catch(e => {
      writeJson(res, 413, { error: String(e.message || e) })
      return null
    })
    if (body === null) return
    if (!isAuthorized(req, body)) return writeJson(res, 401, { error: 'unauthorized' })
    try {
      fs.writeFileSync(SNAP_PATH, JSON.stringify({ ts: Date.now(), ...JSON.parse(body) }))
      res.writeHead(204); res.end(); return
    } catch (e) {
      res.writeHead(400); res.end(JSON.stringify({ error: String(e) })); return
    }
  }

  // Dev loop reads the latest snapshot
  if (req.method === 'GET' && req.url === '/snapshot') {
    if (!isAuthorized(req)) return writeJson(res, 401, { error: 'unauthorized' })
    if (!fs.existsSync(SNAP_PATH)) { res.writeHead(404); res.end('no snapshot yet'); return }
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(fs.readFileSync(SNAP_PATH))
    return
  }

  // Health
  if (req.method === 'GET' && req.url === '/health') {
    if (!isAuthorized(req)) return writeJson(res, 401, { error: 'unauthorized' })
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ ok: true, queue: readQueue().length, snap: fs.existsSync(SNAP_PATH) }))
    return
  }

  res.writeHead(404); res.end('not found')
})

server.listen(PORT, HOST, () => {
  console.error(`sigil-eye server listening on http://${HOST}:${PORT}`)
})
