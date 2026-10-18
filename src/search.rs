//! Búsqueda de texto y detección de URLs sobre líneas de terminal, ya convertidas a texto.

/// Rangos `[inicio, fin)` en índices de carácter donde aparece `query`.
/// Distingue mayúsculas solo si la búsqueda contiene alguna ("smart case").
pub fn find(text: &str, query: &str) -> Vec<(usize, usize)> {
    let query: Vec<char> = query.chars().collect();
    if query.is_empty() {
        return Vec::new();
    }
    let case_sensitive = query.iter().any(|c| c.is_uppercase());
    let fold = |c: char| if case_sensitive { c } else { c.to_lowercase().next().unwrap_or(c) };
    let query: Vec<char> = query.into_iter().map(fold).collect();
    let text: Vec<char> = text.chars().map(fold).collect();

    let mut found = Vec::new();
    let mut start = 0;
    while start + query.len() <= text.len() {
        if text[start..start + query.len()] == query[..] {
            found.push((start, start + query.len()));
            start += query.len();
        } else {
            start += 1;
        }
    }
    found
}

/// URLs `http(s)://` de una línea: rango en caracteres y texto, sin la puntuación final.
pub fn find_urls(text: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut urls = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..chars.len().min(i + 8)].iter().collect();
        let prefix = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            i += 1;
            continue;
        };
        let mut end = i + prefix;
        while end < chars.len() && !chars[end].is_whitespace() && !matches!(chars[end], '<' | '>' | '"' | '`' | '\'') {
            end += 1;
        }
        while end > i + prefix {
            let last = chars[end - 1];
            let slice: String = chars[i..end].iter().collect();
            let unbalanced = |open: char, close: char| {
                slice.matches(close).count() > slice.matches(open).count()
            };
            let trim = matches!(last, '.' | ',' | ';' | ':' | '!' | '?')
                || (last == ')' && unbalanced('(', ')'))
                || (last == ']' && unbalanced('[', ']'))
                || (last == '}' && unbalanced('{', '}'));
            if trim {
                end -= 1;
            } else {
                break;
            }
        }
        if end > i + prefix {
            urls.push((i, end, chars[i..end].iter().collect()));
        }
        i = end.max(i + 1);
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_all_non_overlapping_matches() {
        assert_eq!(find("abcabcabc", "abc"), vec![(0, 3), (3, 6), (6, 9)]);
        assert_eq!(find("aaaa", "aa"), vec![(0, 2), (2, 4)]);
        assert!(find("hola", "").is_empty());
        assert!(find("hola", "mundo").is_empty());
    }

    #[test]
    fn smart_case_ignores_case_only_for_lowercase_queries() {
        assert_eq!(find("Error error ERROR", "error").len(), 3);
        assert_eq!(find("Error error ERROR", "Error").len(), 1);
        assert_eq!(find("Error error ERROR", "ERROR").len(), 1);
    }

    #[test]
    fn indices_are_in_characters_not_bytes() {
        assert_eq!(find("¡niño! niño", "niño"), vec![(1, 5), (7, 11)]);
    }

    #[test]
    fn extracts_urls_without_trailing_punctuation() {
        let urls = find_urls("ver https://example.com/a?b=1, y (http://x.org/p). Fin");
        let texts: Vec<_> = urls.iter().map(|u| u.2.as_str()).collect();
        assert_eq!(texts, ["https://example.com/a?b=1", "http://x.org/p"]);
        assert_eq!(urls[0].0, 4);
    }

    #[test]
    fn keeps_balanced_parentheses_inside_urls() {
        let urls = find_urls("https://es.wikipedia.org/wiki/Rust_(lenguaje) y [https://a.io/x]");
        let texts: Vec<_> = urls.iter().map(|u| u.2.as_str()).collect();
        assert_eq!(texts, ["https://es.wikipedia.org/wiki/Rust_(lenguaje)", "https://a.io/x"]);
    }

    #[test]
    fn ignores_bare_schemes_and_other_protocols() {
        assert!(find_urls("http:// https:// ftp://x.org").is_empty());
    }
}
