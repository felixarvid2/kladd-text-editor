//! Textformat i redigeringsläget: teckenstorlek, textfärg, justering och punktlista. Ändringarna
//! skrivs som PowerPoint skriver dem (`sz` i hundradels punkt, `a:solidFill` med `a:schemeClr`/
//! `a:srgbClr`, `algn`, `a:buNone`/`a:buChar`) och med barnen i schemats ordning.
//!
//! Alla funktioner arbetar på `TextModel` mellan två lägen. Utan markering (samma läge) gäller
//! ändringen ordet vid markören; ett tomt stycke får formatet på `a:endParaRPr`, som nästa
//! skrivna tecken ärver.

use super::edit_text::{ParaFmt, Pos, TextModel, Item};
use super::edit_xml::XElem;

/// Textfärgen som skrivs: en temafärg (`accent1`, `tx1` …) eller en RGB-färg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextColor {
    Scheme(&'static str),
    Rgb([u8; 3]),
}

/// Teckenstorlekarna i PowerPoints storleksmeny (punkter).
pub const SIZES: &[f32] = &[8.0, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 40.0, 44.0, 48.0, 54.0, 60.0, 66.0, 72.0, 80.0, 88.0, 96.0];

/// Nästa större/mindre storlek i listan (som Ctrl+Skift+> och Ctrl+Skift+<).
pub fn step_size(cur: f32, up: bool) -> f32 {
    if up {
        SIZES.iter().copied().find(|&s| s > cur + 0.01).unwrap_or_else(|| (cur + 10.0).min(400.0))
    } else {
        SIZES.iter().rev().copied().find(|&s| s < cur - 0.01).unwrap_or_else(|| (cur - 1.0).max(1.0))
    }
}

/// Barn i `a:rPr` som kommer efter fyllningen (schemats ordning).
const RPR_AFTER_FILL: &[&str] = &["a:effectLst", "a:effectDag", "a:highlight", "a:uLnTx", "a:uLn", "a:uFillTx", "a:uFill", "a:latin", "a:ea", "a:cs", "a:sym", "a:hlinkClick", "a:hlinkMouseOver", "a:rtl", "a:extLst"];
const FILLS: &[&str] = &["a:noFill", "a:solidFill", "a:gradFill", "a:blipFill", "a:pattFill", "a:grpFill"];
/// Punktelementen i `a:pPr` (en av dem) och det som kommer efter dem.
const BULLETS: &[&str] = &["a:buNone", "a:buAutoNum", "a:buChar", "a:buBlip"];
const PPR_AFTER_BULLET: &[&str] = &["a:tabLst", "a:defRPr", "a:extLst"];

/// Storleken (utan autoanpassning) i stycke `pi` vid tecknet `ch`.
fn size_at(fmts: &[ParaFmt], pi: usize, ch: usize) -> f32 {
    let Some(f) = fmts.get(pi) else { return 18.0 };
    f.runs.iter().find(|r| r.0 <= ch && ch < r.1).or(f.runs.last()).map_or(18.0, |r| r.2)
}

impl TextModel {
    /// Elementnamnet med modellens prefix.
    fn tag(&self, l: &str) -> String {
        if self.prefix().is_empty() { l.to_owned() } else { format!("{}:{l}", self.prefix()) }
    }

    /// Läget från och med/till: ordnade, och ordet vid markören när intervallet är tomt.
    fn format_span(&self, a: Pos, b: Pos) -> (Pos, Pos) {
        let (mut a, mut b) = if a <= b { (self.clamp(a), self.clamp(b)) } else { (self.clamp(b), self.clamp(a)) };
        if a == b {
            a = self.word_start(a);
            b = self.word_end(b);
        }
        (a, b)
    }

    /// Anropar `f(rPr, stycke, tecken)` för varje körning mellan lägena (körningarna delas vid
    /// gränserna) och för `a:endParaRPr` i stycken där intervallet når slutet (tomma stycken med).
    /// Ger om något ändrades.
    pub fn edit_runs(&mut self, a: Pos, b: Pos, f: &mut dyn FnMut(&mut XElem, usize, usize)) -> bool {
        let (a, b) = self.format_span(a, b);
        let mut any = false;
        let rpr_tag = self.tag("rPr");
        let end_tag = self.tag("endParaRPr");
        for pi in a.para..=b.para {
            let len = self.paras[pi].len();
            let from = if pi == a.para { a.ch } else { 0 };
            let to = if pi == b.para { b.ch } else { len };
            if from >= to && !(len == 0 && from == 0) {
                continue;
            }
            let (i, j) = if from < to {
                let i = self.split_at(pi, from);
                let j = self.split_at(pi, to);
                (i, j)
            } else {
                (0, 0)
            };
            // Körningarnas startlägen räknas före ändringen av listan (längderna ändras inte).
            let mut starts = Vec::new();
            let mut at = 0;
            for it in &self.paras[pi].items {
                starts.push(at);
                at += it.len();
            }
            let empty = self.frag(&format!("<{rpr_tag}/>"));
            let end_empty = self.frag(&format!("<{end_tag}/>"));
            let para = &mut self.paras[pi];
            para.touch();
            for k in i..j {
                if let Item::Run { rpr, .. } = &mut para.items[k] {
                    if rpr.is_none() {
                        *rpr = empty.clone();
                    }
                    if let Some(r) = rpr {
                        f(r, pi, starts[k]);
                        any = true;
                    }
                }
            }
            if to == len {
                if para.end.is_none() {
                    para.end = end_empty;
                }
                if let Some(e) = &mut para.end {
                    f(e, pi, len);
                    any = true;
                }
            }
        }
        any
    }

    /// Sätter teckenstorleken (punkter) mellan lägena.
    pub fn set_size(&mut self, a: Pos, b: Pos, pt: f32) -> bool {
        let sz = ((pt * 100.0).round() as i64).clamp(100, 400_000).to_string();
        self.edit_runs(a, b, &mut |r, _, _| r.set_attr("sz", &sz))
    }

    /// Större/mindre text: varje körning går till nästa storlek i listan, räknat från sin egen
    /// (ärvda) storlek ur `fmts`.
    pub fn step_size(&mut self, a: Pos, b: Pos, up: bool, fmts: &[ParaFmt]) -> bool {
        self.edit_runs(a, b, &mut |r, pi, ch| {
            let cur = r.attr("sz").and_then(|v| v.parse::<f32>().ok()).map_or_else(|| size_at(fmts, pi, ch), |v| v / 100.0);
            let new = step_size(cur, up);
            r.set_attr("sz", &((new * 100.0).round() as i64).to_string());
        })
    }

    /// Sätter textfärgen (`None` = tillbaka till ärvd färg) mellan lägena.
    pub fn set_color(&mut self, a: Pos, b: Pos, color: Option<TextColor>) -> bool {
        let pre = self.prefix().to_owned();
        let q = |l: &str| if pre.is_empty() { l.to_owned() } else { format!("{pre}:{l}") };
        let fill = color.map(|c| match c {
            TextColor::Scheme(n) => format!("<{}><{} val=\"{n}\"/></{}>", q("solidFill"), q("schemeClr"), q("solidFill")),
            TextColor::Rgb([r, g, b]) => format!("<{}><{} val=\"{r:02X}{g:02X}{b:02X}\"/></{}>", q("solidFill"), q("srgbClr"), q("solidFill")),
        });
        let el = fill.and_then(|x| self.frag(&x));
        self.edit_runs(a, b, &mut |r, _, _| {
            for f in FILLS {
                r.remove_children(f);
            }
            if let Some(e) = &el {
                r.insert_before(e.clone(), RPR_AFTER_FILL);
            }
        })
    }

    /// Sätter justeringen (`l`, `ctr`, `r`, `just`) i styckena mellan lägena.
    pub fn set_align(&mut self, a: Pos, b: Pos, algn: &str) {
        let (a, b) = if a <= b { (self.clamp(a), self.clamp(b)) } else { (self.clamp(b), self.clamp(a)) };
        let tag = self.tag("pPr");
        for pi in a.para..=b.para {
            if self.paras[pi].ppr.is_none() {
                self.paras[pi].ppr = self.frag(&format!("<{tag}/>"));
            }
            let para = &mut self.paras[pi];
            para.touch();
            if let Some(p) = &mut para.ppr {
                p.set_attr("algn", algn);
            }
        }
    }

    /// Slår på/av punkter i styckena mellan lägena. På: `a:buFont` + `a:buChar` med indrag; av:
    /// `a:buNone` utan indrag (som PowerPoint).
    pub fn set_bullet(&mut self, a: Pos, b: Pos, on: bool) {
        let (a, b) = if a <= b { (self.clamp(a), self.clamp(b)) } else { (self.clamp(b), self.clamp(a)) };
        let pre = self.prefix().to_owned();
        let q = |l: &str| if pre.is_empty() { l.to_owned() } else { format!("{pre}:{l}") };
        for pi in a.para..=b.para {
            if self.paras[pi].ppr.is_none() {
                self.paras[pi].ppr = self.frag(&format!("<{}/>", q("pPr")));
            }
            let lvl = self.paras[pi].level();
            let none = self.frag(&format!("<{}/>", q("buNone")));
            let font = self.frag(&format!("<{} typeface=\"Arial\" panose=\"020B0604020202020204\" pitchFamily=\"34\" charset=\"0\"/>", q("buFont")));
            let chr = self.frag(&format!("<{} char=\"•\"/>", q("buChar")));
            let para = &mut self.paras[pi];
            para.touch();
            let Some(p) = &mut para.ppr else { continue };
            for n in BULLETS.iter().chain(&["a:buFont", "a:buFontTx"]) {
                p.remove_children(n);
            }
            if on {
                p.set_attr("marL", &(285_750 + 457_200 * lvl).to_string());
                p.set_attr("indent", "-285750");
                if let (Some(f), Some(c)) = (font, chr) {
                    p.insert_before(f, &[BULLETS, PPR_AFTER_BULLET].concat());
                    p.insert_before(c, PPR_AFTER_BULLET);
                }
            } else {
                p.set_attr("marL", "0");
                p.set_attr("indent", "0");
                if let Some(n) = none {
                    p.insert_before(n, PPR_AFTER_BULLET);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::edit_xml;

    const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

    fn model(body: &str) -> (TextModel, edit_xml::XDoc) {
        let doc = edit_xml::parse(format!("<p:sp {NS}><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>").as_bytes()).unwrap();
        (TextModel::from_body(doc.root.child("p:txBody"), &doc.root_scope(), "a"), doc)
    }

    fn write(m: &TextModel, doc: &edit_xml::XDoc) -> String {
        let mut sp = doc.root.clone();
        m.write_into(&mut sp, "p");
        sp.to_xml()
    }

    #[test]
    fn storlekssteg_foljer_listan() {
        assert_eq!(step_size(18.0, true), 20.0);
        assert_eq!(step_size(18.0, false), 16.0);
        assert_eq!(step_size(13.0, true), 14.0);
        assert_eq!(step_size(8.0, false), 7.0);
        assert_eq!(step_size(96.0, true), 106.0);
    }

    #[test]
    fn storlek_pa_markeringen_delar_kormingen() {
        let (mut m, doc) = model(r#"<a:p><a:r><a:rPr lang="sv-SE" b="1"/><a:t>Hej världen</a:t></a:r></a:p>"#);
        assert!(m.set_size(Pos { para: 0, ch: 0 }, Pos { para: 0, ch: 3 }, 32.0));
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:rPr lang="sv-SE" b="1" sz="3200"/><a:t>Hej</a:t>"#), "{out}");
        assert!(out.contains(r#"<a:rPr lang="sv-SE" b="1"/><a:t> världen</a:t>"#), "{out}");
        // Slutet av stycket är inte med: ingen `endParaRPr` skapas.
        assert!(!out.contains("endParaRPr"), "{out}");
    }

    #[test]
    fn storlek_utan_markering_tar_ordet_och_tomt_stycke_slutformatet() {
        let (mut m, doc) = model(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>Hej världen</a:t></a:r></a:p><a:p><a:endParaRPr lang="sv-SE"/></a:p>"#);
        assert!(m.set_size(Pos { para: 0, ch: 5 }, Pos { para: 0, ch: 5 }, 40.0));
        assert!(m.set_size(Pos { para: 1, ch: 0 }, Pos { para: 1, ch: 0 }, 28.0));
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:rPr lang="sv-SE" sz="4000"/><a:t>världen</a:t>"#), "{out}");
        assert!(out.contains(r#"<a:endParaRPr lang="sv-SE" sz="2800"/>"#), "{out}");
    }

    #[test]
    fn storlekssteg_utgar_fran_arvd_storlek() {
        let (mut m, doc) = model(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>Hej</a:t></a:r><a:r><a:rPr sz="1400"/><a:t>då</a:t></a:r></a:p>"#);
        let fmts = vec![ParaFmt { align: Default::default(), bullet: false, runs: vec![(0, 3, 24.0, [0; 3]), (3, 5, 14.0, [0; 3])] }];
        m.step_size(Pos::default(), m.end(), true, &fmts);
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:rPr lang="sv-SE" sz="2800"/><a:t>Hej</a:t>"#), "{out}");
        assert!(out.contains(r#"<a:rPr sz="1600"/><a:t>då</a:t>"#), "{out}");
        assert!(out.contains("<a:endParaRPr sz=\"2800\"/>") || out.contains("<a:endParaRPr sz=\"1600\"/>"), "{out}");
    }

    #[test]
    fn farg_ersatter_fyllning_och_hamnar_i_schemats_ordning() {
        let (mut m, doc) = model(r#"<a:p><a:r><a:rPr lang="sv-SE" b="1"><a:ln w="9525"/><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:effectLst/><a:latin typeface="Arial"/></a:rPr><a:t>Hej</a:t></a:r></a:p>"#);
        m.set_color(Pos::default(), m.end(), Some(TextColor::Scheme("accent2")));
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:rPr lang="sv-SE" b="1"><a:ln w="9525"/><a:solidFill><a:schemeClr val="accent2"/></a:solidFill><a:effectLst/><a:latin typeface="Arial"/></a:rPr>"#), "{out}");
        m.set_color(Pos::default(), m.end(), Some(TextColor::Rgb([0xC0, 0x00, 0x1F])));
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:srgbClr val="C0001F"/>"#) && !out.contains("accent2"), "{out}");
        m.set_color(Pos::default(), m.end(), None);
        let out = write(&m, &doc);
        assert!(!out.contains("solidFill"), "{out}");
    }

    #[test]
    fn farg_pa_kormning_utan_rpr_och_med_hlink() {
        let (mut m, doc) = model(r#"<a:p><a:r><a:t>Hej</a:t></a:r><a:r><a:rPr><a:hlinkClick r:id="rId2" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/></a:rPr><a:t>länk</a:t></a:r></a:p>"#);
        m.set_color(Pos::default(), m.end(), Some(TextColor::Rgb([1, 2, 3])));
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:rPr><a:solidFill><a:srgbClr val="010203"/></a:solidFill></a:rPr><a:t>Hej</a:t>"#), "{out}");
        let at = out.find("solidFill").unwrap();
        assert!(at < out.find("hlinkClick").unwrap(), "fyllningen före länken: {out}");
    }

    #[test]
    fn justering_skrivs_pa_alla_stycken_i_intervallet() {
        let (mut m, doc) = model(r#"<a:p><a:pPr lvl="1" algn="l"/><a:r><a:t>a</a:t></a:r></a:p><a:p><a:r><a:t>b</a:t></a:r></a:p><a:p><a:r><a:t>c</a:t></a:r></a:p>"#);
        m.set_align(Pos { para: 0, ch: 0 }, Pos { para: 1, ch: 1 }, "ctr");
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:pPr lvl="1" algn="ctr"/>"#), "{out}");
        assert!(out.contains(r#"<a:p><a:pPr algn="ctr"/><a:r><a:t>b</a:t></a:r></a:p>"#), "{out}");
        assert!(out.contains(r#"<a:p><a:r><a:t>c</a:t></a:r></a:p>"#), "tredje stycket orört: {out}");
    }

    #[test]
    fn punkter_pa_och_av_i_schemats_ordning() {
        let (mut m, doc) = model(r#"<a:p><a:pPr><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:tabLst/><a:defRPr/></a:pPr><a:r><a:t>a</a:t></a:r></a:p>"#);
        m.set_bullet(Pos::default(), Pos::default(), true);
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:pPr marL="285750" indent="-285750"><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:buFont typeface="Arial" panose="020B0604020202020204" pitchFamily="34" charset="0"/><a:buChar char="•"/><a:tabLst/><a:defRPr/></a:pPr>"#), "{out}");
        m.set_bullet(Pos::default(), Pos::default(), false);
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:pPr marL="0" indent="0"><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:buNone/><a:tabLst/><a:defRPr/></a:pPr>"#), "{out}");
        assert!(!out.contains("buChar") && !out.contains("buFont"), "{out}");
    }
}
