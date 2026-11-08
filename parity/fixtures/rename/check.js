const assert = require('node:assert');
const user = require('./user');
const api = require('./api');
const cli = require('./cli');

// user.js must export exactly one function, named fetchUser.
assert.deepStrictEqual(Object.keys(user), ['fetchUser']);
assert.strictEqual(typeof user.fetchUser, 'function');
assert.deepStrictEqual(user.fetchUser(7), { id: 7, name: 'user-7' });
assert.deepStrictEqual(api.handleGet(7), { status: 200, body: { id: 7, name: 'user-7' } });
assert.strictEqual(typeof cli.run, 'function');
console.log('rename check passed');
