const { withRetry } = require('./retry');

async function fetchJson(url) {
  return withRetry(async () => {
    const res = await fetch(url);
    return res.json();
  });
}

module.exports = { fetchJson };
