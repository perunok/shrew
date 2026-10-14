//! Sublime/TextMate scope selectors, enough to read a color scheme: comma-separated alternatives of space-separated
//! scope prefixes, with `- exclusions`. A match has a score; the more specific selector (longer prefixes, deeper in
//! the stack) wins, as in Sublime.

/// One alternative: the path, then what must not match.
struct Alt<'a> {
    path: Vec<&'a str>,
    exclude: Vec<Vec<&'a str>>,
}

fn parse(sel: &str) -> Vec<Alt<'_>> {
    sel.split([',', '|'])
        .filter_map(|alt| {
            let mut parts = alt.split(" -").map(|p| p.split([' ', '(', ')']).filter(|s| !s.is_empty()).collect());
            let path: Vec<&str> = parts.next()?;
            (!path.is_empty()).then(|| Alt { path, exclude: parts.collect() })
        })
        .collect()
}

/// How many dot-separated atoms of `prefix` match the start of `scope` (0 when it is not a prefix).
fn prefix_atoms(prefix: &str, scope: &str) -> usize {
    let ok = scope == prefix || (scope.starts_with(prefix) && scope.as_bytes().get(prefix.len()) == Some(&b'.'));
    if ok { prefix.split('.').count() } else { 0 }
}

/// Score of `path` against `stack` (outermost first), matching from the innermost scope outwards. The last
/// element must match some scope; earlier elements must match scopes further out, in order.
fn path_score(path: &[&str], stack: &[&str]) -> Option<u64> {
    let mut score = 0u64;
    let mut top = stack.len();
    for el in path.iter().rev() {
        let found = (0..top).rev().find(|&i| prefix_atoms(el, stack[i]) > 0)?;
        score += (prefix_atoms(el, stack[found]) as u64) << (4 * (found as u64 + 1)).min(60);
        top = found;
    }
    Some(score)
}

/// The best score of `selector` against `stack`, `None` when it does not match.
pub fn score(selector: &str, stack: &[&str]) -> Option<u64> {
    parse(selector)
        .iter()
        .filter(|a| a.exclude.iter().all(|ex| path_score(ex, stack).is_none()))
        .filter_map(|a| path_score(&a.path, stack))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specificity() {
        let stack = ["source.rust", "keyword.operator.word.rust"];
        let op = score("keyword.operator", &stack).unwrap();
        let word = score("keyword.operator.word", &stack).unwrap();
        let kw = score("keyword", &stack).unwrap();
        assert!(word > op && op > kw);
        assert!(score("string", &stack).is_none());
        assert!(score("source.rust keyword", &stack).unwrap() > kw);
        assert!(score("keyword - source.rust", &stack).is_none());
        assert!(score("string, keyword", &stack).is_some());
        assert!(score("keywordx", &stack).is_none());
        // deeper scopes outrank shallower ones
        assert!(score("keyword", &stack).unwrap() > score("source", &stack).unwrap());
    }
}
