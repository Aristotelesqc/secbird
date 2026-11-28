const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const i18n = require('../src/i18n.js');
const editorHtml = fs.readFileSync(path.join(__dirname, '..', 'src', 'editor.html'), 'utf8');

test('round 2 UI strings have French entries and placeholder formatting', () => {
  const keys = [
    'Mouse settings',
    'What is HyperShift?',
    'HyperShift is a second layer. While the HyperShift button is held, every button does its HyperShift action instead.',
    'Pick a button to choose what it does. HyperShift button: {0}.',
    'Pick a button in Standard',
    'On-board memory…',
    'Mouse on-board memory',
    'Use stage {0}',
    'Removed stage {0} · saved to {1}',
    'Record steps',
    'Stop capturing',
    'Write to device',
  ];
  for (const key of keys) {
    assert.ok(Object.prototype.hasOwnProperty.call(i18n.frMessages, key), `Missing FR entry for: ${key}`);
  }
  const fr = i18n.create({ preference: 'fr' });
  assert.match(fr.text`Pick a button to choose what it does. HyperShift button: ${'Side trigger'}.`, /Side trigger/);
  assert.match(fr.text`Use stage ${2}`, /2/);
  assert.match(fr.text`Removed stage ${1} · saved to ${'Default'}`, /1.*Default/);
});

test('mouse editor implements #10 and #12 guidance, toolbar separation, and keyboard access', () => {
  // #10: HyperShift help popover, holder name in hint, warning CTA, scroll card under drawing, stable pe-bar height
  assert.match(editorHtml, /id:\s*'peHsQ'/);
  assert.match(editorHtml, /id:\s*'peHsPop'/);
  assert.match(editorHtml, /\.pe-bar\s*\{[^}]*min-height:\s*82px/);
  assert.match(editorHtml, /if\s*\(dev\.scroll\)\s*stage\.append\(peScrollCard\(\)\)/);

  // Toolbar fix: separator and On-board memory label, keeping restoreAllControl(() => peBar())
  assert.match(editorHtml, /className:\s*'pe-sep'/);
  assert.match(editorHtml, /restoreAllControl\(\(\)\s*=>\s*peBar\(\)\)/);
  assert.match(editorHtml, /uiText\('On-board memory…'\)/);

  // #12: tablist ARIA pattern, DPI stage group + radio + 24x24 remove + undo, list Space/Up/Down, unambiguous macro buttons
  assert.match(editorHtml, /b\.setAttribute\('aria-controls',\s*'peStage'\)/);
  assert.match(editorHtml, /stage\.setAttribute\('aria-labelledby',\s*'peTab-'\s*\+\s*peTab\)/);
  assert.match(editorHtml, /tabIndex:\s*id\s*===\s*peTab\s*\?\s*0\s*:\s*-1/);
  assert.match(editorHtml, /e\.key\s*===\s*'ArrowRight'/);
  assert.match(editorHtml, /e\.key\s*===\s*'Home'/);
  assert.match(editorHtml, /e\.key\s*===\s*'End'/);

  assert.match(editorHtml, /role:\s*'group',\s*ariaLabel:\s*uiText\('Stage '\)/);
  assert.match(editorHtml, /type:\s*'radio',\s*name:\s*'peStageUse'/);
  assert.match(editorHtml, /\.pe-st \.x\s*\{[^}]*width:\s*24px;\s*height:\s*24px;/);
  assert.match(editorHtml, /cur\.splice\(Math\.min\(i,\s*cur\.length\),\s*0,\s*removed\)/);

  assert.match(editorHtml, /e\.key\s*===\s*'Enter'\s*\|\|\s*e\.key\s*===\s*' '/);
  assert.match(editorHtml, /e\.key\s*===\s*'ArrowDown'\s*\|\|\s*e\.key\s*===\s*'ArrowUp'/);

  assert.match(editorHtml, /uiText\('Record steps'\)/);
  assert.match(editorHtml, /uiText\('Write to device'\)/);
});
