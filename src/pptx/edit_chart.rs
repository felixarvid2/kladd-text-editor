//! Diagramdata under redigering: läsning av ett diagrams serier, kategorier, värden och rubriker ur
//! diagramdelen (`ppt/charts/chartN.xml`) till en tabell (`ChartData`), och skrivning tillbaka.
//!
//! **Källbevarande:** diagramdelen tolkas förlustfritt (`edit_xml.rs`). Bara cacharna (`c:strCache`,
//! `c:numCache`, `c:strLit`/`c:numLit`) i seriernas `c:tx`, `c:cat`/`c:xVal` och `c:val`/`c:yVal` skrivs om
//! (med `ptCount` och `formatCode` kvar), och referenserna `c:f` får nya slutrader/-kolumner när antalet
//! kategorier ändras. Allt annat (formatering, `extLst`, `mc:AlternateContent` …) står kvar.
//! - **Ny serie:** en kopia av sista seriens `c:ser` med nytt `idx`/`order`, utan egen färg (`c:spPr`,
//!   `c:dPt`) så att diagrammets stil ger nästa färg, utan trendlinjer och felstaplar, och med referenserna
//!   flyttade en kolumn (eller rad) bort från förra serien.
//! - **Kategorier** som är tal eller datum i cachen (`numRef`) behåller talen när texten inte ändrats; en
//!   ändrad kategori som inte är ett tal gör kategorierna till text (`strRef`/`strCache`).
//! - **Rubriker:** diagrammets rubrik och axlarnas rubriker som ren text i `c:tx/c:rich` (första körningens
//!   formatering behålls). Tom text tar bort rubriken (diagrammets: `autoTitleDeleted`).
//! - **Arbetsboken** (`c:externalData` → `ppt/embeddings/*.xlsx`): cellerna som `c:f` pekar på skrivs
//!   (`edit_chart_xlsx.rs`), så att PowerPoints *Redigera data* visar samma värden. Länkade (externa)
//!   arbetsböcker och diagram utan arbetsbok: bara cachen (`Written::workbook`).

use super::edit::Txn;
use super::edit_chart_xlsx::{self as xlsx, CellVal, Write};
use super::edit_xml::{self, escape, XDoc, XElem, XNode};
use super::model::Deck;

pub const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

/// En kategori: texten som visas och talet i cachen (tal- och datumkategorier).
#[derive(Clone, Debug, PartialEq)]
pub struct Cat {
    pub text: String,
    pub num: Option<f64>,
}

/// En serie i tabellen.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesData {
    pub name: String,
    pub vals: Vec<Option<f64>>,
    /// Seriens läge bland diagrammets `c:ser` (i dokumentordning) när tabellen lästes; `None` = ny.
    pub orig: Option<usize>,
}

/// En rubrik: axelns `c:axId` (eller `None` för diagrammets rubrik) och texten (`None` = ingen rubrik).
#[derive(Clone, Debug, PartialEq)]
pub struct TitleData {
    pub axis: Option<String>,
    /// Axelns slag och läge för etiketten i tabellen (`valAx`, `l`).
    pub label: String,
    pub text: Option<String>,
}

/// Diagrammets data som en tabell.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartData {
    pub cats: Vec<Cat>,
    pub series: Vec<SeriesData>,
    pub titles: Vec<TitleData>,
    /// Kategorierna är tal i cachen (punktdiagrammets x-värden, datum).
    pub numeric_cats: bool,
}

/// Diagramdelen för toppnivåformen `index` på bild `slide` (en `p:graphicFrame` med `c:chart`).
pub fn chart_part(deck: &Deck, slide: usize, index: usize) -> Option<String> {
    let gf = super::edit_ops::node(deck, slide, index)?;
    if gf.name != "p:graphicFrame" {
        return None;
    }
    let id = gf.path(&["a:graphic", "a:graphicData", "c:chart"])?.attr("r:id")?;
    let s = deck.slides.get(slide)?;
    deck.pkg.rels(&s.part).into_iter().find(|r| r.id == id && !r.external).map(|r| r.target).filter(|t| deck.pkg.parts.contains_key(t))
}

/// Är formen ett diagram vars data kan redigeras?
pub fn is_chart(deck: &Deck, slide: usize, index: usize) -> bool {
    chart_part(deck, slide, index).and_then(|p| deck.pkg.read(&p)).and_then(|b| edit_xml::parse(&b)).is_some_and(|d| plot_area(&d.root).is_some_and(|pa| sers(pa).next().is_some()))
}

fn plot_area(root: &XElem) -> Option<&XElem> {
    root.path(&["c:chart", "c:plotArea"])
}

/// Alla `c:ser` i ritytan i dokumentordning.
fn sers(pa: &XElem) -> impl Iterator<Item = &XElem> {
    pa.elems().filter(|p| p.local().ends_with("Chart")).flat_map(|p| p.elems().filter(|e| e.name == "c:ser"))
}

// ------------------------------------------------------------------------------------- läsning

/// Punkterna i en cache som text (index → text), och antalet.
fn cache_pts(cache: &XElem) -> Vec<String> {
    let src = cache.child("c:lvl").unwrap_or(cache);
    let n = cache.child("c:ptCount").and_then(|c| c.attr_i64("val")).unwrap_or(0).clamp(0, 100_000) as usize;
    let mut out = vec![String::new(); n];
    for pt in src.elems().filter(|e| e.name == "c:pt") {
        let Some(i) = pt.attr_i64("idx").filter(|i| (0..100_000).contains(i)) else { continue };
        let i = i as usize;
        if i >= out.len() {
            out.resize(i + 1, String::new());
        }
        out[i] = pt.child("c:v").map(|v| v.text()).unwrap_or_default();
    }
    out
}

/// Cachen i en datakälla (`c:tx`, `c:cat`, `c:val` …): (referensen, cachen eller literalen).
fn source(n: &XElem) -> (Option<&XElem>, Option<&XElem>) {
    for (r, c) in [("c:strRef", "c:strCache"), ("c:numRef", "c:numCache"), ("c:multiLvlStrRef", "c:multiLvlStrCache")] {
        if let Some(re) = n.child(r) {
            return (Some(re), re.child(c));
        }
    }
    (None, n.child("c:strLit").or_else(|| n.child("c:numLit")))
}

fn ser_name(ser: &XElem) -> String {
    let Some(tx) = ser.child("c:tx") else { return String::new() };
    if let Some(v) = tx.child("c:v") {
        return v.text();
    }
    source(tx).1.map(|c| cache_pts(c).join(" ")).unwrap_or_default()
}

fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".").replace([' ', '\u{a0}'], "");
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Ett tal som text för tabellen (kortaste form).
pub fn num_text(v: f64) -> String {
    let s = format!("{v}");
    s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
}

/// Rubrikens text: `c:tx/c:rich` (styckena med radbrytning) eller `c:tx/c:strRef`.
fn title_text(title: &XElem) -> String {
    let Some(tx) = title.child("c:tx") else { return String::new() };
    if let Some(rich) = tx.child("c:rich") {
        let paras: Vec<String> = rich
            .elems()
            .filter(|p| p.name == "a:p")
            .map(|p| {
                let mut s = String::new();
                p.walk(&mut |e| {
                    if e.name == "a:t" {
                        s.push_str(&e.text());
                    }
                });
                s
            })
            .collect();
        return paras.join("\n");
    }
    source(tx).1.map(|c| cache_pts(c).join(" ")).unwrap_or_default()
}

/// Läser diagrammets data ur diagramdelens XML.
pub fn read(doc: &XDoc) -> Option<ChartData> {
    let chart = doc.root.child("c:chart")?;
    let pa = chart.child("c:plotArea")?;
    let all: Vec<&XElem> = sers(pa).collect();
    if all.is_empty() {
        return None;
    }
    // Kategorierna ur första serien som har några (punktdiagram: x-värdena).
    let mut cats: Vec<Cat> = Vec::new();
    let mut numeric = false;
    for ser in &all {
        let Some(cn) = ser.child("c:cat").or_else(|| ser.child("c:xVal")) else { continue };
        let (_, cache) = source(cn);
        let Some(cache) = cache else { continue };
        numeric = matches!(cache.name.as_str(), "c:numCache" | "c:numLit");
        let fmt = cache.child("c:formatCode").map(|f| f.text()).filter(|f| !f.is_empty() && !f.eq_ignore_ascii_case("general"));
        cats = cache_pts(cache)
            .into_iter()
            .map(|t| {
                let num = if numeric { parse_num(&t) } else { None };
                let text = match (num, &fmt) {
                    (Some(v), Some(f)) => super::chart_fmt::format(v, f),
                    (Some(v), None) => num_text(v),
                    _ => t,
                };
                Cat { text, num }
            })
            .collect();
        break;
    }
    let series: Vec<SeriesData> = all
        .iter()
        .enumerate()
        .map(|(k, ser)| {
            let vals = ser.child("c:val").or_else(|| ser.child("c:yVal")).and_then(|v| source(v).1).map(|c| cache_pts(c).iter().map(|t| parse_num(t)).collect()).unwrap_or_default();
            SeriesData { name: ser_name(ser), vals, orig: Some(k) }
        })
        .collect();
    let n = series.iter().map(|s| s.vals.len()).max().unwrap_or(0).max(cats.len());
    cats.resize(n, Cat { text: String::new(), num: None });
    let series = series.into_iter().map(|mut s| {
        s.vals.resize(n, None);
        s
    });
    let mut titles = vec![TitleData { axis: None, label: String::new(), text: chart.child("c:title").map(title_text) }];
    for ax in pa.elems().filter(|e| matches!(e.local(), "catAx" | "valAx" | "dateAx" | "serAx")) {
        let Some(id) = ax.child("c:axId").and_then(|a| a.attr("val")) else { continue };
        if ax.child("c:delete").and_then(|d| d.attr("val")).is_some_and(|v| v == "1" || v == "true") {
            continue;
        }
        let pos = ax.child("c:axPos").and_then(|p| p.attr("val")).unwrap_or_default();
        titles.push(TitleData { axis: Some(id), label: format!("{}:{pos}", ax.local()), text: ax.child("c:title").map(title_text) });
    }
    Some(ChartData { cats, series: series.collect(), titles, numeric_cats: numeric })
}

/// Diagrammets data för formen (diagramdelen tolkad ur paketet).
pub fn read_shape(deck: &Deck, slide: usize, index: usize) -> Option<(String, ChartData)> {
    let part = chart_part(deck, slide, index)?;
    let doc = edit_xml::parse(&deck.pkg.read(&part)?)?;
    Some((part.clone(), read(&doc)?))
}

// ------------------------------------------------------------------------------------- referenser

/// En områdesreferens `Blad1!$B$2:$B$5` (blad, första kolumn/rad, sista kolumn/rad; 1-baserade).
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub sheet: String,
    pub c0: u32,
    pub r0: u32,
    pub c1: u32,
    pub r1: u32,
}

fn col_num(s: &str) -> Option<u32> {
    let mut n = 0u32;
    for ch in s.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        n = n.checked_mul(26)?.checked_add(ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1)?;
    }
    (n > 0).then_some(n)
}

pub fn col_name(mut n: u32) -> String {
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push(b'A' + r as u8);
        n = (n - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// `$B$2` → (2, 2).
pub fn cell(s: &str) -> Option<(u32, u32)> {
    let s = s.replace('$', "");
    let i = s.find(|c: char| c.is_ascii_digit())?;
    Some((col_num(&s[..i])?, s[i..].parse().ok().filter(|r| *r > 0)?))
}

impl Range {
    pub fn parse(f: &str) -> Option<Range> {
        let f = f.trim().trim_start_matches('=').trim_start_matches('(').trim_end_matches(')');
        let (sheet, cells) = f.rsplit_once('!')?;
        let (a, b) = cells.split_once(':').unwrap_or((cells, cells));
        let (c0, r0) = cell(a)?;
        let (c1, r1) = cell(b)?;
        Some(Range { sheet: sheet.to_owned(), c0, r0, c1: c1.max(c0), r1: r1.max(r0) })
    }

    pub fn format(&self) -> String {
        let a = format!("${}${}", col_name(self.c0), self.r0);
        if self.c0 == self.c1 && self.r0 == self.r1 {
            format!("{}!{a}", self.sheet)
        } else {
            format!("{}!{a}:${}${}", self.sheet, col_name(self.c1), self.r1)
        }
    }

    /// Bladets namn utan citattecken (`'Mitt blad'` → `Mitt blad`).
    pub fn sheet_name(&self) -> String {
        let s = self.sheet.trim();
        if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') { s[1..s.len() - 1].replace("''", "'") } else { s.to_owned() }
    }

    /// Lodrät (en kolumn) eller vågrät.
    pub fn vertical(&self) -> bool {
        self.c0 == self.c1 && self.r1 > self.r0 || self.c0 == self.c1 && self.r0 == self.r1
    }

    /// Samma början, `n` celler långt i sin riktning (`vertical`).
    pub fn resized(&self, n: usize, vertical: bool) -> Range {
        let n = n.max(1) as u32;
        if vertical { Range { c1: self.c0, r1: self.r0 + n - 1, ..self.clone() } } else { Range { r1: self.r0, c1: self.c0 + n - 1, ..self.clone() } }
    }

    /// Flyttad `d` steg vinkelrätt mot riktningen (nästa serie).
    pub fn shifted(&self, d: u32, vertical: bool) -> Range {
        if vertical { Range { c0: self.c0 + d, c1: self.c1 + d, ..self.clone() } } else { Range { r0: self.r0 + d, r1: self.r1 + d, ..self.clone() } }
    }

    /// Cell nummer `i` i området.
    pub fn at(&self, i: usize, vertical: bool) -> (u32, u32) {
        if vertical { (self.c0, self.r0 + i as u32) } else { (self.c0 + i as u32, self.r0) }
    }
}

// ------------------------------------------------------------------------------------- skrivning

fn elem(doc_scope: &[(String, String)], xml: &str) -> Result<XElem, String> {
    edit_xml::fragment_in(doc_scope, xml).into_iter().next().ok_or_else(|| "ogiltig XML i diagrammet".to_owned())
}

fn str_cache_xml(c: &str, vals: &[String]) -> String {
    let pts: String = vals.iter().enumerate().map(|(i, v)| format!("<{c}:pt idx=\"{i}\"><{c}:v>{}</{c}:v></{c}:pt>", escape(v))).collect();
    format!("<{c}:ptCount val=\"{}\"/>{pts}", vals.len())
}

fn num_cache_xml(c: &str, fmt: &str, vals: &[Option<f64>]) -> String {
    let pts: String = vals.iter().enumerate().filter_map(|(i, v)| v.map(|v| format!("<{c}:pt idx=\"{i}\"><{c}:v>{}</{c}:v></{c}:pt>", num_text(v)))).collect();
    format!("<{c}:formatCode>{}</{c}:formatCode><{c}:ptCount val=\"{}\"/>{pts}", escape(fmt), vals.len())
}

/// Byter innehållet i en cache (behåller `c:extLst`).
fn set_cache_children(cache: &mut XElem, scope: &[(String, String)], inner: &str) -> Result<(), String> {
    let ext: Vec<XNode> = cache.children.iter().filter(|n| matches!(n, XNode::Elem(e) if e.name == "c:extLst")).cloned().collect();
    let tag = cache.qname.clone();
    let fresh = elem(scope, &format!("<{tag}>{inner}</{tag}>"))?;
    cache.children = fresh.children;
    cache.children.extend(ext);
    if cache.children.is_empty() {
        cache.set_text("");
    }
    Ok(())
}

/// Vad en skrivning gjorde (för notiser och proven).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Written {
    /// Arbetsboken uppdaterades (`Some(true)`), är länkad/saknas (`Some(false)` / `None`).
    pub workbook: Option<bool>,
}

/// Skriver tabellen till diagrammet (formen `index` på bild `slide`) och dess arbetsbok.
pub fn write(txn: &mut Txn, slide: usize, index: usize, data: &ChartData) -> Result<Written, String> {
    let deck = txn.deck;
    let part = chart_part(deck, slide, index).ok_or_else(|| crate::i18n::t("Diagrammet finns inte längre.").to_owned())?;
    // Oförändrad tabell: ingenting skrivs (och inget ångringssteg).
    if txn.read(&part).and_then(|b| edit_xml::parse(&b)).and_then(|d| read(&d)).as_ref() == Some(data) {
        return Ok(Written::default());
    }
    let writes = write_doc(txn.doc(&part)?, data, Vec::new())?;
    // Arbetsboken.
    let mut written = Written::default();
    let ext = txn.read(&part).and_then(|b| edit_xml::parse(&b)).and_then(|d| d.root.child("c:externalData").and_then(|e| e.attr("r:id")));
    if let Some(id) = ext {
        let rel = txn.raw_rels(&part).into_iter().find(|r| r.0 == id);
        match rel {
            Some((_, _, _, true)) => written.workbook = Some(false),
            Some(_) => {
                let target = txn.rels(&part).into_iter().find(|r| r.id == id).map(|r| r.target);
                if let Some(target) = target.filter(|t| t.to_lowercase().ends_with(".xlsx") || t.to_lowercase().ends_with(".xlsm")) {
                    if let Some(bytes) = txn.read(&target) {
                        let out = xlsx::edit(&bytes, &writes)?;
                        if out != bytes {
                            txn.put_raw(&target, out);
                        }
                        written.workbook = Some(true);
                    }
                }
            }
            None => {}
        }
    }
    Ok(written)
}

/// Skriver tabellen i diagramdelens XML; ger cellerna som ska skrivas i arbetsboken.
pub fn write_doc(doc: &mut XDoc, data: &ChartData, mut writes: Vec<Write>) -> Result<Vec<Write>, String> {
    let scope = doc.root_scope();
    let c = doc.prefix_for(NS_C).unwrap_or_else(|| "c".into());
    let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
    let n = data.cats.len();
    // Kategorierna: tal om alla är tal (och var tal från början), annars text.
    let cat_nums: Option<Vec<Option<f64>>> = data.numeric_cats.then(|| data.cats.iter().map(|c| c.num.or_else(|| parse_num(&c.text))).collect::<Vec<_>>()).filter(|v| v.iter().zip(&data.cats).all(|(n, c)| n.is_some() || c.text.trim().is_empty()));
    let cat_texts: Vec<String> = data.cats.iter().map(|c| c.text.clone()).collect();
    let chart = doc.root.child_mut("c:chart").ok_or("c:chart saknas")?;
    let pa = chart.child_mut("c:plotArea").ok_or("c:plotArea saknas")?;
    // Seriernas plats: (plott, index i plotten) i dokumentordning.
    let mut slots: Vec<(usize, usize)> = Vec::new();
    for (pi, p) in pa.children.iter().enumerate() {
        let XNode::Elem(p) = p else { continue };
        if !p.local().ends_with("Chart") {
            continue;
        }
        for (si, s) in p.children.iter().enumerate() {
            if matches!(s, XNode::Elem(e) if e.name == "c:ser") {
                slots.push((pi, si));
            }
        }
    }
    if slots.is_empty() {
        return Err("diagrammet har inga serier".into());
    }
    // Kopior av gamla serier som finns kvar, och mallen för nya (sista serien).
    let get = |pa: &XElem, k: usize| -> Option<XElem> {
        let (pi, si) = slots.get(k)?;
        match pa.children.get(*pi)? {
            XNode::Elem(p) => match p.children.get(*si)? {
                XNode::Elem(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        }
    };
    let last = get(pa, slots.len() - 1).ok_or("serien saknas")?;
    let last_plot = slots[slots.len() - 1].0;
    let mut max_idx = slots.iter().enumerate().filter_map(|(k, _)| get(pa, k)).map(|s| s.child("c:idx").and_then(|i| i.attr_i64("val")).unwrap_or(0).max(s.child("c:order").and_then(|i| i.attr_i64("val")).unwrap_or(0))).max().unwrap_or(0);
    // Nya serier: referenserna en kolumn/rad längre bort per ny serie.
    let mut new_count = 0u32;
    let mut built: Vec<(usize, XElem)> = Vec::new();
    for sd in &data.series {
        let (plot, mut ser) = match sd.orig.and_then(|k| get(pa, k).map(|s| (slots[k].0, s))) {
            Some(x) => x,
            None => {
                new_count += 1;
                max_idx += 1;
                let mut s = last.clone();
                s.remove_children("c:spPr");
                s.remove_children("c:dPt");
                s.remove_children("c:trendline");
                s.remove_children("c:errBars");
                s.remove_children("c:extLst");
                for name in ["c:idx", "c:order"] {
                    if let Some(e) = s.child_mut(name) {
                        e.set_attr("val", &max_idx.to_string());
                    }
                }
                // Referenserna: värdenas och namnets flyttas, kategoriernas står kvar.
                for name in ["c:tx", "c:val", "c:yVal"] {
                    if let Some(src) = s.child_mut(name) {
                        shift_refs(src, new_count);
                    }
                }
                (last_plot, s)
            }
        };
        update_series(&mut ser, &scope, &c, sd, &cat_texts, cat_nums.as_deref(), n, &mut writes)?;
        built.push((plot, ser));
    }
    // Ersätt seriernas platser: varje plott får sina serier i tabellens ordning.
    let mut per_plot: std::collections::BTreeMap<usize, Vec<XElem>> = std::collections::BTreeMap::new();
    for (p, s) in built {
        per_plot.entry(p).or_default().push(s);
    }
    let plots: Vec<usize> = slots.iter().map(|s| s.0).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    for pi in plots {
        let Some(XNode::Elem(p)) = pa.children.get_mut(pi) else { continue };
        let first = p.children.iter().position(|n| matches!(n, XNode::Elem(e) if e.name == "c:ser"));
        let Some(first) = first else { continue };
        p.children.retain(|n| !matches!(n, XNode::Elem(e) if e.name == "c:ser"));
        let new: Vec<XNode> = per_plot.remove(&pi).unwrap_or_default().into_iter().map(XNode::Elem).collect();
        let at = first.min(p.children.len());
        p.children.splice(at..at, new);
    }
    // Rubriker.
    for td in &data.titles {
        match &td.axis {
            None => set_chart_title(chart_mut(doc)?, &scope, &c, &a, td.text.as_deref())?,
            Some(id) => {
                let pa = chart_mut(doc)?.child_mut("c:plotArea").ok_or("c:plotArea saknas")?;
                if let Some(ax) = pa.elems_mut().find(|e| matches!(e.local(), "catAx" | "valAx" | "dateAx" | "serAx") && e.child("c:axId").and_then(|x| x.attr("val")).as_deref() == Some(id.as_str())) {
                    set_axis_title(ax, &scope, &c, &a, td.text.as_deref())?;
                }
            }
        }
    }
    Ok(writes)
}

fn chart_mut(doc: &mut XDoc) -> Result<&mut XElem, String> {
    doc.root.child_mut("c:chart").ok_or_else(|| "c:chart saknas".to_owned())
}

/// Flyttar referenserna (`c:f`) i en datakälla `d` kolumner/rader.
fn shift_refs(src: &mut XElem, d: u32) {
    src.walk_mut(&mut |e| {
        if e.name == "c:f" {
            if let Some(r) = Range::parse(&e.text()) {
                let v = r.vertical();
                // Namnets cell (en cell) flyttas som värdena: värdena är lodräta om namnet ligger ovanför.
                e.set_text(&r.shifted(d, v || r.c0 == r.c1 && r.r0 == r.r1).format());
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn update_series(ser: &mut XElem, scope: &[(String, String)], c: &str, sd: &SeriesData, cats: &[String], cat_nums: Option<&[Option<f64>]>, n: usize, writes: &mut Vec<Write>) -> Result<(), String> {
    // Namnet.
    if let Some(tx) = ser.child_mut("c:tx") {
        if let Some(v) = tx.child_mut("c:v") {
            v.set_text(&sd.name);
        } else if let Some(re) = tx.child_mut("c:strRef") {
            if let Some(r) = re.child("c:f").and_then(|f| Range::parse(&f.text())) {
                writes.push(Write { sheet: r.sheet_name(), col: r.c0, row: r.r0, val: CellVal::Str(sd.name.clone()) });
            }
            match re.child_mut("c:strCache") {
                Some(cache) => set_cache_children(cache, scope, &str_cache_xml(c, std::slice::from_ref(&sd.name)))?,
                None => {
                    re.children.push(XNode::Elem(elem(scope, &format!("<{c}:strCache>{}</{c}:strCache>", str_cache_xml(c, std::slice::from_ref(&sd.name))))?));
                }
            }
        }
    } else if !sd.name.is_empty() {
        let tx = elem(scope, &format!("<{c}:tx><{c}:v>{}</{c}:v></{c}:tx>", escape(&sd.name)))?;
        ser.insert_before(tx, &["c:spPr", "c:invertIfNegative", "c:pictureOptions", "c:marker", "c:dPt", "c:dLbls", "c:trendline", "c:errBars", "c:cat", "c:xVal", "c:val", "c:yVal", "c:smooth", "c:shape", "c:explosion", "c:bubbleSize", "c:bubble3D", "c:extLst"]);
    }
    // Kategorierna (punktdiagram: x-värdena).
    let cat_name = if ser.child("c:xVal").is_some() { "c:xVal" } else { "c:cat" };
    if let Some(cn) = ser.child_mut(cat_name) {
        write_source(cn, scope, c, n, |k| cats.get(k).cloned().unwrap_or_default(), cat_nums, writes)?;
    }
    // Värdena.
    let val_name = if ser.child("c:yVal").is_some() { "c:yVal" } else { "c:val" };
    if let Some(vn) = ser.child_mut(val_name) {
        let vals: Vec<Option<f64>> = (0..n).map(|k| sd.vals.get(k).copied().flatten()).collect();
        write_source(vn, scope, c, n, |k| vals[k].map(num_text).unwrap_or_default(), Some(&vals), writes)?;
    }
    Ok(())
}

/// Skriver en datakälla (`c:cat`, `c:val` …): referensens område får `n` celler, cachen skrivs om, och
/// cellerna läggs till i `writes`. `nums` = talvärden (då blir cachen en `numCache`), annars text.
fn write_source(src: &mut XElem, scope: &[(String, String)], c: &str, n: usize, text: impl Fn(usize) -> String, nums: Option<&[Option<f64>]>, writes: &mut Vec<Write>) -> Result<(), String> {
    let texts: Vec<String> = (0..n).map(&text).collect();
    // Referens och cache.
    let ref_name = ["c:strRef", "c:numRef", "c:multiLvlStrRef"].into_iter().find(|r| src.child(r).is_some());
    if let Some(rn) = ref_name {
        let re = src.child_mut(rn).ok_or("referens saknas")?;
        let range = re.child("c:f").and_then(|f| Range::parse(&f.text()));
        if let Some(r) = &range {
            let v = r.vertical();
            let nr = r.resized(n, v);
            if let Some(f) = re.child_mut("c:f") {
                f.set_text(&nr.format());
            }
            for (k, t) in texts.iter().enumerate() {
                let (col, row) = nr.at(k, v);
                let val = match nums {
                    Some(ns) => ns.get(k).copied().flatten().map_or(CellVal::Empty, CellVal::Num),
                    None if t.is_empty() => CellVal::Empty,
                    None => CellVal::Str(t.clone()),
                };
                writes.push(Write { sheet: r.sheet_name(), col, row, val });
            }
        }
        let fmt = re.path(&["c:numCache", "c:formatCode"]).map(|f| f.text()).unwrap_or_else(|| "General".into());
        let f_elem = re.child("c:f").cloned();
        let ext: Vec<XNode> = re.children.iter().filter(|n| matches!(n, XNode::Elem(e) if e.name == "c:extLst")).cloned().collect();
        // Ny referens: tal → numRef/numCache, text → strRef/strCache (även om källan var en annan sort).
        let (rtag, ctag, inner) = match nums {
            Some(ns) => ("numRef", "numCache", num_cache_xml(c, &fmt, ns)),
            None => ("strRef", "strCache", str_cache_xml(c, &texts)),
        };
        let mut fresh = elem(scope, &format!("<{c}:{rtag}><{c}:{ctag}>{inner}</{c}:{ctag}></{c}:{rtag}>"))?;
        if let Some(f) = f_elem {
            fresh.children.insert(0, XNode::Elem(f));
        }
        fresh.children.extend(ext);
        if rn == format!("c:{rtag}") {
            // Samma sort: bara cachen byts (referensens egna attribut och barn står kvar).
            let re = src.child_mut(rn).ok_or("referens saknas")?;
            let cache_name = format!("c:{ctag}");
            match re.child_mut(&cache_name) {
                Some(cache) => set_cache_children(cache, scope, &inner)?,
                None => {
                    let cache = elem(scope, &format!("<{c}:{ctag}>{inner}</{c}:{ctag}>"))?;
                    re.insert_before(cache, &["c:extLst"]);
                }
            }
        } else {
            src.children.retain(|x| !matches!(x, XNode::Elem(e) if e.name == rn));
            src.children.push(XNode::Elem(fresh));
        }
        return Ok(());
    }
    // Literaler.
    let inner = match nums {
        Some(ns) => format!("<{c}:numLit>{}</{c}:numLit>", num_cache_xml(c, "General", ns)),
        None => format!("<{c}:strLit>{}</{c}:strLit>", str_cache_xml(c, &texts)),
    };
    src.children.retain(|x| !matches!(x, XNode::Elem(e) if matches!(e.name.as_str(), "c:numLit" | "c:strLit")));
    src.children.push(XNode::Elem(elem(scope, &inner)?));
    Ok(())
}

/// `c:tx` med `c:rich` för en rubrik: första körningens `a:rPr` (och styckets `a:pPr`) behålls.
fn rich_tx(old: Option<&XElem>, scope: &[(String, String)], c: &str, a: &str, text: &str) -> Result<XElem, String> {
    let rich = old.and_then(|t| t.child("c:rich"));
    let body = rich.and_then(|r| r.child("a:bodyPr")).map(|b| b.to_xml()).unwrap_or_else(|| format!("<{a}:bodyPr/>"));
    let lst = rich.and_then(|r| r.child("a:lstStyle")).map(|b| b.to_xml()).unwrap_or_else(|| format!("<{a}:lstStyle/>"));
    let p0 = rich.and_then(|r| r.child("a:p"));
    let ppr = p0.and_then(|p| p.child("a:pPr")).map(|x| x.to_xml()).unwrap_or_default();
    let rpr = p0.and_then(|p| p.child("a:r")).and_then(|r| r.child("a:rPr")).map(|x| x.to_xml()).unwrap_or_default();
    let paras: String = text.split('\n').map(|line| format!("<{a}:p>{ppr}<{a}:r>{rpr}<{a}:t>{}</{a}:t></{a}:r></{a}:p>", escape(line))).collect();
    elem(scope, &format!("<{c}:tx><{c}:rich>{body}{lst}{paras}</{c}:rich></{c}:tx>"))
}

fn set_title_tx(title: &mut XElem, scope: &[(String, String)], c: &str, a: &str, text: &str) -> Result<(), String> {
    if title_text(title) == text {
        return Ok(());
    }
    let tx = rich_tx(title.child("c:tx"), scope, c, a, text)?;
    match title.index_of("c:tx") {
        Some(i) => title.children[i] = XNode::Elem(tx),
        None => {
            title.children.insert(0, XNode::Elem(tx));
        }
    }
    Ok(())
}

fn set_chart_title(chart: &mut XElem, scope: &[(String, String)], c: &str, a: &str, text: Option<&str>) -> Result<(), String> {
    let text = text.unwrap_or("");
    match chart.child_mut("c:title") {
        Some(t) if !text.is_empty() => set_title_tx(t, scope, c, a, text)?,
        Some(_) => {
            chart.remove_children("c:title");
            set_flag(chart, scope, c, "c:autoTitleDeleted", true)?;
        }
        None if !text.is_empty() => {
            let mut t = elem(scope, &format!("<{c}:title><{c}:overlay val=\"0\"/></{c}:title>"))?;
            set_title_tx(&mut t, scope, c, a, text)?;
            chart.children.insert(0, XNode::Elem(t));
            set_flag(chart, scope, c, "c:autoTitleDeleted", false)?;
        }
        None => {}
    }
    Ok(())
}

fn set_flag(parent: &mut XElem, scope: &[(String, String)], c: &str, name: &str, on: bool) -> Result<(), String> {
    let v = if on { "1" } else { "0" };
    match parent.child_mut(name) {
        Some(e) => e.set_attr("val", v),
        None => {
            let local = name.trim_start_matches("c:");
            let e = elem(scope, &format!("<{c}:{local} val=\"{v}\"/>"))?;
            parent.insert_before(e, &["c:pivotFmts", "c:view3D", "c:floor", "c:sideWall", "c:backWall", "c:plotArea"]);
        }
    }
    Ok(())
}

fn set_axis_title(ax: &mut XElem, scope: &[(String, String)], c: &str, a: &str, text: Option<&str>) -> Result<(), String> {
    let text = text.unwrap_or("");
    match ax.child_mut("c:title") {
        Some(t) if !text.is_empty() => set_title_tx(t, scope, c, a, text)?,
        Some(_) => ax.remove_children("c:title"),
        None if !text.is_empty() => {
            let mut t = elem(scope, &format!("<{c}:title><{c}:overlay val=\"0\"/></{c}:title>"))?;
            set_title_tx(&mut t, scope, c, a, text)?;
            ax.insert_before(t, &["c:numFmt", "c:majorTickMark", "c:minorTickMark", "c:tickLblPos", "c:spPr", "c:txPr", "c:crossAx"]);
        }
        None => {}
    }
    Ok(())
}

/// Bilderna vars diagram (eller andra delar de pekar på) ändrats mellan två versioner av paketet, så
/// att de renderas om (en ändring i diagramdelen ändrar inte bildens del).
pub fn slides_with_changed_parts(old: &Deck, new: &Deck) -> Vec<usize> {
    (0..new.slides.len())
        .filter(|&i| {
            let s = &new.slides[i];
            new.pkg.rels(&s.part).iter().filter(|r| !r.external && r.kind == "chart").any(|r| match (old.pkg.parts.get(&r.target), new.pkg.parts.get(&r.target)) {
                (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
                _ => false,
            })
        })
        .collect()
}
