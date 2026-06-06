#!/usr/bin/env node
/**
 * Flux Aether control surface — Excel prototype generator (pure Node, no deps).
 * Output: Flux_Prototype_1_Windows.xlsx (7 sheets per FLUX_SOURCE_EDITING.md)
 */
import { writeFileSync } from 'node:fs';
import { deflateRawSync } from 'node:zlib';

const OUT = 'Flux_Prototype_1_Windows.xlsx';

const SHEETS = [
  {
    name: 'Prototype 1',
    rows: [
      ['Area', 'Priority', 'Description'],
      ['Recorder', 'P0', 'Screen + audio capture with consent gate'],
      ['Presence Video', 'P0', 'Webcam opt-in floating overlay'],
      ['MCP Command Surface', 'P0', 'flux_ui_* deploy + cache-bust workflow'],
      ['Operator Skills', 'P0', 'Movement/writing/command style policies'],
      ['Auto Updater', 'P0', 'Check → verify → apply → restart → rollback'],
    ],
  },
  {
    name: 'MCP Commands',
    rows: [
      ['Command', 'Input', 'Output', 'Endpoint'],
      ['flux_ui_list', 'ext?', 'surfaces + cache-busted URLs', 'fluxc-mcp'],
      ['flux_ui_deploy', 'file, content', 'cache-busted URL', 'fluxc-mcp'],
      ['flux_ui_read', 'file', 'deployed source', 'fluxc-mcp'],
      ['flux_ui_preview', 'file', 'fresh ?v= URL', 'fluxc-mcp'],
      ['flux_launcher_register', 'appId, title, src', 'desktop.html?v=', 'fluxc-mcp'],
    ],
  },
  {
    name: 'Recorder Presence',
    rows: [
      ['Feature', 'Policy'],
      ['Webcam opt-in', 'Explicit toggle before stream'],
      ['Floating video', 'Draggable, always-on-top optional'],
      ['Overlay reflow', 'Resize does not clip MCP panel'],
    ],
  },
  {
    name: 'Operator Skills',
    rows: [
      ['Skill', 'Learns', 'Gate'],
      ['Movement', 'Mouse/keyboard cadence', 'Human confirm first 10 actions'],
      ['Writing', 'Tone + structure', 'Two-mind veto on money paths'],
      ['Command style', 'flux_combo preferences', 'VARFLOW honest checklist'],
    ],
  },
  {
    name: 'Auto Updater',
    rows: [
      ['Stage', 'Action', 'Rollback'],
      ['Check', 'flux_release_check manifest', 'N/A'],
      ['Verify', 'sha256 + blake3 match', 'Hold publish'],
      ['Apply', 'fluxc auto-update --apply', 'Restore previous binary'],
      ['Restart', 'systemd fluxc serve', 'flux-self-heal'],
    ],
  },
  {
    name: 'Event Schema',
    rows: [
      ['Event', 'Required fields', 'Optional'],
      ['ui_deploy', 'file, sha256, ts_ms', 'agent, notes'],
      ['recorder_start', 'session_id, consent', 'resolution'],
      ['updater_apply', 'version, product', 'publisher'],
    ],
  },
  {
    name: 'Test Plan',
    rows: [
      ['ID', 'Priority', 'Gate', 'Pass criteria'],
      ['TP-01', 'P0', 'Excel generator', '7 sheets, valid xlsx zip'],
      ['TP-02', 'P0', 'flux_ui_deploy', 'Returns ?v= cache-busted URL'],
      ['TP-03', 'P0', 'flux_ui_list', 'Lists dist-final surfaces'],
      ['TP-04', 'P1', 'flux_launcher_register', 'APPS + tile + dock wired'],
      ['TP-05', 'P0', 'v0.25 promote gate', '5/5 gates green on testnet'],
    ],
  },
];

function esc(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function sheetXml(rows) {
  const cells = rows
    .map((row, r) => {
      const cs = row
        .map((v, c) => {
          const ref = String.fromCharCode(65 + c) + (r + 1);
          return `<c r="${ref}" t="inlineStr"><is><t>${esc(v)}</t></is></c>`;
        })
        .join('');
      return `<row r="${r + 1}">${cs}</row>`;
    })
    .join('');
  return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>${cells}</sheetData>
</worksheet>`;
}

function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
  }
  return ~c >>> 0;
}

function u16(n) {
  const b = Buffer.alloc(2);
  b.writeUInt16LE(n);
  return b;
}
function u32(n) {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
}

function zipStore(files) {
  const parts = [];
  const central = [];
  let offset = 0;
  for (const [name, data] of files) {
    const nameBuf = Buffer.from(name, 'utf8');
    const crc = crc32(data);
    const local = Buffer.concat([
      u32(0x04034b50),
      u16(20),
      u16(0),
      u16(0),
      u16(0),
      u16(0),
      u32(crc),
      u32(data.length),
      u32(data.length),
      u16(nameBuf.length),
      u16(0),
      nameBuf,
      data,
    ]);
    parts.push(local);
    const cen = Buffer.concat([
      u32(0x02014b50),
      u16(20),
      u16(20),
      u16(0),
      u16(0),
      u16(0),
      u16(0),
      u32(crc),
      u32(data.length),
      u32(data.length),
      u16(nameBuf.length),
      u16(0),
      u16(0),
      u16(0),
      u16(0),
      u32(0),
      u32(offset),
      nameBuf,
    ]);
    central.push(cen);
    offset += local.length;
  }
  const centralBuf = Buffer.concat(central);
  const end = Buffer.concat([
    u32(0x06054b50),
    u16(0),
    u16(0),
    u16(files.length),
    u16(files.length),
    u32(centralBuf.length),
    u32(offset),
    u16(0),
  ]);
  return Buffer.concat([...parts, centralBuf, end]);
}

const sheetEntries = SHEETS.map((s, i) => ({
  path: `xl/worksheets/sheet${i + 1}.xml`,
  xml: sheetXml(s.rows),
  name: s.name,
  id: i + 1,
}));

const contentTypes = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
${sheetEntries.map((s) => `  <Override PartName="/${s.path}" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`).join('\n')}
</Types>`;

const workbook = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
${sheetEntries.map((s) => `    <sheet name="${esc(s.name)}" sheetId="${s.id}" r:id="rId${s.id}"/>`).join('\n')}
  </sheets>
</workbook>`;

const workbookRels = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
${sheetEntries.map((s) => `  <Relationship Id="rId${s.id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet${s.id}.xml"/>`).join('\n')}
</Relationships>`;

const rootRels = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>`;

const files = [
  ['[Content_Types].xml', Buffer.from(contentTypes, 'utf8')],
  ['_rels/.rels', Buffer.from(rootRels, 'utf8')],
  ['xl/workbook.xml', Buffer.from(workbook, 'utf8')],
  ['xl/_rels/workbook.xml.rels', Buffer.from(workbookRels, 'utf8')],
  ...sheetEntries.map((s) => [s.path, Buffer.from(s.xml, 'utf8')]),
];

const xlsx = zipStore(files);
writeFileSync(OUT, xlsx);
console.log(`✓ wrote ${OUT} (${SHEETS.length} sheets, ${xlsx.length} bytes)`);