//! Från `p:txBody`/`a:txBody` med arv till textmotorns stycken (`text::Para`) och `bodyPr`.
//! Används av former, tabeller, SmartArt och (senare) diagram.

use super::color::{self, Rgba};
use super::inherit::{self, Sheets};
use super::model::EMU_PER_PT;
use super::text::{self, Align, Anchor, Bullet, Caps, Para, Run, RunStyle, Spacing};
use super::xml::Node;

/// `a:bodyPr` med arv.
#[derive(Clone, Debug)]
pub struct BodyProps {
    pub insets: (f32, f32, f32, f32),
    pub anchor: Anchor,
    pub anchor_ctr: bool,
    pub wrap: bool,
    /// `vert`: `horz`, `vert`, `vert270`, `eaVert`, `wordArtVert` …
    pub vert: String,
    /// Extra rotation av texten (grader).
    pub rot: f32,
    #[allow(dead_code)]
    pub upright: bool,
    pub font_scale: f32,
    pub ln_spc_reduction: f32,
    pub columns: (u32, f32),
    /// `spAutoFit`: rutan växer med texten (vi låter rutan vara).
    #[allow(dead_code)]
    pub sp_auto_fit: bool,
}

fn emu(v: i64) -> f32 {
    (v as f64 / EMU_PER_PT) as f32
}

/// Läser `bodyPr` ur kedjan (formen först).
pub fn body_props(chain: &[&Node]) -> BodyProps {
    let a = |name: &str| inherit::first_attr(chain, name);
    let ins = |name: &str, def: i64| a(name).and_then(super::xml::parse_i64).map_or(emu(def), emu);
    let fit = inherit::first_child_of(chain, &["a:normAutofit", "a:spAutoFit", "a:noAutofit"]);
    let (font_scale, ln_red) = match fit {
        Some(n) if n.name == "a:normAutofit" => (
            n.attr("fontScale").and_then(super::xml::parse_i64).map_or(1.0, |v| v as f32 / 100000.0),
            n.attr("lnSpcReduction").and_then(super::xml::parse_i64).map_or(0.0, |v| v as f32 / 100000.0),
        ),
        _ => (1.0, 0.0),
    };
    BodyProps {
        insets: (ins("lIns", 91440), ins("tIns", 45720), ins("rIns", 91440), ins("bIns", 45720)),
        anchor: match a("anchor") {
            Some("ctr") => Anchor::Middle,
            Some("b") => Anchor::Bottom,
            _ => Anchor::Top,
        },
        anchor_ctr: a("anchorCtr").is_some_and(|v| v == "1" || v == "true"),
        wrap: a("wrap") != Some("none"),
        vert: a("vert").unwrap_or("horz").to_owned(),
        rot: a("rot").and_then(super::xml::parse_i64).map_or(0.0, |v| v as f32 / 60000.0),
        upright: a("upright").is_some_and(|v| v == "1" || v == "true"),
        font_scale: font_scale.clamp(0.01, 1.0),
        ln_spc_reduction: ln_red.clamp(0.0, 0.9),
        columns: (
            a("numCol").and_then(super::xml::parse_i64).unwrap_or(1).clamp(1, 16) as u32,
            a("spcCol").and_then(super::xml::parse_i64).map_or(0.0, emu),
        ),
        sp_auto_fit: fit.is_some_and(|n| n.name == "a:spAutoFit"),
    }
}

/// Det som behövs för att lösa upp text i en form.
pub struct TextEnv<'s, 'a> {
    pub sheets: &'s Sheets<'a>,
    /// Nivålistorna i prioritetsordning (`inherit::list_styles`).
    pub lists: Vec<&'a Node>,
    /// Typsnitt och färg från formens `p:style/a:fontRef`.
    pub font_ref: Option<String>,
    pub font_color: Option<Rgba>,
    pub font_scale: f32,
    /// Bildnumret för `slidenum`-fält.
    pub slide_no: i64,
}

fn spacing(n: Option<&Node>) -> Option<Spacing> {
    let n = n?;
    if let Some(p) = n.child("a:spcPct") {
        return Some(Spacing::Pct(p.attr("val").and_then(super::xml::parse_i64).unwrap_or(100000) as f32 / 100000.0));
    }
    if let Some(p) = n.child("a:spcPts") {
        return Some(Spacing::Pts(p.attr_i64("val").unwrap_or(0) as f32 / 100.0));
    }
    None
}

/// Fyllningen för text: solid färg, medelfärg för övertoning, genomskinlig för `noFill`.
fn text_fill(sheets: &Sheets<'_>, n: &Node) -> Option<Rgba> {
    match n.name.as_str() {
        "a:solidFill" => color::first_color(n).and_then(|c| sheets.color(c, None)),
        "a:noFill" => Some(Rgba { a: 0.0, ..Rgba::BLACK }),
        "a:gradFill" => {
            let stops: Vec<Rgba> = n
                .child("a:gsLst")
                .map(|l| l.children.iter().filter_map(|gs| color::first_color(gs).and_then(|c| sheets.color(c, None))).collect())
                .unwrap_or_default();
            if stops.is_empty() {
                return None;
            }
            let k = stops.len() as f32;
            let s = stops.iter().fold((0.0, 0.0, 0.0, 0.0), |a, c| (a.0 + c.r, a.1 + c.g, a.2 + c.b, a.3 + c.a));
            Some(Rgba { r: s.0 / k, g: s.1 / k, b: s.2 / k, a: s.3 / k })
        }
        "a:pattFill" => n.child("a:fgClr").and_then(color::first_color).and_then(|c| sheets.color(c, None)),
        _ => None,
    }
}

impl TextEnv<'_, '_> {
    /// Körningsstil ur kedjan (körningens `rPr` först, sedan nivåernas `defRPr`).
    pub fn run_style(&self, rpr: Option<&Node>, levels: &[&Node]) -> (RunStyle, Option<String>) {
        let mut chain: Vec<&Node> = Vec::new();
        if let Some(r) = rpr {
            chain.push(r);
        }
        for l in levels {
            if let Some(d) = l.child("a:defRPr") {
                chain.push(d);
            }
        }
        let a = |name: &str| inherit::first_attr(&chain, name);
        let lang = a("lang").map(str::to_owned);
        let theme = self.sheets.theme;
        let face = |name: &str| -> Option<String> {
            chain.iter().find_map(|n| n.child(name).and_then(|c| c.attr("typeface"))).map(|t| inherit::theme_font(theme, t, lang.as_deref()))
        };
        let latin = face("a:latin")
            .filter(|s| !s.is_empty())
            .or_else(|| self.font_ref.as_deref().map(|f| inherit::theme_font(theme, f, None)))
            .unwrap_or_else(|| inherit::theme_font(theme, "+mn-lt", None));
        let ea = face("a:ea").filter(|s| !s.is_empty()).unwrap_or_else(|| inherit::theme_font(theme, "+mn-ea", lang.as_deref()));
        let cs = face("a:cs").filter(|s| !s.is_empty()).unwrap_or_else(|| inherit::theme_font(theme, "+mn-cs", lang.as_deref()));
        let sym = face("a:sym").filter(|s| !s.is_empty());
        let size = a("sz").and_then(super::xml::parse_i64).map_or(18.0, |v| v as f32 / 100.0) * self.font_scale;
        let on = |v: Option<&str>| v.is_some_and(|v| v == "1" || v == "true");
        let mut color = inherit::first_child_of(&chain, inherit::FILLS)
            .and_then(|f| text_fill(self.sheets, f))
            .or(self.font_color)
            .or_else(|| self.sheets.colors.scheme_color("tx1"))
            .unwrap_or(Rgba::BLACK);
        let mut underline = a("u").is_some_and(|u| u != "none");
        // Hyperlänkar får temats länkfärg och understrykning (om inte länken säger "textens färg").
        if let Some(h) = rpr.and_then(|r| r.child("a:hlinkClick")) {
            let keep_text_color = h.find("a:extLst").is_some_and(|e| {
                let mut found = false;
                e.walk(&mut |n| {
                    if n.local() == "hlinkClr" && n.attr("val") == Some("tx") {
                        found = true;
                    }
                });
                found
            });
            if h.attr("r:id").is_some_and(|id| !id.is_empty()) || h.attr("action").is_some() {
                if !keep_text_color {
                    if let Some(c) = self.sheets.colors.scheme_color("hlink") {
                        color = c;
                    }
                }
                underline = true;
            }
        }
        let outline = chain.iter().find_map(|n| n.child("a:ln")).and_then(|ln| {
            let c = ln.child("a:solidFill").and_then(color::first_color).and_then(|c| self.sheets.color(c, None))?;
            let w = ln.attr_i64("w").map_or(0.75, emu);
            Some(super::scene::Stroke { paint: super::scene::Paint::Solid(c), width: w, dash: Vec::new(), cap: super::scene::Cap::Butt, join: super::scene::Join::Round })
        });
        let style = RunStyle {
            latin,
            ea,
            cs,
            sym,
            size: size.max(0.5),
            bold: on(a("b")),
            italic: on(a("i")),
            color,
            outline,
            underline,
            strike: a("strike").is_some_and(|s| s != "noStrike"),
            baseline: a("baseline").and_then(super::xml::parse_i64).map_or(0.0, |v| v as f32 / 100000.0),
            spacing: a("spc").and_then(super::xml::parse_i64).map_or(0.0, |v| v as f32 / 100.0),
            caps: match a("cap") {
                Some("all") => Caps::All,
                Some("small") => Caps::Small,
                _ => Caps::None,
            },
            highlight: chain.iter().find_map(|n| n.child("a:highlight")).and_then(color::first_color).and_then(|c| self.sheets.color(c, None)),
        };
        (style, lang)
    }

    /// Alla stycken i en textkropp.
    pub fn paragraphs(&self, body: &Node) -> Vec<Para> {
        let mut out = Vec::new();
        // Räknare för automatisk numrering per nivå: (schema, nästa nummer).
        let mut counters: Vec<Option<(String, i64)>> = vec![None; 9];
        for p in body.children_named("a:p") {
            let ppr = p.child("a:pPr");
            let lvl = ppr.and_then(|n| n.attr_i64("lvl")).unwrap_or(0).clamp(0, 8) as usize;
            let levels = inherit::level_chain(&self.lists, lvl);
            let mut pchain: Vec<&Node> = Vec::new();
            if let Some(pp) = ppr {
                pchain.push(pp);
            }
            pchain.extend(levels.iter().copied());
            let a = |name: &str| inherit::first_attr(&pchain, name);
            let mut para = Para {
                align: match a("algn") {
                    Some("ctr") => Align::Center,
                    Some("r") => Align::Right,
                    Some("just") | Some("justLow") => Align::Justify,
                    Some("dist") | Some("thaiDist") => Align::Distributed,
                    _ => Align::Left,
                },
                mar_l: a("marL").and_then(super::xml::parse_i64).map_or(0.0, emu),
                mar_r: a("marR").and_then(super::xml::parse_i64).map_or(0.0, emu),
                indent: a("indent").and_then(super::xml::parse_i64).map_or(0.0, emu),
                rtl: a("rtl").is_some_and(|v| v == "1" || v == "true"),
                def_tab: a("defTabSz").and_then(super::xml::parse_i64).map_or(72.0, emu),
                ln_spc: spacing(inherit::first_child_of(&pchain, &["a:lnSpc"])).unwrap_or(Spacing::Pct(1.0)),
                spc_bef: spacing(inherit::first_child_of(&pchain, &["a:spcBef"])).unwrap_or(Spacing::Pts(0.0)),
                spc_aft: spacing(inherit::first_child_of(&pchain, &["a:spcAft"])).unwrap_or(Spacing::Pts(0.0)),
                ..Para::default()
            };
            if let Some(tabs) = inherit::first_child_of(&pchain, &["a:tabLst"]) {
                para.tabs = tabs.children_named("a:tab").filter_map(|t| t.attr("pos").and_then(super::xml::parse_i64)).map(emu).collect();
                para.tabs.sort_by(|a, b| a.total_cmp(b));
            }
            // Körningar.
            let mut first_style: Option<RunStyle> = None;
            for r in &p.children {
                match r.name.as_str() {
                    "a:r" | "a:fld" => {
                        let (st, _) = self.run_style(r.child("a:rPr"), &levels);
                        let mut t = r.child("a:t").map(|t| t.text.clone()).unwrap_or_default();
                        if r.name == "a:fld" && r.attr("type") == Some("slidenum") {
                            t = self.slide_no.to_string();
                        }
                        // Vertikala tabbar och radslut i texten blir blanksteg/radbrytningar.
                        let t = t.replace('\u{b}', "\n");
                        for (i, part) in t.split('\n').enumerate() {
                            if i > 0 {
                                para.runs.push(Run { text: String::new(), style: st.clone(), line_break: true });
                            }
                            if !part.is_empty() {
                                para.runs.push(Run { text: part.to_owned(), style: st.clone(), line_break: false });
                            }
                        }
                        if first_style.is_none() {
                            first_style = Some(st);
                        }
                    }
                    "a:br" => {
                        let (st, _) = self.run_style(r.child("a:rPr"), &levels);
                        para.runs.push(Run { text: String::new(), style: st, line_break: true });
                    }
                    _ => {}
                }
            }
            let (end_style, _) = self.run_style(p.child("a:endParaRPr"), &levels);
            para.end_style = end_style;
            // Punkter och numrering.
            let has_text = para.runs.iter().any(|r| !r.line_break && !r.text.trim().is_empty());
            let bu = inherit::first_child_of(&pchain, &["a:buNone", "a:buChar", "a:buAutoNum", "a:buBlip"]);
            for l in counters.iter_mut().skip(lvl + 1) {
                *l = None;
            }
            match bu {
                Some(b) if has_text && b.name != "a:buNone" => {
                    let base = first_style.clone().unwrap_or_else(|| para.end_style.clone());
                    let mut bst = base.clone();
                    bst.underline = false;
                    bst.strike = false;
                    bst.baseline = 0.0;
                    bst.highlight = None;
                    if let Some(f) = inherit::first_child_of(&pchain, &["a:buFont", "a:buFontTx"]) {
                        if f.name == "a:buFont" {
                            if let Some(tf) = f.attr("typeface") {
                                bst.latin = inherit::theme_font(self.sheets.theme, tf, None);
                                bst.ea = bst.latin.clone();
                                bst.cs = bst.latin.clone();
                            }
                        }
                    }
                    if let Some(c) = inherit::first_child_of(&pchain, &["a:buClr", "a:buClrTx"]) {
                        if c.name == "a:buClr" {
                            if let Some(col) = color::first_color(c).and_then(|n| self.sheets.color(n, None)) {
                                bst.color = col;
                            }
                        }
                    }
                    if let Some(s) = inherit::first_child_of(&pchain, &["a:buSzPct", "a:buSzPts", "a:buSzTx"]) {
                        match s.name.as_str() {
                            "a:buSzPct" => bst.size = base.size * s.attr("val").and_then(super::xml::parse_i64).unwrap_or(100000) as f32 / 100000.0,
                            "a:buSzPts" => bst.size = s.attr_i64("val").unwrap_or(1800) as f32 / 100.0 * self.font_scale,
                            _ => {}
                        }
                    }
                    let text = match b.name.as_str() {
                        "a:buChar" => {
                            counters[lvl] = None;
                            b.attr("char").unwrap_or("•").to_owned()
                        }
                        "a:buAutoNum" => {
                            let scheme = b.attr("type").unwrap_or("arabicPeriod").to_owned();
                            let start = b.attr_i64("startAt").unwrap_or(1);
                            let n = match &counters[lvl] {
                                Some((s, n)) if *s == scheme => *n,
                                _ => start,
                            };
                            counters[lvl] = Some((scheme.clone(), n + 1));
                            auto_number(&scheme, n)
                        }
                        // Bildpunkt: en vanlig punkt tills bilden kan ritas.
                        _ => {
                            counters[lvl] = None;
                            "•".to_owned()
                        }
                    };
                    para.bullet = Some(Bullet { text, style: bst });
                }
                _ => {
                    if has_text {
                        counters[lvl] = None;
                    }
                }
            }
            out.push(para);
        }
        out
    }
}

/// Bygger en hel textruta.
pub fn frame(env: &TextEnv<'_, '_>, body: &Node, bp: &BodyProps) -> text::Frame {
    text::Frame {
        paras: env.paragraphs(body),
        insets: bp.insets,
        anchor: bp.anchor,
        anchor_ctr: bp.anchor_ctr,
        wrap: bp.wrap,
        ln_spc_reduction: bp.ln_spc_reduction,
        columns: bp.columns,
    }
}

fn roman(mut n: i64) -> String {
    if n <= 0 || n > 3999 {
        return n.to_string();
    }
    let table = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: i64) -> String {
    if n <= 0 {
        return n.to_string();
    }
    // a..z, aa..zz (PowerPoint upprepar bokstaven).
    let k = ((n - 1) / 26 + 1) as usize;
    let c = (b'a' + ((n - 1) % 26) as u8) as char;
    std::iter::repeat_n(c, k).collect()
}

/// Texten för ett nummer i ett `ST_TextAutonumberScheme`.
pub fn auto_number(scheme: &str, n: i64) -> String {
    let (body, wrap): (String, &str) = if scheme.starts_with("arabic") {
        (n.to_string(), scheme.trim_start_matches("arabic"))
    } else if scheme.starts_with("romanLc") {
        (roman(n), scheme.trim_start_matches("romanLc"))
    } else if scheme.starts_with("romanUc") {
        (roman(n).to_uppercase(), scheme.trim_start_matches("romanUc"))
    } else if scheme.starts_with("alphaLc") {
        (alpha(n), scheme.trim_start_matches("alphaLc"))
    } else if scheme.starts_with("alphaUc") {
        (alpha(n).to_uppercase(), scheme.trim_start_matches("alphaUc"))
    } else if scheme.starts_with("circleNumDbPlain") || scheme.starts_with("circleNumWdBlackPlain") || scheme.starts_with("circleNumWdWhitePlain") {
        let base = if scheme.starts_with("circleNumWdBlack") { 0x2776 - 1 } else { 0x2460 - 1 };
        let c = if (1..=10).contains(&n) { char::from_u32(base + n as u32).map(String::from) } else { None };
        return c.unwrap_or_else(|| n.to_string());
    } else {
        (n.to_string(), "Period")
    };
    match wrap {
        "ParenBoth" => format!("({body})"),
        "ParenR" => format!("{body})"),
        "Period" => format!("{body}."),
        "Plain" => body,
        "Minus" => format!("- {body} -"),
        "Colon" => format!("{body}:"),
        _ => format!("{body}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numrering() {
        assert_eq!(auto_number("arabicPeriod", 3), "3.");
        assert_eq!(auto_number("arabicParenR", 2), "2)");
        assert_eq!(auto_number("romanUcPeriod", 4), "IV.");
        assert_eq!(auto_number("alphaLcParenBoth", 28), "(bb)");
        assert_eq!(auto_number("arabicPlain", 7), "7");
        assert_eq!(auto_number("circleNumDbPlain", 2), "②");
    }
}
