//! Sheet identification and text search: sheet numbers/titles from
//! bookmarks, page labels or title-block text; references to other sheets
//! (for clickable links); and full-text search. Pure logic, testable.

/// A word on a page: text, box in page DIPs [l, t, r, b], font size (DIP).
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: [f32; 4],
    pub size: f32,
}

/// What a sheet is called.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetName {
    pub number: Option<String>,
    pub title: Option<String>,
}

impl SheetName {
    /// e.g. "A-101  Floor Plan" or "A-101" or "".
    pub fn display(&self) -> String {
        match (&self.number, &self.title) {
            (Some(n), Some(t)) => format!("{n}  {t}"),
            (Some(n), None) => n.clone(),
            (None, Some(t)) => t.clone(),
            (None, None) => String::new(),
        }
    }
}

/// Does this token look like a drawing sheet number? A101, A-101, S2.01,
/// M-201A, E0.1, FP-1, G001 (1-3 letters, then digits with optional
/// separators, optional trailing letter).
pub fn is_sheet_number(s: &str) -> bool {
    let s = s.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']'));
    let b = s.as_bytes();
    let letters = b.iter().take_while(|c| c.is_ascii_uppercase()).count();
    if !(1..=3).contains(&letters) || b.len() > 10 {
        return false;
    }
    let mut rest = &s[letters..];
    if let Some(r) = rest.strip_prefix(['-', '.', ' ']) {
        rest = r;
    }
    let rb = rest.as_bytes();
    let first_digits = rb.iter().take_while(|c| c.is_ascii_digit()).count();
    if first_digits == 0 || first_digits > 4 {
        return false;
    }
    let mut tail = &rest[first_digits..];
    if let Some(t) = tail.strip_prefix('.') {
        let d = t.bytes().take_while(|c| c.is_ascii_digit()).count();
        if d == 0 {
            return false;
        }
        tail = &t[d..];
    }
    tail.is_empty() || (tail.len() == 1 && tail.as_bytes()[0].is_ascii_uppercase())
}

/// Canonical form for matching: "A-101", "A101", "a 101" -> "A101".
pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Split a bookmark / label like "A101 - FLOOR PLAN" into number and title.
pub fn parse_label(label: &str) -> SheetName {
    let label = label.trim();
    if label.is_empty() {
        return SheetName::default();
    }
    // First token (up to a space or " - ").
    let (first, rest) = match label.find(char::is_whitespace) {
        Some(i) => (&label[..i], label[i..].trim()),
        None => (label, ""),
    };
    if is_sheet_number(first) {
        let title = rest.trim_start_matches(['-', '\u{2013}', '\u{2014}', ':', '|']).trim();
        SheetName { number: Some(first.to_string()), title: (!title.is_empty()).then(|| title_case(title)) }
    } else {
        SheetName { number: None, title: Some(title_case(label)) }
    }
}

/// "FIRST FLOOR PLAN" -> "First Floor Plan" (leaves mixed case alone).
fn title_case(s: &str) -> String {
    if s.chars().any(|c| c.is_lowercase()) {
        return s.to_string();
    }
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_string() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Guess the sheet number from title-block text: the largest sheet-number
/// token in the right-hand 35% or bottom 25% of the sheet.
pub fn guess_number(words: &[Word], page: (f32, f32)) -> Option<String> {
    words
        .iter()
        .filter(|w| is_sheet_number(&w.text))
        .filter(|w| w.rect[0] > page.0 * 0.65 || w.rect[1] > page.1 * 0.75)
        .max_by(|a, b| a.size.total_cmp(&b.size))
        .map(|w| w.text.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '.').to_string())
}

/// Words on a page that name another sheet of the set: (rect, target page).
pub fn find_links(words: &[Word], own_page: u32, number_to_page: &std::collections::HashMap<String, u32>) -> Vec<([f32; 4], u32)> {
    words
        .iter()
        .filter(|w| is_sheet_number(&w.text))
        .filter_map(|w| number_to_page.get(&normalize(&w.text)).map(|p| (w.rect, *p)))
        .filter(|(_, p)| *p != own_page)
        .collect()
}

/// A search hit: page, word boxes, and a snippet of surrounding text.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub page: u32,
    pub rects: Vec<[f32; 4]>,
    pub snippet: String,
}

/// Case-insensitive search for `query` (words or a phrase) on one page.
pub fn search_page(page: u32, words: &[Word], query: &str) -> Vec<Hit> {
    let q: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let lower: Vec<String> = words.iter().map(|w| w.text.to_lowercase()).collect();
    let mut hits = Vec::new();
    let n = q.len();
    for i in 0..words.len() {
        if i + n > words.len() {
            break;
        }
        let ok = (0..n).all(|k| {
            let w = &lower[i + k];
            if n == 1 {
                w.contains(&q[0])
            } else if k == 0 {
                w.ends_with(&q[0])
            } else if k == n - 1 {
                w.starts_with(&q[k])
            } else {
                *w == q[k]
            }
        });
        if ok {
            let lo = i.saturating_sub(4);
            let hi = (i + n + 4).min(words.len());
            let snippet = words[lo..hi].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            hits.push(Hit { page, rects: words[i..i + n].iter().map(|w| w.rect).collect(), snippet });
        }
    }
    hits
}

/// Discipline letter of a sheet number (US National CAD Standard): the
/// first letter, so "FP-101" is F (fire protection), "AD-101" is A.
pub fn discipline(number: &str) -> Option<char> {
    number.trim().chars().next().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase())
}

/// Name of a discipline letter.
pub fn discipline_name(c: char) -> &'static str {
    match c {
        'G' => "General",
        'H' => "Hazardous materials",
        'V' => "Survey",
        'B' => "Geotechnical",
        'W' => "Civil works",
        'C' => "Civil",
        'L' => "Landscape",
        'S' => "Structural",
        'A' => "Architectural",
        'I' => "Interiors",
        'Q' => "Equipment",
        'F' => "Fire protection",
        'P' => "Plumbing",
        'D' => "Process",
        'M' => "Mechanical",
        'E' => "Electrical",
        'T' => "Telecom",
        'R' => "Resource",
        'X' => "Other disciplines",
        'Z' => "Contractor / shop",
        'O' => "Operations",
        'K' => "Food service",
        _ => "Other",
    }
}

/// Disciplines of a set in order of first appearance:
/// (letter, first sheet, sheet count). Sheets without a number are skipped.
pub fn discipline_groups(numbers: &[Option<&str>]) -> Vec<(char, u32, usize)> {
    let mut out: Vec<(char, u32, usize)> = Vec::new();
    for (i, n) in numbers.iter().enumerate() {
        let Some(d) = n.and_then(discipline) else { continue };
        match out.iter_mut().find(|g| g.0 == d) {
            Some(g) => g.2 += 1,
            None => out.push((d, i as u32, 1)),
        }
    }
    out
}

/// Parse "1, 3-5, 8" (1-based, as typed) into sorted 0-based sheets.
pub fn parse_range(s: &str, count: u32) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for part in s.split([',', ';', ' ']).filter(|p| !p.trim().is_empty()) {
        let (a, b) = match part.split_once(['-', '\u{2013}']) {
            Some((a, b)) => (a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?),
            None => {
                let v = part.trim().parse::<u32>().ok()?;
                (v, v)
            }
        };
        if a == 0 || b == 0 || a > count || b > count || a > b {
            return None;
        }
        out.extend(a - 1..b);
    }
    out.sort_unstable();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

/// Sort key so "A-2" comes before "A-10" (natural order).
pub fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut key = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n = String::new();
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                n.push(d);
                chars.next();
            }
            key.push((0, n.parse().unwrap_or(u64::MAX), String::new()));
        } else {
            let mut t = String::new();
            while let Some(&d) = chars.peek().filter(|d| !d.is_ascii_digit()) {
                t.push(d.to_ascii_lowercase());
                chars.next();
            }
            key.push((1, 0, t));
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disciplines() {
        assert_eq!(discipline("FP-101"), Some('F'));
        assert_eq!(discipline("a-101"), Some('A'));
        assert_eq!(discipline("101"), None);
        let nums = [Some("G-001"), Some("A-101"), Some("A-102"), None, Some("S-201"), Some("A-501")];
        assert_eq!(discipline_groups(&nums), vec![('G', 0, 1), ('A', 1, 3), ('S', 4, 1)]);
        assert_eq!(discipline_name('M'), "Mechanical");
    }

    #[test]
    fn ranges_and_order() {
        assert_eq!(parse_range("1, 3-5, 8", 10), Some(vec![0, 2, 3, 4, 7]));
        assert_eq!(parse_range("2-2", 3), Some(vec![1]));
        assert_eq!(parse_range("0", 3), None);
        assert_eq!(parse_range("4", 3), None);
        assert_eq!(parse_range("x", 3), None);
        let mut names = vec!["A-10.pdf", "A-2.pdf", "a-1.pdf", "S-1.pdf"];
        names.sort_by_key(|n| natural_key(n));
        assert_eq!(names, vec!["a-1.pdf", "A-2.pdf", "A-10.pdf", "S-1.pdf"]);
    }

    fn w(text: &str, x: f32, y: f32, size: f32) -> Word {
        Word { text: text.into(), rect: [x, y, x + size * text.len() as f32 * 0.6, y + size], size }
    }

    #[test]
    fn sheet_numbers() {
        for ok in ["A101", "A-101", "S2.01", "M-201A", "E0.1", "FP-1", "G001", "A1.1", "(A-501)"] {
            assert!(is_sheet_number(ok), "{ok}");
        }
        for bad in ["101", "ABCD-1", "A-", "PLAN", "A-101-B", "a101", "12'-6\"", "4X", "A.B"] {
            assert!(!is_sheet_number(bad), "{bad}");
        }
        assert_eq!(normalize("a-101"), "A101");
        assert_eq!(normalize("S2.01"), "S201");
    }

    #[test]
    fn labels() {
        assert_eq!(parse_label("A101 - FLOOR PLAN"), SheetName { number: Some("A101".into()), title: Some("Floor Plan".into()) });
        assert_eq!(parse_label("S-201 Foundation Plan"), SheetName { number: Some("S-201".into()), title: Some("Foundation Plan".into()) });
        assert_eq!(parse_label("COVER SHEET"), SheetName { number: None, title: Some("Cover Sheet".into()) });
        assert_eq!(parse_label("A-501").display(), "A-501");
    }

    #[test]
    fn title_block_and_links() {
        let page = (3456.0, 2304.0);
        let words = vec![
            w("SEE", 500.0, 500.0, 10.0),
            w("A-501", 540.0, 500.0, 10.0),   // reference in the drawing
            w("A-101", 3200.0, 2200.0, 36.0), // big title-block number
            w("A-102", 3200.0, 2100.0, 9.0),  // small revision table text
        ];
        assert_eq!(guess_number(&words, page).as_deref(), Some("A-101"));
        let map = [("A101".to_string(), 0u32), ("A501".to_string(), 4u32)].into_iter().collect();
        let links = find_links(&words, 0, &map);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1, 4);
    }

    #[test]
    fn search() {
        let words = vec![w("VERIFY", 0.0, 0.0, 10.0), w("IN", 70.0, 0.0, 10.0), w("FIELD.", 90.0, 0.0, 10.0), w("Field", 0.0, 50.0, 10.0)];
        let h = search_page(3, &words, "in field");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].rects.len(), 2);
        assert_eq!(search_page(3, &words, "field").len(), 2);
        assert!(search_page(3, &words, "").is_empty());
    }
}
