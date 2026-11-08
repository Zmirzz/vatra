# wordcount

Write `wordcount.js` as a CommonJS module that exports two functions.

Words are separated by whitespace. Empty strings are not words.

- `countWords(text)` returns the number of words in `text`. For example, `countWords('')` is 0 and `countWords('  a  b ')` is 2.
- `topWords(text, n)` returns the `n` most frequent words as an array of `[word, count]` pairs, most frequent first. Compare words in lowercase, so `The` and `the` are the same word. Ties are ordered alphabetically. For example, `topWords('B a b A c', 3)` is `[['a', 2], ['b', 2], ['c', 1]]`.
