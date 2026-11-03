#!/usr/bin/env node
// Build a synthetic "extracted data" tree that satisfies one wayfinding act:
// every preset the mod reads exists, and each is big enough to be realistic.
//
//   node make-fixture.js "<act folder>" <out data dir> [entitiesPerPreset]
//
// The file list is discovered by running the mod with a recording readJson, so the
// fixture cannot drift from what the mod actually asks for.

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const MOD_DIR = path.resolve(process.argv[2]);
const OUT = path.resolve(process.argv[3]);
const ENTITIES = parseInt(process.argv[4] || '2000', 10);

const modJson = JSON.parse(fs.readFileSync(path.join(MOD_DIR, 'mod.json'), 'utf8'));
const config = {};
const walk = (nodes) => {
  for (const node of nodes || []) {
    if (node.children) walk(node.children);
    else if (node.id && node.defaultValue !== undefined) config[node.id] = node.defaultValue;
  }
};
walk(modJson.config);

const readPaths = new Set();
const writePaths = new Set();

const D2RMM = {
  getVersion: () => 1.9,
  readJson: (rel) => { readPaths.add(rel); return { entities: [] }; },
  readTsv: () => {
    const rows = [];
    for (let i = 0; i < 32; i++) {
      rows.push({ File1: 'hd\\env\\preset\\fake\\r' + i + '.ds1', File2: '0',
                  File3: '0', File4: '0', File5: '0', File6: '0',
                  SizeX: '48', SizeY: '40' });
    }
    return { rows };
  },
  writeTxt: (rel) => writePaths.add(rel),
  writeJson: (rel) => writePaths.add(rel),
  copyFile: () => {},
  nextStringId: () => 0,
};

const sandbox = {
  D2RMM, config,
  console: { log: () => {}, warn: () => {}, error: () => {} },
  JSON, Math, Object, Array, String, Number, Boolean, Set, Map, Date, Error,
};
vm.runInNewContext(fs.readFileSync(path.join(MOD_DIR, 'mod.js'), 'utf8'), sandbox, { filename: 'mod.js' });

// A preset shaped like the real thing, big enough to matter.
function preset(i) {
  const entities = [];
  for (let n = 0; n < ENTITIES; n++) {
    entities.push({
      type: 'Entity',
      name: 'entity_' + i + '_' + n,
      id: 2000000 + i * 10000 + n,
      components: [
        { type: 'TransformDefinitionComponent', name: 't',
          position: { x: n * 0.5, y: 2, z: n * 0.25 },
          orientation: { x: 0, y: 0, z: 0, w: 1 },
          scale: { x: 1, y: 1, z: 1 }, inheritOnlyPosition: false },
        { type: 'VfxDefinitionComponent', name: 'v',
          filename: 'data/hd/env/preset/fake/' + n + '.particles', hardKillOnDestroy: false },
      ],
    });
  }
  return { fx: 'stuff', entities };
}

let created = 0;
let bytes = 0;
[...readPaths].sort().forEach((rel, idx) => {
  const full = path.join(OUT, 'data', rel.replace(/\\/g, '/'));
  fs.mkdirSync(path.dirname(full), { recursive: true });
  const text = JSON.stringify(preset(idx), null, 2);
  fs.writeFileSync(full, text);
  created += 1;
  bytes += text.length;
});

// lvlprest.txt so getEnvPresetSize() has something to read. Tab separated with CRLF,
// and the consumed columns are deliberately not last.
const tsvLines = ['File1\tFile2\tFile3\tFile4\tFile5\tFile6\tSizeX\tSizeY\tName'];
for (let i = 0; i < 32; i++) {
  tsvLines.push([`hd\\env\\preset\\fake\\r${i}.ds1`, '0', '0', '0', '0', '0', '48', '40', `row${i}`].join('\t'));
}
const tsvPath = path.join(OUT, 'data', 'global', 'excel', 'lvlprest.txt');
fs.mkdirSync(path.dirname(tsvPath), { recursive: true });
fs.writeFileSync(tsvPath, '\uFEFF' + tsvLines.join('\r\n') + '\r\n');

console.log(JSON.stringify({
  mod: path.basename(MOD_DIR),
  presetFilesCreated: created,
  avgPresetKB: created ? Math.round(bytes / created / 1024) : 0,
  totalMB: +(bytes / 1048576).toFixed(1),
  writeTargets: writePaths.size,
  readPathsSample: [...readPaths].slice(0, 3),
}, null, 1));
