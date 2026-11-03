#!/usr/bin/env node
// Run a wayfinding act folder against a synthetic D2R tree and dump every effect
// as JSON, so two versions of the mod can be compared exactly.
//
//   node compare-run.js "<mod folder>" > out.json
//
// The point is to answer "did translating the comments change behaviour?" by
// feeding identical input to the Korean original and the English version and
// diffing everything they produce.

const fs = require('fs');
const path = require('path');
const vm = require('vm');
const crypto = require('crypto');

const MOD_DIR = path.resolve(process.argv[2]);
const modJson = JSON.parse(fs.readFileSync(path.join(MOD_DIR, 'mod.json'), 'utf8'));

// config defaults straight from the manifest, exactly as a manager would build them
const config = {};
const walk = (nodes) => {
  for (const node of nodes || []) {
    if (node.children) walk(node.children);
    else if (node.id && node.defaultValue !== undefined) config[node.id] = node.defaultValue;
  }
};
walk(modJson.config);

const logs = [];
const writes = new Map();   // path -> sha256 of content (+ length)
const copies = [];
let peakHeap = 0;

const emptyPreset = () => ({
  fx: 'x',
  entities: [{ type: 'Entity', name: 'pre_existing', id: 1, components: [] }],
});

const D2RMM = {
  getVersion: () => 1.9,
  // Every preset the mod asks for 'exists' and looks the same, so both versions
  // see identical input. Fresh object per call, like a real readJson.
  readJson: () => emptyPreset(),
  readTsv: () => {
    // the mod walks File1..File6 on every row and calls .toLowerCase() on each
    const rows = [];
    for (let i = 0; i < 24; i++) {
      rows.push({
        File1: 'hd\\env\\preset\\fake\\room' + i + '.ds1',
        File2: 'hd\\env\\preset\\fake\\room' + i + 'b.ds1',
        File3: '0', File4: '0', File5: '0', File6: '0',
        SizeX: '40', SizeY: '40',
      });
    }
    return { rows };
  },
  writeJson: (rel, data) => {
    const text = JSON.stringify(data);
    writes.set(rel, { sha: crypto.createHash('sha256').update(text).digest('hex'), len: text.length });
  },
  writeTxt: (rel, data) => {
    const text = String(data);
    writes.set(rel, { sha: crypto.createHash('sha256').update(text).digest('hex'), len: text.length });
    const heap = process.memoryUsage().heapUsed;
    if (heap > peakHeap) peakHeap = heap;
  },
  copyFile: (src, dst, overwrite) => copies.push([src, dst, Boolean(overwrite)]),
  nextStringId: () => 0,
};

const sandbox = {
  D2RMM,
  config,
  console: {
    log: (m) => logs.push(String(m)),
    warn: (m) => logs.push('WARN ' + m),
    error: (m) => logs.push('ERR ' + m),
  },
  JSON, Math, Object, Array, String, Number, Boolean, Set, Map, Date, Error,
};

let completed = true;
let error = null;
try {
  vm.runInNewContext(fs.readFileSync(path.join(MOD_DIR, 'mod.js'), 'utf8'), sandbox, {
    filename: 'mod.js',
  });
} catch (e) {
  completed = false;
  error = { name: e && e.name, message: e && e.message, isError: e instanceof Error };
}

const writeList = [...writes.entries()]
  .map(([p, v]) => [p, v.sha, v.len])
  .sort((a, b) => (a[0] < b[0] ? -1 : 1));

process.stdout.write(JSON.stringify({
  mod: path.basename(MOD_DIR),
  completed,
  error,
  logLines: logs.length,
  lastLogs: logs.slice(-3),
  writeCount: writes.size,
  totalBytes: writeList.reduce((n, [, , len]) => n + len, 0),
  peakHeapMB: Math.round(peakHeap / 1048576),
  copies: copies.length,
  writes: writeList,
}, null, 1));
