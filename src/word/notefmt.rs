//! Fot- och slutnoternas talformat (`w:footnotePr`/`w:endnotePr` i .docx, `text:notes-configuration` i
//! .odt). Etiketterna i texten är stabila (`[^1]`, `[^ei]` …, se `markup`); formatet ligger i grundformatsraden
//! (`endnote-format: decimal; footnote-format: lowerRoman; endnote-start: 3`) och styr bara hur numren visas
//! i dokumentläget, i PDF-exporten och i den sparade filen.

use super::markup;

/// Ett talformat för noternas nummer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteFmt {
    /// 1, 2, 3
    Decimal,
    /// i, ii, iii
    LowerRoman,
    /// I, II, III
    UpperRoman,
    /// a, b, c … z, aa, bb
    LowerLetter,
    /// A, B, C …
    UpperLetter,
    /// *, †, ‡, §, **, †† …
    Chicago,
}

impl NoteFmt {
    /// Alla format i den ordning menyn visar dem.
    pub const ALL: [NoteFmt; 6] =
        [NoteFmt::Decimal, NoteFmt::LowerRoman, NoteFmt::UpperRoman, NoteFmt::LowerLetter, NoteFmt::UpperLetter, NoteFmt::Chicago];

    /// Namnet på raden och i `w:numFmt`.
    pub fn name(self) -> &'static str {
        match self {
            NoteFmt::Decimal => "decimal",
            NoteFmt::LowerRoman => "lowerRoman",
            NoteFmt::UpperRoman => "upperRoman",
            NoteFmt::LowerLetter => "lowerLetter",
            NoteFmt::UpperLetter => "upperLetter",
            NoteFmt::Chicago => "chicago",
        }
    }

    pub fn from_name(s: &str) -> Option<NoteFmt> {
        let s = s.trim();
        NoteFmt::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(s))
    }

    /// `style:num-format` i ODF (`chicago` finns inte där och blir en stjärnlista via `*`).
    pub fn odf(self) -> &'static str {
        match self {
            NoteFmt::Decimal => "1",
            NoteFmt::LowerRoman => "i",
            NoteFmt::UpperRoman => "I",
            NoteFmt::LowerLetter => "a",
            NoteFmt::UpperLetter => "A",
            NoteFmt::Chicago => "*",
        }
    }

    pub fn from_odf(s: &str) -> Option<NoteFmt> {
        Some(match s.trim() {
            "1" => NoteFmt::Decimal,
            "i" => NoteFmt::LowerRoman,
            "I" => NoteFmt::UpperRoman,
            "a" => NoteFmt::LowerLetter,
            "A" => NoteFmt::UpperLetter,
            "*" => NoteFmt::Chicago,
            _ => return None,
        })
    }

    /// Nummer `n` (1-baserat) i formatet. Noll skrivs som siffra.
    pub fn format(self, n: usize) -> String {
        if n == 0 {
            return "0".to_owned();
        }
        match self {
            NoteFmt::Decimal => n.to_string(),
            NoteFmt::LowerRoman => markup::roman(n),
            NoteFmt::UpperRoman => markup::roman(n).to_uppercase(),
            NoteFmt::LowerLetter => letters(n, false),
            NoteFmt::UpperLetter => letters(n, true),
            NoteFmt::Chicago => {
                const SYM: [&str; 4] = ["*", "†", "‡", "§"];
                SYM[(n - 1) % 4].repeat((n - 1) / 4 + 1)
            }
        }
    }

    /// Exempel för menyn: de tre första numren.
    pub fn sample(self) -> String {
        format!("{}, {}, {}", self.format(1), self.format(2), self.format(3))
    }
}

/// a … z, aa … zz, aaa … (Words `lowerLetter`: bokstaven upprepas).
fn letters(n: usize, upper: bool) -> String {
    let c = (b'a' + ((n - 1) % 26) as u8) as char;
    let c = if upper { c.to_ascii_uppercase() } else { c };
    c.to_string().repeat((n - 1) / 26 + 1)
}

/// Formatet och första numret för en sorts noter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteStyle {
    pub fmt: NoteFmt,
    pub start: u32,
}

/// Dokumentets talformat för fotnoter och slutnoter. Standard: fotnoter 1, 2, 3 och slutnoter i, ii, iii.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteFormats {
    pub foot: NoteStyle,
    pub end: NoteStyle,
}

impl Default for NoteFormats {
    fn default() -> Self {
        NoteFormats { foot: NoteStyle { fmt: NoteFmt::Decimal, start: 1 }, end: NoteStyle { fmt: NoteFmt::LowerRoman, start: 1 } }
    }
}

impl NoteFormats {
    /// Raden i grundformatet: bara det som avviker från standard.
    pub fn to_parts(&self) -> Vec<String> {
        let d = NoteFormats::default();
        let mut v = Vec::new();
        if self.end.fmt != d.end.fmt {
            v.push(format!("endnote-format: {}", self.end.fmt.name()));
        }
        if self.end.start != d.end.start {
            v.push(format!("endnote-start: {}", self.end.start));
        }
        if self.foot.fmt != d.foot.fmt {
            v.push(format!("footnote-format: {}", self.foot.fmt.name()));
        }
        if self.foot.start != d.foot.start {
            v.push(format!("footnote-start: {}", self.foot.start));
        }
        v
    }

    /// Läser en deklaration (`endnote-format` …); `false` om nyckeln inte hör hit.
    pub fn read_decl(&mut self, key: &str, v: &str) -> bool {
        let start = |v: &str| v.trim().parse::<u32>().ok().filter(|n| *n >= 1);
        match key {
            "endnote-format" => self.end.fmt = NoteFmt::from_name(v).unwrap_or(self.end.fmt),
            "footnote-format" => self.foot.fmt = NoteFmt::from_name(v).unwrap_or(self.foot.fmt),
            "endnote-start" => self.end.start = start(v).unwrap_or(1),
            "footnote-start" => self.foot.start = start(v).unwrap_or(1),
            _ => return false,
        }
        true
    }

    /// Texten som visas för etiketten `label`, om den avviker från etikettens egen text (fotnotens
    /// siffror, slutnotens romerska tal). `None` = visa etiketten som den står.
    pub fn display(&self, label: &str) -> Option<String> {
        let (st, n, raw) = if markup::is_endnote_label(label) {
            let raw = &label[1..];
            (self.end, roman_value(raw)?, raw)
        } else if !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()) && label.len() < 7 {
            (self.foot, label.parse::<usize>().ok()?, label)
        } else {
            return None;
        };
        let shown = st.fmt.format(n + st.start as usize - 1);
        (shown != raw).then_some(shown)
    }

    /// Texten för referens eller definition nummer `n` (1-baserat; fotnot eller slutnot).
    pub fn number(&self, end: bool, n: usize) -> String {
        let st = if end { self.end } else { self.foot };
        st.fmt.format(n + st.start as usize - 1)
    }
}

/// Värdet av ett litet romerskt tal (`iv` → 4); `None` om det inte är ett riktigt tal.
pub fn roman_value(s: &str) -> Option<usize> {
    let v = |c: u8| match c {
        b'i' => 1,
        b'v' => 5,
        b'x' => 10,
        b'l' => 50,
        b'c' => 100,
        b'd' => 500,
        b'm' => 1000,
        _ => 0,
    };
    let b = s.as_bytes();
    let mut total = 0i64;
    for (i, &c) in b.iter().enumerate() {
        let x = v(c);
        if x == 0 {
            return None;
        }
        total += if b.get(i + 1).is_some_and(|&n| v(n) > x) { -x } else { x };
    }
    let total = usize::try_from(total).ok().filter(|t| *t > 0)?;
    (markup::roman(total) == s).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(NoteFmt::LowerLetter.format(27), "aa");
        assert_eq!(NoteFmt::UpperLetter.format(2), "B");
        assert_eq!(NoteFmt::UpperRoman.format(4), "IV");
        assert_eq!(NoteFmt::Chicago.format(2), "†");
        assert_eq!(NoteFmt::Chicago.format(5), "**");
        assert_eq!(NoteFmt::Decimal.format(12), "12");
    }

    #[test]
    fn roman_values_round_trip() {
        for n in 1..400 {
            assert_eq!(roman_value(&markup::roman(n)), Some(n));
        }
        assert_eq!(roman_value("iiv"), None);
    }

    #[test]
    fn display_follows_format() {
        let mut f = NoteFormats::default();
        assert_eq!(f.display("1"), None);
        assert_eq!(f.display("eii"), None);
        assert_eq!(f.display("abc"), None);
        f.end.fmt = NoteFmt::Decimal;
        f.foot.fmt = NoteFmt::LowerRoman;
        assert_eq!(f.display("eiii").as_deref(), Some("3"));
        assert_eq!(f.display("2").as_deref(), Some("ii"));
        f.foot.start = 4;
        assert_eq!(f.display("1").as_deref(), Some("iv"));
        assert_eq!(f.number(false, 2), "v");
    }

    #[test]
    fn line_parts_roundtrip() {
        let mut f = NoteFormats::default();
        assert!(f.to_parts().is_empty());
        f.end.fmt = NoteFmt::Chicago;
        f.foot.fmt = NoteFmt::UpperLetter;
        f.foot.start = 3;
        let mut g = NoteFormats::default();
        for p in f.to_parts() {
            let (k, v) = p.split_once(": ").unwrap();
            assert!(g.read_decl(k, v));
        }
        assert_eq!(f, g);
    }
}
