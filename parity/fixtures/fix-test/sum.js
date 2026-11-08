// Adds up every number in the list.
function sum(list) {
  let total = 0;
  for (let i = 0; i < list.length - 1; i++) {
    total += list[i];
  }
  return total;
}

module.exports = { sum };
