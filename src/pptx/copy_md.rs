//! *Redigera en kopia*: presentationen som ett Markdown-dokument. En rubrik per bild (titel-
//! platshållaren, annars "Bild N"), punktlistor med nivåer ur textplatshållarna, vanliga stycken ur
//! textrutor, tabeller som Markdown-tabeller, bilder som inbäddade bilder (`bild:`-adresser, som
//! Word-importen) och anteckningarna som citat. Presentationen ändras aldrig.

use std::collections::HashMap;

use super::notes::{self, Para};
use super::xml::Node;
use super::{inherit, package, Deck};
use crate::docx::{escape_inline, escape_line_start, wrap, Image, IMAGE_SCHEME};
use crate::i18n::t;
use crate::tf;

/// Hur ett textstycke skrivs.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Plain,
    Bullet,
    Numbered,
}

struct Writer<'a> {
    deck: &'a Deck,
    out: String,
    images: Vec<(String, Image)>,
    /// Bilddel → namn i dokumentet (samma bild används bara en gång).
    names: HashMap<String, String>,
}

/// Hela presentationen som Markdown och bilderna som dokumentet refererar till.
pub fn deck_to_markdown(deck: &Deck) -> (String, Vec<(String, Image)>) {
    let mut w = Writer { deck, out: String::new(), images: Vec::new(), names: HashMap::new() };
    for i in 0..deck.slides.len() {
        w.slide(i);
    }
    (w.out.trim_end().to_owned() + "\n", w.images)
}

/// En rad inline-Markdown ur körningarna (fet/kursiv, specialtecken skyddade).
fn inline(p: &Para) -> String {
    let mut s = String::new();
    for r in &p.runs {
        let text = r.text.replace('\n', " ");
        let mut piece = escape_inline(&text);
        if r.bold {
            piece = wrap(&piece, "**");
        }
        if r.italic {
            piece = wrap(&piece, "*");
        }
        s.push_str(&piece);
    }
    s.trim().to_owned()
}

/// Styckets slag: platshållare för brödtext ger punkter (som i PowerPoint), textrutor vanliga
/// stycken; `buNone`/`buChar`/`buAutoNum` i stycket gäller före det.
fn kind_of(p: &Node, bulleted_box: bool) -> Kind {
    let ppr = p.child("a:pPr");
    if ppr.and_then(|x| x.child("a:buNone")).is_some() {
        return Kind::Plain;
    }
    if ppr.and_then(|x| x.child("a:buAutoNum")).is_some() {
        return Kind::Numbered;
    }
    if ppr.and_then(|x| x.child("a:buChar")).is_some() || bulleted_box {
        return Kind::Bullet;
    }
    Kind::Plain
}

impl Writer<'_> {
    fn block(&mut self, s: &str) {
        if s.trim().is_empty() {
            return;
        }
        self.out.push_str(s.trim_end());
        self.out.push_str("\n\n");
    }

    fn slide(&mut self, i: usize) {
        let deck = self.deck;
        let slide = &deck.slides[i];
        let no = deck.slide_number(i);
        let tree = slide.root.path(&["p:cSld", "p:spTree"]);
        let shapes = tree.map(notes::shapes).unwrap_or_default();
        // Rubriken: första titelplatshållaren med text.
        let mut title_shape = None;
        let mut title = String::new();
        for sp in &shapes {
            let is_title = inherit::placeholder(sp).is_some_and(|ph| matches!(ph.typ.as_str(), "title" | "ctrTitle"));
            if !is_title {
                continue;
            }
            let text = sp.child("p:txBody").map(|b| notes::paragraphs(b, no)).unwrap_or_default();
            let line: Vec<String> = text.iter().map(Para::text).map(|s| s.replace('\n', " ").trim().to_owned()).filter(|s| !s.is_empty()).collect();
            if !line.is_empty() {
                title = line.join(" ");
                title_shape = Some(*sp as *const Node);
                break;
            }
        }
        let heading = if title.is_empty() { tf!("Bild {n}", n = i + 1) } else { title };
        self.block(&format!("# {}", escape_inline(&heading)));
        for sp in shapes {
            if Some(sp as *const Node) == title_shape {
                continue;
            }
            match sp.name.as_str() {
                "p:sp" => self.text_shape(sp, no),
                "p:pic" => self.picture(sp, i),
                "p:graphicFrame" => self.frame(sp, i, no),
                _ => {}
            }
        }
        // Anteckningarna som citat.
        let paras: Vec<String> = notes::notes(deck, i).iter().map(inline).filter(|s| !s.is_empty()).collect();
        if !paras.is_empty() {
            let mut q = format!("> **{}**", escape_inline(t("Anteckningar")));
            for p in paras {
                q.push_str("\n>\n> ");
                q.push_str(&escape_line_start(&p));
            }
            self.block(&q);
        }
    }

    fn text_shape(&mut self, sp: &Node, no: i64) {
        let Some(body) = sp.child("p:txBody") else { return };
        let ph = inherit::placeholder(sp);
        // Sidfötter, datum och bildnummer hör till mallen, inte innehållet.
        if ph.as_ref().is_some_and(|p| matches!(p.typ.as_str(), "dt" | "ftr" | "sldNum" | "hdr")) {
            return;
        }
        let bulleted = ph.as_ref().is_some_and(|p| matches!(p.typ.as_str(), "body" | "obj"));
        let paras = notes::paragraphs(body, no);
        let mut s = String::new();
        let mut prev: Option<Kind> = None;
        let mut counters = [0usize; 9];
        for (p, node) in paras.iter().zip(body.children_named("a:p")) {
            let text = inline(p);
            if text.is_empty() {
                continue;
            }
            let kind = kind_of(node, bulleted);
            let lvl = p.level.min(8);
            match kind {
                Kind::Plain => {
                    if !s.is_empty() {
                        s.push_str("\n\n");
                    }
                    s.push_str(&escape_line_start(&text));
                    counters = [0; 9];
                }
                Kind::Bullet | Kind::Numbered => {
                    if !s.is_empty() {
                        s.push_str(if prev.is_some_and(|k| k != Kind::Plain) { "\n" } else { "\n\n" });
                    }
                    let indent = "    ".repeat(lvl);
                    let marker = if kind == Kind::Numbered {
                        counters[lvl] += 1;
                        counters.iter_mut().skip(lvl + 1).for_each(|c| *c = 0);
                        format!("{}. ", counters[lvl])
                    } else {
                        "- ".to_owned()
                    };
                    s.push_str(&format!("{indent}{marker}{}", escape_line_start(&text)));
                }
            }
            prev = Some(kind);
        }
        self.block(&s);
    }

    /// En bild ur paketet som inbäddad bild. EMF/WMF och okända format hoppas över.
    fn picture(&mut self, pic: &Node, i: usize) {
        let Some(id) = pic.find("a:blip").and_then(|b| b.attr("r:embed")) else { return };
        let rels = &self.deck.slides[i].rels;
        let Some(part) = package::rel(rels, id).map(|r| r.target.clone()) else { return };
        let alt = pic.path(&["p:nvPicPr", "p:cNvPr"]).and_then(|c| c.attr("descr")).unwrap_or("").replace(['\n', '\r'], " ");
        let Some(name) = self.image_name(&part) else { return };
        let alt = escape_inline(alt.trim());
        self.block(&format!("![{alt}]({IMAGE_SCHEME}{})", name.replace(' ', "%20")));
    }

    fn image_name(&mut self, part: &str) -> Option<String> {
        if let Some(n) = self.names.get(part) {
            return Some(n.clone());
        }
        let bytes = self.deck.pkg.read(part)?.to_vec();
        // Bara format som dokumentet kan visa (EMF/WMF och TIFF hoppas över).
        let ok = crate::imgfmt::is_svg(&bytes)
            || matches!(image::guess_format(&bytes), Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif | image::ImageFormat::Bmp | image::ImageFormat::WebP));
        if !ok {
            return None;
        }
        let image = crate::imgfmt::prepare(bytes)?;
        // Namnet får rastrets filändelse (SVG och WebP blir PNG).
        let ext = match image::guess_format(&image.bytes) {
            Ok(image::ImageFormat::Jpeg) => "jpeg",
            Ok(image::ImageFormat::Gif) => "gif",
            Ok(image::ImageFormat::Bmp) => "bmp",
            _ => "png",
        };
        let file = part.rsplit('/').next().unwrap_or(part);
        let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
        let base = if stem.is_empty() { format!("bild{}.{ext}", self.images.len() + 1) } else { format!("{stem}.{ext}") };
        let mut name = base.clone();
        let mut n = 2;
        while self.images.iter().any(|(k, _)| *k == name) {
            name = format!("{n}-{base}");
            n += 1;
        }
        self.images.push((name.clone(), image));
        self.names.insert(part.to_owned(), name.clone());
        Some(name)
    }

    /// Tabeller och SmartArt-text i en `p:graphicFrame`. Diagram hoppas över.
    fn frame(&mut self, gf: &Node, i: usize, no: i64) {
        if let Some(tbl) = gf.find("a:tbl") {
            self.block(&table(tbl, no));
            return;
        }
        // SmartArt: texten i datamodellen som punktlista.
        let Some(dm) = gf.find("dgm:relIds").and_then(|r| r.attr("r:dm")) else { return };
        let Some(part) = package::rel(&self.deck.slides[i].rels, dm).map(|r| r.target.clone()) else { return };
        let Some(data) = self.deck.pkg.xml(&part) else { return };
        let mut items = Vec::new();
        data.walk(&mut |n| {
            if n.name == "dgm:pt" && n.attr("type").is_none_or(|t| t == "node") {
                if let Some(b) = n.child("dgm:t") {
                    for p in notes::paragraphs(b, no) {
                        let s = inline(&p);
                        if !s.is_empty() {
                            items.push(format!("- {}", escape_line_start(&s)));
                        }
                    }
                }
            }
        });
        self.block(&items.join("\n"));
    }
}

/// En tabell som Markdown-tabell: första raden blir rubrikrad, sammanslagna celler blir tomma,
/// radbrytningar i en cell blir `<br>`.
fn table(tbl: &Node, no: i64) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for tr in tbl.children_named("a:tr") {
        let mut row = Vec::new();
        for tc in tr.children_named("a:tc") {
            let merged = tc.attr_bool("hMerge") == Some(true) || tc.attr_bool("vMerge") == Some(true);
            let text = if merged {
                String::new()
            } else {
                tc.child("a:txBody")
                    .map(|b| notes::paragraphs(b, no).iter().map(inline).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("<br>"))
                    .unwrap_or_default()
            };
            row.push(text.replace('|', "\\|"));
        }
        rows.push(row);
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let line = |r: &[String]| {
        let mut s = String::from("|");
        for c in 0..cols {
            let cell = r.get(c).map(String::as_str).unwrap_or("");
            s.push(' ');
            s.push_str(cell);
            s.push_str(" |");
        }
        s
    };
    let mut out = line(&rows[0]);
    out.push('\n');
    out.push('|');
    out.push_str(&" --- |".repeat(cols));
    for r in &rows[1..] {
        out.push('\n');
        out.push_str(&line(r));
    }
    out
}
