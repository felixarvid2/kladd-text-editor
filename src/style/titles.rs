//! Wordstilarna Titel (Title) och Underrubrik (Subtitle) i Markdown-texten.
//!
//! De skrivs som rubriker med en klass i slutet av raden (Markdowns rubrikattribut):
//!
//! ```text
//! # Mitt dokument {.title}
//! ## Med en underrubrik {.subtitle}
//! ```
//!
//! Dokumentläget döljer `{.title}` som övrig markup; läsare och skrivare för docx, odt, pdf och
//! export använder `OPTS` och `kind` så att de känner igen stilarna och en vanlig Markdown-
//! läsare ändå ser rubriker.
use pulldown_cmark::{CowStr, Options};

/// Tolkaralternativet som gör att `{.title}` blir klass på rubriken i stället för text.
pub const OPTS: Options = Options::ENABLE_HEADING_ATTRIBUTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleKind {
    Title,
    Subtitle,
}

impl TitleKind {
    pub fn class(self) -> &'static str {
        match self {
            TitleKind::Title => "title",
            TitleKind::Subtitle => "subtitle",
        }
    }

    /// Markören sist på raden, t.ex. `{.title}`.
    pub fn marker(self) -> &'static str {
        match self {
            TitleKind::Title => "{.title}",
            TitleKind::Subtitle => "{.subtitle}",
        }
    }

    /// Rubriknivån raden skrivs med (`#` / `##`).
    pub fn level(self) -> u8 {
        match self {
            TitleKind::Title => 1,
            TitleKind::Subtitle => 2,
        }
    }
}

/// Vilken av stilarna rubrikens klasser pekar ut.
pub fn kind(classes: &[CowStr<'_>]) -> Option<TitleKind> {
    classes.iter().find_map(|c| match c.as_ref() {
        "title" => Some(TitleKind::Title),
        "subtitle" => Some(TitleKind::Subtitle),
        _ => None,
    })
}

/// Markören i slutet av `line` (utan radbrytning): (byteläge där blanksteget före börjar, slag).
pub fn trailing_marker(line: &str) -> Option<(usize, TitleKind)> {
    let t = line.trim_end();
    for k in [TitleKind::Title, TitleKind::Subtitle] {
        if let Some(head) = t.strip_suffix(k.marker()) {
            let h = head.trim_end_matches([' ', '\t']);
            if h.len() < head.len() || h.is_empty() {
                return Some((h.len(), k));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_found_at_line_end() {
        assert_eq!(trailing_marker("# Hej {.title}"), Some((5, TitleKind::Title)));
        assert_eq!(trailing_marker("## Hej  {.subtitle}  "), Some((6, TitleKind::Subtitle)));
        assert_eq!(trailing_marker("# Hej{.title}"), None);
        assert_eq!(trailing_marker("# Hej"), None);
    }

    #[test]
    fn parser_gives_classes() {
        use pulldown_cmark::{Event, Parser, Tag};
        let md = "# Mitt dokument {.title}\n\n## Under {.subtitle}\n\n## Vanlig\n";
        let kinds: Vec<Option<TitleKind>> = Parser::new_ext(md, OPTS)
            .filter_map(|e| match e {
                Event::Start(Tag::Heading { classes, .. }) => Some(kind(&classes)),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, vec![Some(TitleKind::Title), Some(TitleKind::Subtitle), None]);
    }
}
