const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

test('editor.html contains no browser confirm(), alert() or prompt() calls', () => {
  const html = fs.readFileSync(path.join(__dirname, '../src/editor.html'), 'utf8');
  const withoutComments = html.replace(/<!--[\s\S]*?-->|\/\*[\s\S]*?\*\/|(?:^|\s)\/\/.*$/gm, '');
  assert.doesNotMatch(withoutComments, /\b(?:confirm|alert|prompt)\s*\(/);
});
