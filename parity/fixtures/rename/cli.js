const { getUser } = require('./user');

function run(argv) {
  const id = argv[2] || '1';
  console.log(JSON.stringify(getUser(id)));
}

if (require.main === module) {
  run(process.argv);
}

module.exports = { run };
