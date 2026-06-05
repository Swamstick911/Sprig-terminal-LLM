//! dict-builder
//!
//! Host-side tool that turns a word-frequency corpus into a compact, `include!`-able
//! Rust source file (`dict_data.rs`) for the Pocket LLM Terminal firmware (RP2040).
//!
//! The firmware embeds the generated arrays in flash (2 MB budget) and uses them for
//! next-word prediction with minimal RAM: `WORDS` is scanned by prefix to offer
//! completions for the current word, and `BIGRAMS` ranks the most-likely next word
//! given the previously committed word.
//!
//! Usage:
//!   dict-builder <input.txt> <output_dir> [--max-words N]
//!
//! Input format (one entry per line):
//!   word<TAB>frequency      e.g. `the\t229000`
//!   word                    (no tab => implicit descending frequency by line order)
//!
//! Words must be lowercase ASCII (a-z). Malformed lines are skipped.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process;

/// Default cap on the number of words emitted into `WORDS`.
const DEFAULT_MAX_WORDS: usize = 4000;

/// Number of leading (most-frequent) words for which we emit bigram follower lists.
const BIGRAM_HEAD_WORDS: usize = 256;

/// Max number of followers kept per bigram head word.
const MAX_FOLLOWERS: usize = 4;

/// A parsed corpus entry: a word and its frequency weight.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    word: String,
    freq: u64,
}

/// Parse the corpus text into ranked entries (descending frequency, then alphabetical
/// as a stable tie-breaker). Malformed lines are silently skipped.
///
/// A line is either `word<TAB>freq` or just `word`. When no frequency is given, an
/// implicit descending weight is assigned by line order, so earlier lines rank higher.
fn parse_corpus(text: &str) -> Vec<Entry> {
    let total_lines = text.lines().count() as u64;
    let mut implicit_rank: u64 = 0;
    let mut best: HashMap<String, u64> = HashMap::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (word_part, freq) = match line.split_once('\t') {
            Some((w, f)) => {
                let parsed: u64 = match f.trim().parse() {
                    Ok(v) => v,
                    Err(_) => continue, // malformed frequency => skip
                };
                (w.trim(), parsed)
            }
            None => {
                // No tab: implicit descending frequency by line order.
                implicit_rank += 1;
                (line, total_lines.saturating_sub(implicit_rank) + 1)
            }
        };

        if !is_valid_word(word_part) {
            continue;
        }

        // Keep the highest frequency if a word appears more than once.
        let e = best.entry(word_part.to_string()).or_insert(0);
        if freq > *e {
            *e = freq;
        }
    }

    let mut entries: Vec<Entry> = best
        .into_iter()
        .map(|(word, freq)| Entry { word, freq })
        .collect();

    rank(&mut entries);
    entries
}

/// True if `w` is a non-empty lowercase ASCII word (a-z only).
fn is_valid_word(w: &str) -> bool {
    !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase())
}

/// Sort entries by descending frequency, then ascending word for a deterministic order.
fn rank(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.freq.cmp(&a.freq).then_with(|| a.word.cmp(&b.word)));
}

/// Build bigram follower lists from `word1<TAB>word2<TAB>freq` lines, if present in the
/// input. This is optional: most simple corpora are unigram-only, so we also support a
/// purely synthetic fallback elsewhere. Returns a map head -> ranked followers.
///
/// (Kept for forward-compatibility; the current sample corpus is unigram-only and this
/// returns an empty map, which is handled gracefully by the emitter.)
fn parse_bigrams(text: &str, heads: &[String]) -> Vec<(String, Vec<String>)> {
    let head_set: std::collections::HashSet<&str> = heads.iter().map(|s| s.as_str()).collect();
    // map: head -> (follower -> freq)
    let mut map: HashMap<String, HashMap<String, u64>> = HashMap::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').map(|p| p.trim()).collect();
        if parts.len() != 3 {
            continue; // not a bigram line
        }
        let (w1, w2) = (parts[0], parts[1]);
        let freq: u64 = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !is_valid_word(w1) || !is_valid_word(w2) || !head_set.contains(w1) {
            continue;
        }
        *map.entry(w1.to_string())
            .or_default()
            .entry(w2.to_string())
            .or_insert(0) += freq;
    }

    let mut out: Vec<(String, Vec<String>)> = map
        .into_iter()
        .map(|(head, followers)| {
            let mut fv: Vec<(String, u64)> = followers.into_iter().collect();
            fv.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            fv.truncate(MAX_FOLLOWERS);
            let followers: Vec<String> = fv.into_iter().map(|(w, _)| w).collect();
            (head, followers)
        })
        .filter(|(_, fs)| !fs.is_empty())
        .collect();

    // Deterministic head order: follow the ranked `heads` order.
    let head_index: HashMap<&str, usize> =
        heads.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    out.sort_by_key(|(h, _)| *head_index.get(h.as_str()).unwrap_or(&usize::MAX));
    out
}

/// Escape a word for emission as a Rust string literal. Words are validated as ASCII
/// lowercase, so escaping is trivial, but we keep this defensive.
fn esc(w: &str) -> String {
    w.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render the `dict_data.rs` source from ranked words and bigram follower lists.
fn render(words: &[&str], bigrams: &[(String, Vec<String>)]) -> String {
    let mut s = String::new();

    s.push_str(
        "// @generated by dict-builder — DO NOT EDIT BY HAND.\n\
         //\n\
         // Prediction data for the Pocket LLM Terminal firmware (RP2040).\n\
         // Embedded in flash; `include!`-ed by the firmware, e.g.:\n\
         //     mod dict { include!(concat!(env!(\"OUT_DIR\"), \"/dict_data.rs\")); }\n\
         // or with a fixed path:\n\
         //     include!(\"../generated/dict_data.rs\");\n\
         //\n\
         // Format:\n\
         //   pub static WORDS: &[&str]\n\
         //     Top-N words, sorted by DESCENDING frequency (ties broken alphabetically).\n\
         //     Firmware does a prefix scan over this slice to offer word completions.\n\
         //\n\
         //   pub static BIGRAMS: &[(&str, &[&str])]\n\
         //     For common preceding words (the head), their most-likely next words\n\
         //     (the followers), already ranked best-first and capped per head.\n\
         //     Firmware looks up the last committed word as the head and offers the\n\
         //     followers as next-word predictions. Heads are listed in descending\n\
         //     frequency order; an empty slice means no bigram data was available.\n\
         //\n",
    );
    s.push_str(&format!(
        "// Counts: {} words, {} bigram heads.\n\n",
        words.len(),
        bigrams.len()
    ));

    // WORDS
    s.push_str("pub static WORDS: &[&str] = &[\n");
    for chunk in words.chunks(8) {
        s.push_str("    ");
        for w in chunk {
            s.push_str(&format!("\"{}\", ", esc(w)));
        }
        s.push('\n');
    }
    s.push_str("];\n\n");

    // BIGRAMS
    s.push_str("pub static BIGRAMS: &[(&str, &[&str])] = &[\n");
    for (head, followers) in bigrams {
        let fs: Vec<String> = followers.iter().map(|f| format!("\"{}\"", esc(f))).collect();
        s.push_str(&format!(
            "    (\"{}\", &[{}]),\n",
            esc(head),
            fs.join(", ")
        ));
    }
    s.push_str("];\n");

    s
}

struct Args {
    input: String,
    output_dir: String,
    max_words: usize,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut max_words = DEFAULT_MAX_WORDS;

    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--max-words" => {
                i += 1;
                let v = argv
                    .get(i)
                    .ok_or_else(|| "--max-words requires a value".to_string())?;
                max_words = v
                    .parse()
                    .map_err(|_| format!("invalid --max-words value: {v}"))?;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag: {other}"));
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    if positional.len() != 2 {
        return Err("expected <input.txt> <output_dir>".to_string());
    }
    if max_words == 0 {
        return Err("--max-words must be > 0".to_string());
    }

    Ok(Args {
        input: positional[0].clone(),
        output_dir: positional[1].clone(),
        max_words,
    })
}

fn usage() -> &'static str {
    "usage: dict-builder <input.txt> <output_dir> [--max-words N]\n\
     \n\
     Input lines: `word<TAB>frequency` or just `word` (implicit rank by line order).\n\
     Optional bigram lines: `word1<TAB>word2<TAB>frequency`.\n\
     Writes `dict_data.rs` into <output_dir>."
}

fn main() {
    let argv: Vec<String> = env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", usage());
            process::exit(2);
        }
    };

    let text = match fs::read_to_string(&args.input) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read input '{}': {e}", args.input);
            process::exit(1);
        }
    };

    let mut entries = parse_corpus(&text);
    if entries.is_empty() {
        eprintln!("error: no valid words parsed from '{}'", args.input);
        process::exit(1);
    }
    entries.truncate(args.max_words);

    let words: Vec<&str> = entries.iter().map(|e| e.word.as_str()).collect();

    // Bigram heads are the most-frequent words we kept.
    let heads: Vec<String> = entries
        .iter()
        .take(BIGRAM_HEAD_WORDS.min(words.len()))
        .map(|e| e.word.clone())
        .collect();
    let bigrams = parse_bigrams(&text, &heads);

    let out = render(&words, &bigrams);

    let out_dir = Path::new(&args.output_dir);
    if let Err(e) = fs::create_dir_all(out_dir) {
        eprintln!("error: cannot create output dir '{}': {e}", args.output_dir);
        process::exit(1);
    }
    let out_path = out_dir.join("dict_data.rs");
    if let Err(e) = fs::write(&out_path, out) {
        eprintln!("error: cannot write '{}': {e}", out_path.display());
        process::exit(1);
    }

    println!(
        "wrote {} ({} words, {} bigram heads)",
        out_path.display(),
        words.len(),
        bigrams.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tab_separated_freq() {
        let txt = "the\t100\nand\t50\nof\t75\n";
        let e = parse_corpus(txt);
        // Sorted by descending freq: the(100), of(75), and(50)
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].word, "the");
        assert_eq!(e[0].freq, 100);
        assert_eq!(e[1].word, "of");
        assert_eq!(e[2].word, "and");
    }

    #[test]
    fn implicit_rank_by_line_order() {
        let txt = "first\nsecond\nthird\n";
        let e = parse_corpus(txt);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].word, "first");
        assert_eq!(e[1].word, "second");
        assert_eq!(e[2].word, "third");
        // Strictly descending implicit weights.
        assert!(e[0].freq > e[1].freq && e[1].freq > e[2].freq);
    }

    #[test]
    fn skips_malformed_lines() {
        // bad freq, empty, comment, uppercase, digits, punctuation
        let txt = "good\t10\nbad\tNOTANUM\n\n# comment\nUPPER\t5\nwith2\t3\nhy-phen\t4\nok\t7\n";
        let e = parse_corpus(txt);
        let words: Vec<&str> = e.iter().map(|x| x.word.as_str()).collect();
        assert_eq!(words, vec!["good", "ok"]);
    }

    #[test]
    fn dedupes_keeping_max_freq() {
        let txt = "the\t10\nthe\t999\nthe\t5\n";
        let e = parse_corpus(txt);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].freq, 999);
    }

    #[test]
    fn tie_break_is_alphabetical() {
        let txt = "beta\t10\nalpha\t10\ngamma\t10\n";
        let e = parse_corpus(txt);
        let words: Vec<&str> = e.iter().map(|x| x.word.as_str()).collect();
        assert_eq!(words, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn is_valid_word_rules() {
        assert!(is_valid_word("hello"));
        assert!(!is_valid_word(""));
        assert!(!is_valid_word("Hello"));
        assert!(!is_valid_word("a1"));
        assert!(!is_valid_word("a-b"));
        assert!(!is_valid_word("café"));
    }

    #[test]
    fn bigrams_rank_and_cap_followers() {
        let heads = vec!["the".to_string()];
        // five distinct followers with differing weights; cap is MAX_FOLLOWERS (4)
        let txt = "\
the\tquick\t5
the\tlazy\t1
the\tbrown\t4
the\tfox\t3
the\tdog\t2
of\tthe\t9
";
        let bg = parse_bigrams(txt, &heads);
        assert_eq!(bg.len(), 1);
        let (head, followers) = &bg[0];
        assert_eq!(head, "the");
        assert_eq!(followers.len(), MAX_FOLLOWERS);
        // Best-first by weight: quick(5), brown(4), fox(3), dog(2); lazy(1) dropped.
        assert_eq!(followers, &vec!["quick", "brown", "fox", "dog"]);
    }

    #[test]
    fn bigrams_ignore_non_head_and_invalid() {
        let heads = vec!["the".to_string()];
        let txt = "go\tnow\t5\nthe\tBAD\t5\nthe\tword\t5\n";
        let bg = parse_bigrams(txt, &heads);
        assert_eq!(bg.len(), 1);
        assert_eq!(bg[0].1, vec!["word"]);
    }

    #[test]
    fn render_is_includable_shape() {
        let words = vec!["the", "and"];
        let bigrams = vec![("the".to_string(), vec!["quick".to_string()])];
        let out = render(&words, &bigrams);
        assert!(out.contains("pub static WORDS: &[&str] = &["));
        assert!(out.contains("\"the\""));
        assert!(out.contains("pub static BIGRAMS: &[(&str, &[&str])] = &["));
        assert!(out.contains("(\"the\", &[\"quick\"]),"));
    }

    #[test]
    fn arg_parsing() {
        let a = parse_args(&[
            "in.txt".into(),
            "out".into(),
            "--max-words".into(),
            "10".into(),
        ])
        .unwrap();
        assert_eq!(a.input, "in.txt");
        assert_eq!(a.output_dir, "out");
        assert_eq!(a.max_words, 10);

        let d = parse_args(&["in.txt".into(), "out".into()]).unwrap();
        assert_eq!(d.max_words, DEFAULT_MAX_WORDS);

        assert!(parse_args(&["only-one".into()]).is_err());
        assert!(parse_args(&["a".into(), "b".into(), "--nope".into()]).is_err());
        assert!(parse_args(&[
            "a".into(),
            "b".into(),
            "--max-words".into(),
            "0".into()
        ])
        .is_err());
    }
}
