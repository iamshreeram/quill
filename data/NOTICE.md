# Notice: data/en_freq_10k.txt

- File: `en_freq_10k.txt`, 9894 lowercase words, one per line. Line order is the frequency rank.
- Origin: the "no-swears" variant of google-10000-english, https://github.com/first20hours/google-10000-english
- That list is derived from Peter Norvig's n-gram counts of the Google Web Trillion Word Corpus (LDC2006T13).
- Licence caveat: upstream describes the underlying data as covered by the LDC licence, which permits educational and personal/research use. That may not cover redistribution inside an MIT-licensed release binary. The maintainer should confirm this, or replace the file with a permissively licensed word list.
- Use in Quill: ranking and membership only, for offline spelling fixes (see `src/modules/grammar.rs`). No sentence text is stored.
