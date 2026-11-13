//! Typo-tolerant matching, used when the full-text search (exact words and prefixes) finds almost nothing:
//! "beatels" finds *The Beatles*, "quen" finds *Queen*.
//!
//! Every word of the query must be close to a word of the candidate: an edit distance (insertion, deletion,
//! substitution, swap of two letters) of at most 1 for words of 4 to 6 letters and 2 from 7 letters; short words
//! must be exact. The last word of the query may be unfinished: it is also compared with the start of each word.

/// Lowercase, accents removed (the common Latin ones), letters and digits only, split into words.
pub fn words(s: &str) -> Vec<Vec<char>> {
    let mut out: Vec<Vec<char>> = vec![];
    let mut cur: Vec<char> = vec![];
    for c in s.chars().flat_map(|c| c.to_lowercase()) {
        let c = fold(c);
        if c.is_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        c => c,
    }
}

/// Edit distance with adjacent swaps (optimal string alignment), `None` as soon as it exceeds `max`.
pub fn distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let (n, m) = (a.len(), b.len());
    let mut prev2: Vec<usize> = vec![0; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(prev2[j - 2] + 1);
            }
            cur[j] = d;
            row_min = row_min.min(d);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[m] <= max).then_some(prev[m])
}

fn allowed(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=6 => 1,
        _ => 2,
    }
}

/// Total distance when every query word matches some candidate word, `None` otherwise (0 = exact).
pub fn score(query: &[Vec<char>], candidate: &[Vec<char>]) -> Option<usize> {
    // "acdc" must find "AC/DC": the words of the candidate also count glued together.
    let glued: Vec<char> = candidate.iter().flatten().copied().collect();
    let with_glued;
    let candidate = if candidate.len() > 1 {
        with_glued = candidate.iter().cloned().chain(std::iter::once(glued)).collect::<Vec<_>>();
        &with_glued[..]
    } else {
        candidate
    };
    let mut total = 0;
    for (i, q) in query.iter().enumerate() {
        let last = i + 1 == query.len();
        let max = allowed(q.len());
        let best = candidate
            .iter()
            .filter_map(|w| {
                let full = distance(q, w, max);
                // Unfinished last word: compare with the start of the candidate word.
                let prefix = if last && w.len() > q.len() { distance(q, &w[..q.len()], max) } else { None };
                match (full, prefix) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            })
            .min()?;
        total += best;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<Vec<char>> {
        words(s)
    }

    #[test]
    fn distances() {
        let d = |a: &str, b: &str, m| distance(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>(), m);
        assert_eq!(d("beatles", "beatles", 2), Some(0));
        assert_eq!(d("beatels", "beatles", 2), Some(1), "a swap counts once");
        assert_eq!(d("queeen", "queen", 2), Some(1));
        assert_eq!(d("quen", "queen", 2), Some(1));
        assert_eq!(d("radiohead", "raidohead", 2), Some(1));
        assert_eq!(d("abc", "xyz", 1), None);
        assert_eq!(d("kitten", "sitting", 2), None, "3 edits");
    }

    #[test]
    fn typos_match_names() {
        assert_eq!(score(&w("beatels"), &w("The Beatles")), Some(1));
        assert_eq!(score(&w("quen"), &w("Queen")), Some(1));
        assert_eq!(score(&w("beatels help"), &w("The Beatles - Help!")), Some(1));
        assert_eq!(score(&w("eric clapton"), &w("Éric Clapton")), Some(0), "accents folded");
        assert!(score(&w("pink floid"), &w("Pink Floyd")).is_some());
    }

    #[test]
    fn words_glued_together_match() {
        assert_eq!(score(&w("acdc"), &w("AC/DC")), Some(0));
        assert_eq!(score(&w("pinkfloyd"), &w("Pink Floyd")), Some(0));
        assert!(score(&w("pinkfloid"), &w("Pink Floyd")).is_some());
        assert_eq!(score(&w("acdx"), &w("AC/DC")), Some(1), "one typo in the glued name");
        assert_eq!(score(&w("abxx"), &w("AC/DC")), None);
    }

    #[test]
    fn short_words_must_be_exact_and_every_word_must_match() {
        assert_eq!(score(&w("abxx"), &w("AC DC")), None);
        assert_eq!(score(&w("abc"), &w("abd")), None, "3 letters: no typo allowed");
        assert_eq!(score(&w("beatles zzzzzz"), &w("The Beatles")), None);
        assert_eq!(score(&w("rolling"), &w("Queen")), None);
    }

    #[test]
    fn the_last_word_may_be_unfinished() {
        assert_eq!(score(&w("beatles hlp"), &w("The Beatles - Help!")), None, "3 letters stay exact...");
        assert!(score(&w("beatles helo"), &w("The Beatles - Help!")).is_some(), "...but a 4-letter start with a typo is fine");
        assert_eq!(score(&w("bea"), &w("The Beatles")), Some(0), "plain prefix");
    }
}
