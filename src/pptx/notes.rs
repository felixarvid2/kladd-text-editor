//! Talaranteckningar (`p:notes`) och styckestext ur `a:txBody`: läses direkt ur XML:en (ingen
//! utläggning), för anteckningspanelen, sökningen och *Redigera en kopia*.

use super::inherit;
use super::xml::Node;
use super::Deck;

/// Ett textstycke: nivå (`lvl`, 0–8) och körningar (text, fet, kursiv).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Para {
    pub level: usize,
    pub runs: Vec<Run>,
}

/// En körning med samma formatering. `text` kan innehålla `\n` (radbrytning `a:br`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

impl Para {
    /// Styckets text utan formatering.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    pub fn is_blank(&self) -> bool {
        self.runs.iter().all(|r| r.text.trim().is_empty())
    }
}

/// Styckena i en `a:txBody` (eller `p:txBody`). Fält ger sin sparade text; `‹#›` blir bildnumret.
pub fn paragraphs(body: &Node, slide_no: i64) -> Vec<Para> {
    let mut out = Vec::new();
    for p in body.children_named("a:p") {
        let level = p.child("a:pPr").and_then(|pp| pp.attr_i64("lvl")).unwrap_or(0).clamp(0, 8) as usize;
        let mut runs: Vec<Run> = Vec::new();
        for c in &p.children {
            let (text, rpr) = match c.name.as_str() {
                "a:r" | "a:fld" => {
                    let mut t = c.child("a:t").map(|t| t.text.clone()).unwrap_or_default();
                    if c.name == "a:fld" && c.attr("type") == Some("slidenum") {
                        t = slide_no.to_string();
                    }
                    // Lodrät tabb (Skift+Enter i äldre filer) är en radbrytning.
                    (t.replace('\u{b}', "\n"), c.child("a:rPr"))
                }
                "a:br" => ("\n".to_owned(), c.child("a:rPr")),
                _ => continue,
            };
            let bold = rpr.and_then(|r| r.attr_bool("b")).unwrap_or(false);
            let italic = rpr.and_then(|r| r.attr_bool("i")).unwrap_or(false);
            match runs.last_mut() {
                Some(last) if last.bold == bold && last.italic == italic => last.text.push_str(&text),
                _ => runs.push(Run { text, bold, italic }),
            }
        }
        out.push(Para { level, runs });
    }
    out
}

/// Formerna i ett träd i ritordning, genom grupper.
pub fn shapes(tree: &Node) -> Vec<&Node> {
    let mut out = Vec::new();
    for c in &tree.children {
        match c.name.as_str() {
            "p:grpSp" => out.extend(shapes(c)),
            "p:sp" | "p:pic" | "p:graphicFrame" | "p:cxnSp" => out.push(c),
            _ => {}
        }
    }
    out
}

/// Anteckningarna till bild `i` som stycken (tomma stycken i början och slutet borttagna).
/// Texten står i anteckningssidans brödtextplatshållare (`type="body"`).
pub fn notes(deck: &Deck, i: usize) -> Vec<Para> {
    let Some(part) = deck.slides.get(i).and_then(|s| s.notes.as_deref()) else { return Vec::new() };
    let Some(root) = deck.pkg.xml(part) else { return Vec::new() };
    let Some(tree) = root.path(&["p:cSld", "p:spTree"]) else { return Vec::new() };
    let no = deck.slide_number(i);
    let mut paras: Vec<Para> = Vec::new();
    for sp in shapes(tree) {
        if inherit::placeholder(sp).is_none_or(|ph| ph.typ != "body") {
            continue;
        }
        if let Some(body) = sp.child("p:txBody") {
            paras.extend(paragraphs(body, no));
        }
    }
    while paras.last().is_some_and(Para::is_blank) {
        paras.pop();
    }
    let lead = paras.iter().take_while(|p| p.is_blank()).count();
    paras.drain(..lead);
    paras
}

/// Anteckningarna som ren text, ett stycke per rad (tom sträng om bilden saknar anteckningar).
pub fn notes_text(deck: &Deck, i: usize) -> String {
    notes(deck, i).iter().map(Para::text).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(xml: &str) -> Node {
        let src = format!(r#"<p:txBody xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">{xml}</p:txBody>"#);
        super::super::xml::parse(src.as_bytes()).expect("xml")
    }

    #[test]
    fn stycken_med_nivå_formatering_och_radbrytning() {
        let b = body(r#"<a:p><a:pPr lvl="1"/><a:r><a:rPr b="1"/><a:t>Fet</a:t></a:r><a:r><a:rPr b="1"/><a:t> mer</a:t></a:r><a:br/><a:r><a:rPr i="1"/><a:t>kursiv</a:t></a:r></a:p><a:p><a:fld type="slidenum"><a:t>‹#›</a:t></a:fld></a:p>"#);
        let p = paragraphs(&b, 7);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].level, 1);
        assert_eq!(p[0].runs[0], Run { text: "Fet mer".into(), bold: true, italic: false });
        assert_eq!(p[0].text(), "Fet mer\nkursiv");
        assert!(p[0].runs[2].italic);
        assert_eq!(p[1].text(), "7");
    }
}
