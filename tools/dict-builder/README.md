# dict-builder

Host-side tool that turns a word-frequency corpus into compact, `include!`-able Rust
prediction data for the **Pocket LLM Terminal** firmware (RP2040 / Pico WH).

The firmware embeds the generated arrays in flash (2 MB budget) and uses them for
next-word prediction with minimal RAM:

- **`WORDS`** — top-N words sorted by descending frequency. The firmware does a linear
  **prefix scan** over this slice to offer completions for the word currently being typed.
- **`BIGRAMS`** — for the most common preceding words ("heads"), their most-likely next
  words ("followers"), pre-ranked and capped. The firmware looks up the last committed
  word as the head and offers the followers as next-word predictions.

## Usage

```sh
cd tools/dict-builder
cargo run -- <input.txt> <output_dir> [--max-words N]
```

- `<input.txt>` — corpus file (see format below).
- `<output_dir>` — directory to write `dict_data.rs` into (created if missing).
- `--max-words N` — cap on the number of words emitted into `WORDS` (default **4000**).

Example:

```sh
cargo run -- sample-words.txt .          # writes ./dict_data.rs
cargo run -- big-corpus.txt ../generated --max-words 6000
```

## Input format

One entry per line:

```
word<TAB>frequency      e.g.  the<TAB>229000
word                    (no tab => implicit descending frequency by line order)
```

Optional bigram lines (three TAB-separated fields):

```
word1<TAB>word2<TAB>frequency
```

A bigram line only contributes if `word1` is among the most-frequent words kept (the
head set). Rules:

- Words must be **lowercase ASCII** (`a`–`z`). Anything else is skipped.
- Blank lines and lines starting with `#` are ignored.
- Malformed lines (bad frequency, wrong field count, invalid characters) are skipped.
- Duplicate words keep the **highest** frequency seen.
- Ties in frequency break **alphabetically** for deterministic output.

See `sample-words.txt` for a working example.

## Output format (`dict_data.rs`)

```rust
pub static WORDS: &[&str] = &[ /* top-N words, descending frequency */ ];
pub static BIGRAMS: &[(&str, &[&str])] = &[
    ("the", &["people", "first", "water", "time"]),
    // head -> ranked, capped (<= 4) followers
];
```

Heads are listed in descending frequency order. An empty follower slice / empty
`BIGRAMS` simply means no bigram data was present in the corpus.

## Where the firmware includes it

The generated file is plain Rust with no dependencies, so the firmware just pulls it in:

```rust
// In a firmware crate, e.g. crates/<fw>/src/dict.rs:
include!(concat!(env!("OUT_DIR"), "/dict_data.rs")); // via build.rs copy, or
include!("../generated/dict_data.rs");               // a checked-in fixed path
```

## Roadmap

**v2** may replace the `WORDS` linear prefix-scan with a **packed trie** (DAWG / FST-style)
to cut flash size and make prefix lookups O(prefix length) instead of O(N). The CLI and
`BIGRAMS` contract are expected to stay; only the `WORDS` representation would change, so
keep firmware access behind a small `dict` module.
