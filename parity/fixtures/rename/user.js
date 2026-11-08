function getUser(id) {
  return { id, name: `user-${id}` };
}

module.exports = { getUser };
