//! Tal i celler och statistik över en markering ("1 234,5" och "1,234.5" förstås båda).

/// Tolkar ett tal med svenskt eller engelskt skrivsätt. Mellanslag (även hårda) är tusentalsavgränsare.
pub fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s.len() > 40 {
        return None;
    }
    let first_char = s.chars().next()?;
    let (neg, rest) = match first_char {
        '-' | '\u{2212}' => (true, &s[first_char.len_utf8()..]),
        '+' => (false, &s[1..]),
        _ => (false, s),
    };
    // Exponent: bara om det som följer är ett enkelt heltal.
    let (mant, exp) = match rest.find(['e', 'E']) {
        Some(i) => {
            let e = &rest[i + 1..];
            let digits = e.strip_prefix(['+', '-']).unwrap_or(e);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || i == 0 {
                return None;
            }
            (&rest[..i], Some(e))
        }
        None => (rest, None),
    };
    let first = mant.chars().next()?;
    if !(first.is_ascii_digit() || first == ',' || first == '.') {
        return None;
    }
    let is_space = |c: char| matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\u{2009}');
    let has_space = mant.chars().any(is_space);
    if !mant.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '.' || is_space(c)) {
        return None;
    }
    let commas = mant.matches(',').count();
    let dots = mant.matches('.').count();
    // Vilket tecken är decimaltecken (om något), och vilket är tusentalstecken?
    let (dec, thou): (Option<char>, Option<char>) = if commas > 0 && dots > 0 {
        let lc = mant.rfind(',')?;
        let ld = mant.rfind('.')?;
        if lc > ld { (Some(','), Some('.')) } else { (Some('.'), Some(',')) }
    } else if commas + dots == 0 {
        (None, None)
    } else {
        let c = if commas > 0 { ',' } else { '.' };
        if commas + dots == 1 { (Some(c), None) } else { (None, Some(c)) }
    };
    let (int_part, frac_part) = match dec {
        Some(d) => {
            let i = mant.rfind(d)?;
            (&mant[..i], &mant[i + 1..])
        }
        None => (mant, ""),
    };
    if frac_part.chars().any(|c| !c.is_ascii_digit()) {
        return None;
    }
    // Heltalsdelen: grupper om tre siffror när det finns tusentalstecken eller mellanslag.
    let sep = |c: char| is_space(c) || Some(c) == thou;
    let groups: Vec<&str> = int_part.split(sep).collect();
    if groups.len() > 1 {
        let ok = !groups[0].is_empty()
            && groups[0].len() <= 3
            && groups[1..].iter().all(|g| g.len() == 3)
            && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit()));
        if !ok {
            return None;
        }
    } else if has_space {
        return None;
    }
    let int_digits: String = groups.concat();
    if int_digits.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut t = String::with_capacity(int_digits.len() + frac_part.len() + 8);
    if neg {
        t.push('-');
    }
    t.push_str(if int_digits.is_empty() { "0" } else { &int_digits });
    if !frac_part.is_empty() {
        t.push('.');
        t.push_str(frac_part);
    }
    if let Some(e) = exp {
        t.push('e');
        t.push_str(e);
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Sammanfattning av markerade celler.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// Icke-tomma celler.
    pub count: usize,
    /// Celler som är tal.
    pub numeric: usize,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

impl Stats {
    pub fn average(&self) -> Option<f64> {
        (self.numeric > 0).then(|| self.sum / self.numeric as f64)
    }
}

/// Statistik över cellvärden.
pub fn compute<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> Stats {
    let mut st = Stats::default();
    for v in values {
        let v = v.as_ref();
        if v.trim().is_empty() {
            continue;
        }
        st.count += 1;
        if let Some(n) = parse_number(v) {
            if st.numeric == 0 {
                st.min = n;
                st.max = n;
            } else {
                st.min = st.min.min(n);
                st.max = st.max.max(n);
            }
            st.numeric += 1;
            st.sum += n;
        }
    }
    st
}

/// Skriver ett tal i språkets stil: tusentalsavgränsare och decimaltecken, högst sex decimaler.
pub fn format_number(v: f64) -> String {
    let en = crate::i18n::lang() == crate::i18n::Lang::En;
    let mut s = if v.fract() == 0.0 && v.abs() < 1e15 { format!("{v:.0}") } else { format!("{v:.6}") };
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b.to_owned()),
        None => (false, s),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i.to_owned(), Some(f.to_owned())),
        None => (body, None),
    };
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    let n = int.len();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(if en { ',' } else { '\u{a0}' });
        }
        out.push(ch);
    }
    if let Some(f) = frac {
        out.push(if en { '.' } else { ',' });
        out.push_str(&f);
    }
    out
}
