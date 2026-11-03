#!/usr/bin/env node
// Measure the QuickJS-side memory pressure of one act, with presets sized like
// the real game data. Answers: does act 3 approach d2rlmm's 512 MB sandbox cap?
//
//   node mem-probe.js "<act folder>" [entitiesPerPreset]

const fs = require('fs');
const path = require('path');
const vm = require('vm');
const crypto = require('crypto');

const MOD_DIR = path.resolve(process.argv[2]);
const ENTITIES = parseInt(process.argv[3] || '4000', 10);
const modJson = JSON.parse(fs.readFileSync(path.join(MOD_DIR, 'mod.json'), 'utf8'));

const config = {};
const walk = (nodes) => {
  for (const node of nodes || []) {
    if (node.children) walk(node.children);
    else if (node.id && node.defaultValue !== undefined) config[node.id] = node.defaultValue;
  }
};
walk(modJson.config);
if (process.env.CONFIG) Object.assign(config, JSON.parse(process.env.CONFIG));

// A preset shaped like the real thing: thousands of entity objects with components.
const makePreset = () => {
  const entities = [];
  for (let i = 0; i < ENTITIES; i++) {
    entities.push({
      type: 'Entity',
      name: 'entity_root_' + i,
      id: 1000000 + i,
      components: [
        { type: 'TransformDefinitionComponent', name: 't' + i,
          position: { x: 1.5, y: 2, z: 3 }, orientation: { x: 0, y: 0, z: 0, w: 1 },
          scale: { x: 1, y: 1, z: 1 }, inheritOnlyPosition: false },
        { type: 'VfxDefinitionComponent', name: 'v' + i,
          filename: 'data/hd/env/preset/fake/' + i + '.particles', hardKillOnDestroy: false },
      ],
    });
  }
  return { fx: 'x', entities };
};

let peak = 0;
const sample = () => {
  const used = process.memoryUsage().heapUsed;
  if (used > peak) peak = used;
};

let writes = 0;
let bytes = 0;
const D2RMM = {
  getVersion: () => 1.9,
  readJson: () => { sample(); return makePreset(); },
  readTsv: () => {
    const rows = [];
    for (let i = 0; i < 24; i++) {
      rows.push({ File1: 'hd\\e\\r' + i + '.ds1', File2: '0', File3: '0',
                  File4: '0', File5: '0', File6: '0', SizeX: '40', SizeY: '40' });
    }
    return { rows };
  },
  // Mimic the boundary cost: the manager re-encodes the payload as JSON text.
  writeTxt: (rel, data) => {
    const text = String(data);
    const wire = JSON.stringify([rel, text]);
    writes += 1;
    bytes += wire.length;
    sample();
  },
  writeJson: () => {},
  copyFile: () => {},
  nextStringId: () => 0,
};

const sandbox = {
  D2RMM, config,
  console: { log: () => {}, warn: () => {}, error: () => {} },
  JSON, Math, Object, Array, String, Number, Boolean, Set, Map, Date, Error,
};

let error = null;
try {
  vm.runInNewContext(fs.readFileSync(path.join(MOD_DIR, 'mod.js'), 'utf8'), sandbox, { filename: 'mod.js' });
} catch (e) {
  error = String(e && e.message);
}

console.log(JSON.stringify({
  mod: path.basename(MOD_DIR),
  entitiesPerPreset: ENTITIES,
  presetJSONKB: Math.round(JSON.stringify(makePreset()).length / 1024),
  writes,
  wireMB: +(bytes / 1048576).toFixed(1),
  peakHeapMB: Math.round(peak / 1048576),
  error,
}, null, 1));
