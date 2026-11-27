//! PowerPoints 74 inbyggda tabellformat (`Medium Style 2 – Accent 1` …), för tabeller vars
//! `a:tableStyleId` saknas i filens `ppt/tableStyles.xml` (python-pptx och äldre program skriver inte
//! alltid in formatet). Datat i `table_builtin.xml` är genererat av `table_builtin_gen.py` ur
//! PowerPoint-sparade filer i licensierade provsamlingar; källan per format står i
//! `table_builtin_sources.md`.

use std::sync::OnceLock;

use super::xml::{self, Node};

/// Den inbyggda listan (`a:tblStyleLst`), tolkad en gång.
pub fn list() -> &'static Node {
    static LIST: OnceLock<Node> = OnceLock::new();
    LIST.get_or_init(|| xml::parse(include_bytes!("table_builtin.xml")).unwrap_or_default())
}

/// Det inbyggda formatet med GUID:en `id` (skiftlägesokänsligt, med eller utan klamrar).
pub fn style(id: &str) -> Option<&'static Node> {
    let id = id.trim().trim_start_matches('{').trim_end_matches('}');
    list().children.iter().find(|s| s.attr("styleId").is_some_and(|v| v.trim_start_matches('{').trim_end_matches('}').eq_ignore_ascii_case(id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alla_74_inbyggda_format_finns() {
        let l = list();
        assert_eq!(l.name, "a:tblStyleLst");
        assert_eq!(l.children.len(), 74);
        for s in &l.children {
            assert_eq!(s.name, "a:tblStyle");
            assert!(s.child("a:wholeTbl").is_some(), "{:?}", s.attr("styleName"));
            assert!(s.attr("styleName").is_some_and(|n| !n.is_empty()));
        }
        let ids: std::collections::HashSet<&str> = l.children.iter().filter_map(|s| s.attr("styleId")).collect();
        assert_eq!(ids.len(), 74);
    }

    #[test]
    fn standardformatet_hittas() {
        let s = style("{5c22544a-7ee6-4342-b048-85bdc9fd1c3a}").expect("Medium Style 2 - Accent 1");
        assert_eq!(s.attr("styleName"), Some("Medium Style 2 - Accent 1"));
        // Rubrikraden fylls med accent 1 och har vit fet text.
        let fr = s.child("a:firstRow").unwrap();
        assert_eq!(fr.path(&["a:tcStyle", "a:fill", "a:solidFill", "a:schemeClr"]).and_then(|c| c.attr("val")), Some("accent1"));
        assert_eq!(fr.child("a:tcTxStyle").and_then(|t| t.attr("b")), Some("on"));
        assert!(style("{00000000-0000-0000-0000-000000000000}").is_none());
        // Familjen följer accentfärgen.
        let a3 = style("{8799B23B-EC83-4686-B30A-512413B5E67A}").unwrap();
        assert_eq!(a3.attr("styleName"), Some("Light Style 3 - Accent 3"));
        let mut accents = Vec::new();
        a3.walk(&mut |n| {
            if n.name == "a:schemeClr" && n.attr("val").is_some_and(|v| v.starts_with("accent")) {
                accents.push(n.attr("val").unwrap().to_owned());
            }
        });
        assert!(!accents.is_empty() && accents.iter().all(|a| a == "accent3"));
    }
}
