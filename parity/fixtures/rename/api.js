const { getUser } = require('./user');

function handleGet(id) {
  return { status: 200, body: getUser(id) };
}

module.exports = { handleGet };
