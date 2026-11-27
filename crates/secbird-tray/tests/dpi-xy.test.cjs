const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const html = fs.readFileSync(path.join(__dirname, '../src/editor.html'), 'utf8');

// Extract a top-level named `function` declaration up to its matching closing brace,
// without relying on comments or brittle line markers.
function extractFunction(source, name) {
  const re = new RegExp(`(?:^|\\n)\\s*function\\s+${name}\\s*\\(`);
  const match = re.exec(source);
  assert.ok(match, `missing function ${name} in editor.html`);
  const start = match.index + match[0].indexOf('function');
  let i = source.indexOf('{', start);
  assert.ok(i >= 0, `missing body for function ${name}`);
  let depth = 0;
  const modes = [];
  for (; i < source.length; i++) {
    const ch = source[i], next = source[i + 1];
    const mode = modes[modes.length - 1];
    if (mode === '//') {
      if (ch === '\n') modes.pop();
      continue;
    }
    if (mode === '/*') {
      if (ch === '*' && next === '/') { modes.pop(); i++; }
      continue;
    }
    if (mode === "'" || mode === '"') {
      if (ch === '\\') { i++; continue; }
      if (ch === mode) modes.pop();
      continue;
    }
    if (mode === '`') {
      if (ch === '\\') { i++; continue; }
      if (ch === '$' && next === '{') { modes.push('${'); depth++; i++; continue; }
      if (ch === '`') modes.pop();
      continue;
    }
    if (ch === '/' && next === '/') { modes.push('//'); i++; continue; }
    if (ch === '/' && next === '*') { modes.push('/*'); i++; continue; }
    if (ch === "'" || ch === '"' || ch === '`') { modes.push(ch); continue; }
    if (ch === '{') depth++;
    else if (ch === '}') {
      depth--;
      if (mode === '${') {
        modes.pop();
      } else if (depth === 0) {
        return source.slice(start, i + 1);
      }
    }
  }
  assert.fail(`unterminated function ${name}`);
}

const FUNCTION_NAMES = [
  'remembered', 'remember', 'live',
  'dpiLabel', 'dpiPair', 'dpiLinkStore', 'dpiDiffers', 'isDpiLinked', 'setDpiLinked',
  'dpiSplit', 'dpiLinkControl', 'dpiEditor', 'showDpi',
  'stagesOf', 'stagesEditor', 'setDpi', 'pePerfStage',
];
const editorCode = FUNCTION_NAMES.map(name => extractFunction(html, name)).join('\n');

const englishText = (message, ...values) => typeof message === 'string'
  ? message
  : message.map((part, i) => part + (i < values.length ? values[i] : '')).join('');
const tick = () => new Promise(resolve => setImmediate(resolve));
const copy = value => JSON.parse(JSON.stringify(value));

class Element {
  constructor(tag, props = {}, ...children) {
    this.tag = tag;
    this.dataset = {};
    this.children = [];
    Object.assign(this, props);
    this.append(...children);
  }
  append(...children) {
    this.children.push(...children.filter(v => v != null && v !== false));
  }
  set innerHTML(_value) {
    this.children = [];
  }
  get childElementCount() {
    return this.children.filter(c => c instanceof Element).length;
  }
  contains(node) {
    return node === this || this.children.some(c => c instanceof Element && c.contains(node));
  }
}

function descendants(node) {
  return node instanceof Element
    ? [node, ...node.children.flatMap(c => (c instanceof Element ? descendants(c) : []))]
    : [];
}

function makeMouse(id = 'mouse-1', { x = 1150, y = 1050, list = [[1150, 1050], [800, 750]] } = {}) {
  return {
    id,
    active: 'Default',
    profile: {},
    profilesData: {},
    dpi: { x, y, min: 100, max: 26000, step: 50 },
    stages: { active: 1, max: 5, list },
  };
}

function createEditor({ device = makeMouse(), handler = null, storage = new Map() } = {}) {
  const calls = [], errors = [], toasts = [], refreshed = [];
  const box = new Element('div');
  const sandbox = {
    dev: device,
    Map, WeakMap, Set, Array, Number, Math, String, Boolean,
    localStorage: {
      getItem: k => (storage.has(k) ? storage.get(k) : null),
      setItem: (k, v) => { storage.set(k, String(v)); },
    },
    uiText: englishText,
    uiI18n: { language: 'en' },
    icon: () => '',
    prof: name => (name === sandbox.dev.active ? sandbox.dev.profile : (sandbox.dev.profilesData && sandbox.dev.profilesData[name])) || {},
    refreshCell: cell => refreshed.push(cell),
    toast: (text, error) => { if (error) errors.push(text); else toasts.push(text); },
    document: {
      activeElement: null,
      querySelectorAll: selector => {
        if (selector === '#dpiPills .pill') {
          const pills = descendants(box).find(e => e.id === 'dpiPills');
          return pills ? pills.children.filter(c => c instanceof Element) : [];
        }
        return [];
      },
    },
    $: id => descendants(box).find(e => e.id === id) || null,
    el: (...args) => new Element(...args),
    renderPE: () => { box.innerHTML = ''; functions.pePerfStage(box); },
    api: async (route, body) => {
      calls.push({ route, body: copy(body) });
      if (handler) return handler(route, copy(body));
      return route === '/api/dpi'
        ? { dpi: [body.x, body.y] }
        : { stages: { active: body.active, list: copy(body.stages) }, dpi: copy(body.stages[body.active - 1]) };
    },
  };
  vm.createContext(sandbox);
  const functions = vm.runInContext(
    `${editorCode}\n({dpiLabel, dpiPair, dpiDiffers, isDpiLinked, setDpiLinked, dpiSplit, dpiEditor, stagesEditor, pePerfStage, setDpi, showDpi, stagesOf})`,
    sandbox,
  );
  return { ...functions, calls, errors, toasts, refreshed, device, storage, sandbox, box };
}

function field(box, label) {
  const found = descendants(box).find(e => e.ariaLabel === label);
  assert.ok(found, `missing control: ${label}`);
  return found;
}

function hasField(box, label) {
  return descendants(box).some(e => e.ariaLabel === label);
}

test('dpiLabel formats matching and differing pairs from objects or arrays', () => {
  const { dpiLabel } = createEditor();
  assert.equal(dpiLabel({ x: 800, y: 800 }), '800');
  assert.equal(dpiLabel([1600, 1600]), (1600).toLocaleString('en'));
  assert.equal(dpiLabel({ x: 1150, y: 1050 }), `${(1150).toLocaleString('en')} × ${(1050).toLocaleString('en')}`);
  assert.equal(dpiLabel([1150, 1050]), `${(1150).toLocaleString('en')} × ${(1050).toLocaleString('en')}`);
  assert.equal(dpiLabel(null), '');
});

test('dpiPair handles linked/unlinked edits, clamping, and invalid inputs', () => {
  const { dpiPair: pairIn } = createEditor();
  // Arrays made inside the vm sandbox have another Array prototype: copy them out.
  const dpiPair = (...args) => { const r = pairIn(...args); return r && Array.from(r); };
  const range = { min: 100, max: 26000 };
  // Linked edit sets both axes
  assert.deepEqual(dpiPair([1150, 1050], 1200, 'x', true, range), [1200, 1200]);
  assert.deepEqual(dpiPair([1150, 1050], 900, 'y', true, range), [900, 900]);
  // Explicit preset (no axis) sets both even when unlinked
  assert.deepEqual(dpiPair([1150, 1050], 800, null, false, range), [800, 800]);
  // Unlinked edit preserves the other axis
  assert.deepEqual(dpiPair([1150, 1050], 1300, 'x', false, range), [1300, 1050]);
  assert.deepEqual(dpiPair([1150, 1050], 950, 'y', false, range), [1150, 950]);
  // Clamping to min/max and rounding
  assert.deepEqual(dpiPair([800, 800], 50, 'x', false, range), [100, 800]);
  assert.deepEqual(dpiPair([800, 800], 99999, 'y', false, range), [800, 26000]);
  assert.deepEqual(dpiPair([800, 800], 1234.6, 'x', true, range), [1235, 1235]);
  // Invalid inputs return null
  for (const invalid of ['', '   ', 'abc', NaN, Infinity, -Infinity, 0, -50, null, undefined, true, false]) {
    assert.equal(dpiPair([1150, 1050], invalid, 'x', false, range), null, `expected null for ${String(invalid)}`);
  }
});

test('equal mouse defaults to linked with a single control and remembers link choice per device', () => {
  const storage = new Map();
  const e1 = createEditor({
    device: makeMouse('mouse-1', { x: 800, y: 800, list: [[800, 800], [1600, 1600]] }),
    storage,
  });
  e1.dpiEditor(e1.box);
  assert.equal(field(e1.box, 'Link X and Y').checked, true);
  assert.equal(field(e1.box, 'DPI').value, 800);
  assert.equal(field(e1.box, 'DPI slider').value, 800);
  assert.equal(hasField(e1.box, 'X DPI (horizontal)'), false);

  // Unchecking Link X and Y writes nothing, reveals X/Y controls, and persists per device
  field(e1.box, 'Link X and Y').onchange({ target: { checked: false } });
  assert.equal(e1.calls.length, 0);
  assert.equal(storage.get('secbird-dpi-link-mouse-1'), '0');
  assert.equal(field(e1.box, 'X DPI (horizontal)').value, 800);
  assert.equal(field(e1.box, 'Y DPI (vertical)').value, 800);
  assert.equal(field(e1.box, 'X DPI slider (horizontal)').value, 800);
  assert.equal(field(e1.box, 'Y DPI slider (vertical)').value, 800);

  // Reopening on the same device restores unlinked state; a second device still defaults to linked
  const e2 = createEditor({
    device: makeMouse('mouse-1', { x: 800, y: 800, list: [[800, 800], [1600, 1600]] }),
    storage,
  });
  e2.dpiEditor(e2.box);
  assert.equal(field(e2.box, 'Link X and Y').checked, false);
  assert.equal(hasField(e2.box, 'X DPI (horizontal)'), true);

  const other = createEditor({
    device: makeMouse('mouse-2', { x: 800, y: 800, list: [[800, 800], [1600, 1600]] }),
    storage,
  });
  other.dpiEditor(other.box);
  assert.equal(field(other.box, 'Link X and Y').checked, true);
  assert.equal(hasField(other.box, 'X DPI (horizontal)'), false);
});

test('differing values force link off initially even if remembered on, and turning it on does not write until next edit', async () => {
  const storage = new Map([['secbird-dpi-link-mouse-1', '1']]);
  const e = createEditor({
    // Stages match, so once the DPI itself matches again the editor can collapse.
    device: makeMouse('mouse-1', { x: 1150, y: 1050, list: [[800, 800]] }),
    storage,
  });
  e.dpiEditor(e.box);
  const link = field(e.box, 'Link X and Y');
  assert.equal(link.checked, false);
  assert.equal(field(e.box, 'X DPI (horizontal)').value, 1150);
  assert.equal(field(e.box, 'Y DPI (vertical)').value, 1050);

  // Turning link ON keeps both values visible and sends nothing
  link.onchange({ target: { checked: true } });
  assert.equal(e.calls.length, 0);
  assert.deepEqual([e.device.dpi.x, e.device.dpi.y], [1150, 1050]);
  assert.equal(field(e.box, 'Link X and Y').checked, true);
  assert.equal(field(e.box, 'X DPI (horizontal)').value, 1150);
  assert.equal(field(e.box, 'Y DPI (vertical)').value, 1050);

  // Next edit applies to both axes and collapses back to single control once all values match
  field(e.box, 'X DPI (horizontal)').onchange({ target: { value: '1200' } });
  await tick();
  assert.deepEqual(e.calls[0].body, { id: 'mouse-1', x: 1200, y: 1200 });
  assert.equal(field(e.box, 'DPI').value, 1200);
  assert.equal(hasField(e.box, 'X DPI (horizontal)'), false);
});

test('setDpi keeps pending axis edits, targets original device, and rolls back on error', async () => {
  const pending = [];
  const e = createEditor({
    handler: (route, body) => new Promise((resolve, reject) => pending.push({ body, resolve, reject })),
  });
  e.setDpi(1250, 'x');
  e.setDpi(950, 'y');
  e.setDpi(1350, 'x');
  assert.equal(e.calls.length, 1);
  pending.shift().resolve({ dpi: [1250, 1050] });
  await tick();
  assert.equal(e.calls.length, 2);
  assert.deepEqual(e.calls[1].body, { id: 'mouse-1', x: 1350, y: 950 });
  assert.deepEqual([e.device.dpi.x, e.device.dpi.y], [1350, 950]);
  pending.shift().resolve({ dpi: [1350, 950] });
  await tick();
  assert.deepEqual(copy(e.device.profile.dpi), [1350, 950]);

  // Switch device mid-request
  e.setDpi(1500, 'x');
  const other = makeMouse('mouse-2', { x: 800, y: 800, list: [[800, 800]] });
  e.sandbox.dev = other;
  pending.shift().resolve({ dpi: [1500, 950] });
  await tick();
  assert.deepEqual([other.dpi.x, other.dpi.y], [800, 800]);
  assert.deepEqual([e.device.dpi.x, e.device.dpi.y], [1500, 950]);

  // Failed write restores confirmed pair
  e.sandbox.dev = e.device;
  e.setDpi(2000, 'x');
  assert.equal(e.device.dpi.x, 2000);
  pending.shift().reject(new Error('device busy'));
  await tick();
  assert.deepEqual([e.device.dpi.x, e.device.dpi.y], [1500, 950]);
  assert.deepEqual(e.errors, ['device busy']);
});

test('showDpi updates both axes without overwriting dirty typed input', () => {
  const e = createEditor();
  e.dpiEditor(e.box);
  const x = field(e.box, 'X DPI (horizontal)');
  const y = field(e.box, 'Y DPI (vertical)');
  x.dataset.dirty = '1';
  x.value = '123';
  e.device.dpi.x = 1600;
  e.device.dpi.y = 1400;
  e.showDpi();
  assert.equal(x.value, '123');
  assert.equal(y.value, 1400);
  assert.equal(field(e.box, 'X DPI slider (horizontal)').value, 1600);
  assert.equal(field(e.box, 'Y DPI slider (vertical)').value, 1400);
});

test('classic stagesEditor preserves independent X/Y, doubles both on Add stage, and guards stale responses', async () => {
  const e = createEditor();
  e.stagesEditor(e.box);
  field(e.box, 'Stage 1 X DPI (horizontal)').onchange({ target: { value: '1250' } });
  await tick();
  assert.deepEqual(e.calls[0].body, { id: 'mouse-1', active: 1, stages: [[1250, 1050], [800, 750]] });
  field(e.box, 'Stage 1 Y DPI (vertical)').onchange({ target: { value: '950' } });
  await tick();
  assert.deepEqual(e.calls[1].body.stages, [[1250, 950], [800, 750]]);
  field(e.box, 'Add stage').onclick();
  await tick();
  assert.deepEqual(e.calls[2].body.stages, [[1250, 950], [800, 750], [1600, 1500]]);

  // Turning link on does not write; next edit flattens that stage
  field(e.box, 'Link X and Y').onchange({ target: { checked: true } });
  assert.equal(e.calls.length, 3);
  field(e.box, 'Stage 2 Y DPI (vertical)').onchange({ target: { value: '1100' } });
  await tick();
  assert.deepEqual(e.calls[3].body.stages, [[1250, 950], [1100, 1100], [1600, 1500]]);
});

test('new mouse editor pePerfStage defaults to single control, splits when unlinked or differing, and doubles both axes on Add stage', async () => {
  const equal = createEditor({
    device: makeMouse('mouse-1', { x: 800, y: 800, list: [[800, 800], [1600, 1600]] }),
  });
  equal.pePerfStage(equal.box);
  assert.equal(field(equal.box, 'Stage 1 DPI').value, 800);
  assert.equal(hasField(equal.box, 'Stage 1 X DPI (horizontal)'), false);

  field(equal.box, 'Link X and Y').onchange({ target: { checked: false } });
  assert.equal(equal.calls.length, 0);
  const x1 = field(equal.box, 'Stage 1 X DPI (horizontal)');
  assert.equal(x1.value, 800);
  x1.value = '900';
  x1.onchange();
  await tick();
  assert.deepEqual(equal.calls[0].body, { id: 'mouse-1', active: 1, stages: [[900, 800], [1600, 1600]] });

  const asym = createEditor({
    device: makeMouse('mouse-2', { x: 1150, y: 1050, list: [[1150, 1050], [800, 750]] }),
  });
  asym.pePerfStage(asym.box);
  assert.equal(field(asym.box, 'Link X and Y').checked, false);
  assert.equal(field(asym.box, 'Stage 1 X DPI (horizontal)').value, 1150);
  assert.equal(field(asym.box, 'Stage 1 Y DPI (vertical)').value, 1050);
  field(asym.box, 'Add stage').onclick();
  await tick();
  assert.deepEqual(asym.calls[0].body.stages, [[1150, 1050], [800, 750], [1600, 1500]]);
});

test('editor.html no longer flattens X/Y DPI or stage arrays or displays X-only DPI summaries', () => {
  const stagesSrc = extractFunction(html, 'stagesEditor');
  const pePerfSrc = extractFunction(html, 'pePerfStage');
  assert.doesNotMatch(stagesSrc, /\[\s*([A-Za-z_$][\w$]*)\s*,\s*\1\s*\]/);
  assert.doesNotMatch(pePerfSrc, /\[\s*([A-Za-z_$][\w$]*)\s*,\s*\1\s*\]/);
  assert.doesNotMatch(html, /api\(\s*['"]\/api\/dpi['"]\s*,\s*\{[^}]*\bx\s*:\s*([A-Za-z_$][\w$]*)\s*,\s*y\s*:\s*\1\b/);
  assert.doesNotMatch(html, /dev\.dpi\.x\s*=\s*dev\.dpi\.y\s*=/);

  const railSrc = extractFunction(html, 'railSummary');
  const cellSrc = extractFunction(html, 'cellView');
  const inspSrc = extractFunction(html, 'peInspector');
  const ovSrc = extractFunction(html, 'renderOverview');
  assert.match(railSrc, /dpiLabel\(dev\.dpi\)/);
  assert.match(cellSrc, /dpiLabel\(d\)/);
  assert.match(inspSrc, /dpiLabel\(dev\.dpi\)/);
  assert.match(ovSrc, /dpiLabel\(d\.dpi\)/);
});
