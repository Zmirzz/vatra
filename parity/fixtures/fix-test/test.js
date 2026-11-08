const assert = require('node:assert');
const { sum } = require('./sum');

assert.strictEqual(sum([1, 2, 3]), 6);
assert.strictEqual(sum([]), 0);
assert.strictEqual(sum([5]), 5);
console.log('sum tests passed');
