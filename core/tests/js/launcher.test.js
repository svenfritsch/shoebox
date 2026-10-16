// Tests for the path helpers of the Control Panel (core/launcher-web/launcher.js).
// Run by core/tests/launcher_js.rs (`node` needed); or: node core/tests/js/launcher.test.js
const fs = require('fs');
const path = require('path');
const assert = require('assert');

const source = fs.readFileSync(path.join(__dirname, '../../launcher-web/launcher.js'), 'utf8');
const from = source.indexOf('function driveName');
const to = source.indexOf('// The photo app cannot start');
assert(from >= 0 && to > from, 'helpers not found in launcher.js');
const code = source.slice(from, to);
// The helpers read `drives` and `drivesLoaded`, which the page keeps.
const load = (drives, drivesLoaded) =>
  new Function('drives', 'drivesLoaded', code + ';return { driveName, winKey, samePath, isOffline };')(drives, drivesLoaded);

const mixed = load([{ path: 'E:\\' }, { path: '/Volumes/A' }], true);

// winKey: letter in capitals, `\`, no trailing separator; null for other paths.
assert.strictEqual(mixed.winKey('e:/Fotos/'), 'E:\\Fotos');
assert.strictEqual(mixed.winKey('E:'), 'E:');
assert.strictEqual(mixed.winKey('E:\\'), 'E:');
assert.strictEqual(mixed.winKey('/Volumes/A'), null);
assert.strictEqual(mixed.winKey('\\\\server\\share'), null);
assert.strictEqual(mixed.winKey('Ex:\\'), null);

// samePath: one place on Windows, exact text everywhere else.
assert(mixed.samePath('e:/', 'E:\\'));
assert(mixed.samePath('E:', 'E:\\'));
assert(!mixed.samePath('E:\\a', 'E:\\b'));
assert(!mixed.samePath('/a', '/A'), 'APFS can be case-sensitive');
assert(!mixed.samePath('/a', 'E:\\a'));

// isOffline: a letter nobody lists, or a /Volumes name nobody lists.
assert(!mixed.isOffline('E:\\Fotos'));
assert(!mixed.isOffline('e:/Fotos'));
assert(!mixed.isOffline('E:'));
assert(mixed.isOffline('F:\\Fotos'));
assert(mixed.isOffline('f:'));
assert(!mixed.isOffline('\\\\server\\share'), 'no letter, nothing to say');
assert(mixed.isOffline('/Volumes/B/x'));
assert(!mixed.isOffline('/Volumes/A/x'));
assert(!mixed.isOffline('/home/anna/Photos'));
assert(!load([], false).isOffline('F:\\'), 'before the list is loaded nothing is offline');
assert(load([], true).isOffline('F:\\'), 'no drives at all');

// driveName knows the letter once the \\?\ prefix is gone.
assert.strictEqual(mixed.driveName('E:\\Fotos\\0Büro'), 'E:');
assert.strictEqual(mixed.driveName('/Volumes/tesselina\'s harddrive/Photos'), 'tesselina\'s harddrive');

console.log('launcher.js helpers ok');
