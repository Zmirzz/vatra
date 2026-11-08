const MAX_RETRIES = 7;

// Calls fn until it succeeds or the retries run out.
async function withRetry(fn) {
  let lastError;
  for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
    try {
      return await fn();
    } catch (err) {
      lastError = err;
    }
  }
  throw lastError;
}

module.exports = { withRetry };
