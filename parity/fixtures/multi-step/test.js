const assert = require('node:assert');
const { countWords, topWords } = require('./wordcount');

assert.strictEqual(countWords(''), 0);
assert.strictEqual(countWords('  one   two  three '), 3);
assert.strictEqual(countWords('a\nb\tc'), 3);

assert.deepStrictEqual(topWords('the cat and the hat and the bat', 2), [['the', 3], ['and', 2]]);
assert.deepStrictEqual(topWords('B a b A c', 3), [['a', 2], ['b', 2], ['c', 1]]);
assert.deepStrictEqual(topWords('x', 5), [['x', 1]]);
assert.deepStrictEqual(topWords('The the THE', 1), [['the', 3]]);
console.log('wordcount tests passed');
