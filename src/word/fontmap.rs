//! Ersättningstabell för Office-typsnitt: vilket fritt, metriskt kompatibelt typsnitt som
//! används när originalet saknas. Delas av dokumentläget (`fonts.rs`) och PDF-exporten
//! (`pdf_write.rs`), så att båda visar samma sak.
//!
//! | Office-typsnitt | Ersättare | Källa |
//! |---|---|---|
//! | Calibri, Calibri Light | Carlito | inbyggt |
//! | Cambria | Caladea | inbyggt |
//! | Arial, Helvetica | Liberation Sans | systemet |
//! | Arial Narrow | Liberation Sans Narrow | systemet |
//! | Times New Roman, Times | Liberation Serif | systemet |
//! | Courier New, Courier | Liberation Mono | systemet |
//! | Georgia | Gelasio | systemet |
//!
//! Calibri Light har ingen fri, metriskt lika variant (Carlito finns bara i normal och fet vikt),
//! så rubriker i Calibri Light visas i Carlito i normal vikt: samma radbrytning som i Word,
//! men något kraftigare streck. Liberation och Gelasio buntas inte: Windows och macOS har de
//! riktiga typsnitten, och Linux har nästan alltid Liberation. Saknas de används systemets
//! generiska serif/mono.
#![allow(dead_code)]

/// Typsnitt som följer med i binären.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bundled {
    Carlito,
    Caladea,
}

pub const CALADEA: &[u8] = include_bytes!("../../assets/fonts/Caladea-Regular.ttf");
pub const CALADEA_BOLD: &[u8] = include_bytes!("../../assets/fonts/Caladea-Bold.ttf");
pub const CALADEA_ITALIC: &[u8] = include_bytes!("../../assets/fonts/Caladea-Italic.ttf");
pub const CALADEA_BOLD_ITALIC: &[u8] = include_bytes!("../../assets/fonts/Caladea-BoldItalic.ttf");

impl Bundled {
    /// Typsnittsfilen för vikt och stil.
    pub fn data(self, bold: bool, italic: bool) -> &'static [u8] {
        match (self, bold, italic) {
            (Bundled::Carlito, false, false) => crate::theme::CARLITO,
            (Bundled::Carlito, true, false) => crate::theme::CARLITO_BOLD,
            (Bundled::Carlito, false, true) => crate::theme::CARLITO_ITALIC,
            (Bundled::Carlito, true, true) => crate::theme::CARLITO_BOLD_ITALIC,
            (Bundled::Caladea, false, false) => CALADEA,
            (Bundled::Caladea, true, false) => CALADEA_BOLD,
            (Bundled::Caladea, false, true) => CALADEA_ITALIC,
            (Bundled::Caladea, true, true) => CALADEA_BOLD_ITALIC,
        }
    }
}

/// Namnet utan mellanslag och med små bokstäver.
pub fn normalize(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// Den fria ersättaren för ett Office-typsnitt (familjenamn), om vi känner en.
pub fn substitute(name: &str) -> Option<&'static str> {
    Some(match normalize(name).as_str() {
        "calibri" | "calibrilight" | "carlito" => "Carlito",
        "cambria" | "caladea" => "Caladea",
        "arial" | "helvetica" | "liberationsans" => "Liberation Sans",
        "arialnarrow" | "liberationsansnarrow" => "Liberation Sans Narrow",
        "timesnewroman" | "times" | "liberationserif" => "Liberation Serif",
        "couriernew" | "courier" | "liberationmono" => "Liberation Mono",
        "georgia" | "gelasio" => "Gelasio",
        _ => return None,
    })
}

/// Det inbyggda typsnitt som ersätter `name`, om det är Calibri/Carlito eller Cambria/Caladea.
pub fn bundled(name: &str) -> Option<Bundled> {
    match substitute(name)? {
        "Carlito" => Some(Bundled::Carlito),
        "Caladea" => Some(Bundled::Caladea),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabellen() {
        assert_eq!(substitute("Calibri"), Some("Carlito"));
        assert_eq!(substitute("Calibri Light"), Some("Carlito"));
        assert_eq!(substitute("Cambria"), Some("Caladea"));
        assert_eq!(substitute("Arial"), Some("Liberation Sans"));
        assert_eq!(substitute("Arial Narrow"), Some("Liberation Sans Narrow"));
        assert_eq!(substitute("Times New Roman"), Some("Liberation Serif"));
        assert_eq!(substitute("Courier New"), Some("Liberation Mono"));
        assert_eq!(substitute("Georgia"), Some("Gelasio"));
        assert_eq!(substitute("Verdana"), None);
    }

    #[test]
    fn inbyggda() {
        assert_eq!(bundled(" cambria "), Some(Bundled::Caladea));
        assert_eq!(bundled("Calibri Light"), Some(Bundled::Carlito));
        assert_eq!(bundled("Arial"), None);
        for b in [Bundled::Carlito, Bundled::Caladea] {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                assert!(b.data(bold, italic).len() > 10_000);
            }
        }
    }
}
