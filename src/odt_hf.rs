//! Sidhuvud och sidfot i OpenDocument: text med fält (`text:page-number` med talformat,
//! `text:page-count`), bilder (`draw:frame` i `style:header`), vänster|mitten|höger efter
//! styckets tabbstopp, udda/jämna sidor (`style:header-left`), första sidan (`style:header-first`) och
//! sidmallar per avsnitt. Resultatet är samma mall som i docx (se `crate::hf`).

use super::*;
use crate::hf::{HfStyle, NumFmt, SecHf};

/// Paketet och bilderna som hittats i sidhuvuden och sidfötter.
pub(super) struct HfCtx<'a, 'b> {
    pub pkg: &'a mut Pkg<'b>,
    pub images: &'a mut Vec<(String, Image)>,
    pub by_target: &'a mut HashMap<String, String>,
}

/// En läst del.
pub(super) struct HfSlot {
    pub text: String,
    pub align: Align,
    pub style: HfStyle,
}

/// Fältens märken; `text:page-number` följer `style:num-format`.
fn field_token(e: &Node) -> Option<String> {
    let fmt = || match e.attr("style:num-format").map(str::trim) {
        Some("i") => Some(NumFmt::LowerRoman),
        Some("I") => Some(NumFmt::UpperRoman),
        Some("a") => Some(NumFmt::LowerAlpha),
        Some("A") => Some(NumFmt::UpperAlpha),
        Some("1") => Some(NumFmt::Decimal),
        _ => None,
    };
    let with = |base: &str| match fmt() {
        Some(f) if f != NumFmt::Decimal => format!("{{{base}:{}}}", f.name()),
        _ => format!("{{{base}}}"),
    };
    Some(match e.name.as_str() {
        "text:page-number" => with("sida"),
        "text:page-count" => with("sidor"),
        "text:date" => "{datum}".to_owned(),
        "text:title" => "{titel}".to_owned(),
        "text:file-name" => "{fil}".to_owned(),
        _ => return None,
    })
}

/// Bilden i en `draw:frame` som mallens märke; läser in bilden första gången.
fn frame_token(e: &Node, ctx: &mut HfCtx<'_, '_>) -> Option<String> {
    let img = e.child("draw:image")?;
    let href = img.attr("xlink:href")?.trim().to_owned();
    if href.contains("://") || href.starts_with("..") || href.starts_with('/') {
        return None;
    }
    let target = href.trim_start_matches("./").to_owned();
    let name = match ctx.by_target.get(&target) {
        Some(n) => n.clone(),
        None => {
            let bytes = ctx.pkg.bytes(&target).ok().flatten().filter(|b| is_supported_image(b))?;
            let base: String = target
                .rsplit('/')
                .next()
                .unwrap_or("bild")
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
                .collect();
            let mut name = base.clone();
            let mut k = 2;
            while ctx.images.iter().any(|(m, _)| *m == name) {
                name = format!("{k}-{base}");
                k += 1;
            }
            let w = e.attr("svg:width").and_then(style::parse_length);
            let h = e.attr("svg:height").and_then(style::parse_length);
            let extent = w.zip(h).map(|(w, h)| ((w * 12700.0) as i64, (h * 12700.0) as i64));
            ctx.images.push((name.clone(), Image { bytes, extent }));
            ctx.by_target.insert(target, name.clone());
            name
        }
    };
    let w = e.attr("svg:width").and_then(style::parse_length).unwrap_or(0.0);
    let h = e.attr("svg:height").and_then(style::parse_length).unwrap_or(0.0);
    // En ram förankrad vid sidan har sitt läge från sidans hörn; övriga ligger på raden.
    let at = (e.attr("text:anchor-type") == Some("page"))
        .then(|| e.attr("svg:x").and_then(style::parse_length).zip(e.attr("svg:y").and_then(style::parse_length)))
        .flatten();
    Some(crate::hf::ImgTok { src: format!("{IMAGE_SCHEME}{name}"), w, h, at }.to_token())
}

/// Text, fält och bilder i ett stycke; flikar delar texten i `segs` (den sista delen är den pågående).
fn collect(n: &Node, segs: &mut Vec<String>, ctx: &mut HfCtx<'_, '_>) {
    for k in &n.kids {
        match k {
            Kid::Text(t) => {
                let out = segs.last_mut().expect("minst en del");
                for c in t.chars() {
                    if c.is_whitespace() {
                        if !out.ends_with(' ') && !out.is_empty() {
                            out.push(' ');
                        }
                    } else {
                        out.push(c);
                    }
                }
            }
            Kid::El(e) => {
                if let Some(f) = field_token(e) {
                    segs.last_mut().expect("minst en del").push_str(&f);
                    continue;
                }
                match e.name.as_str() {
                    "draw:frame" => {
                        if let Some(t) = frame_token(e, ctx) {
                            segs.last_mut().expect("minst en del").push_str(&t);
                        }
                    }
                    "text:s" => {
                        let c = e.attr("text:c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(100);
                        segs.last_mut().expect("minst en del").push_str(&" ".repeat(c));
                    }
                    "text:tab" => segs.push(String::new()),
                    "text:line-break" => segs.last_mut().expect("minst en del").push(' '),
                    "text:note" | "office:annotation" => {}
                    _ => collect(e, segs, ctx),
                }
            }
        }
    }
}

impl Styles {
    /// Tabbstoppens typer för ett styckeformat (närmaste som anger några), tom om inga finns.
    fn tab_types(&self, name: Option<&str>) -> Vec<String> {
        name.map(|n| self.chain("paragraph", n).iter().rev().find_map(|d| d.tabs.clone()).unwrap_or_default()).unwrap_or_default()
    }

    /// Utseendet på ett sidhuvuds/sidfotsstycke: storlek, färg, fetstil och linjen mot brödtexten.
    fn hf_look(&self, name: Option<&str>, header: bool) -> HfStyle {
        let Some(n) = name else { return HfStyle::default() };
        let tp = self.tprops("paragraph", n);
        let base = self.tprops("paragraph", "Standard");
        let mut s = HfStyle::default();
        // Bara det som avviker från brödtexten.
        s.size = tp.size.filter(|v| base.size.is_none_or(|b| (b - v).abs() > 0.05));
        s.color = tp.color.filter(|c| *c != [0, 0, 0]);
        s.bold = tp.bold == Some(true);
        s.italic = tp.italic == Some(true);
        let border = self.chain("paragraph", n).iter().rev().find_map(|d| if header { d.border_bottom.clone() } else { d.border_top.clone() });
        if let Some(b) = border.filter(|b| !b.trim().starts_with("none")) {
            let mut w = 0.5;
            let mut c = [0, 0, 0];
            for part in b.split_whitespace() {
                if let Some(h) = part.strip_prefix('#') {
                    c = crate::hf::parse_hex(h).unwrap_or(c);
                } else if let Some(l) = style::parse_length(part) {
                    w = l;
                }
            }
            s.line = Some((c, w.max(0.25)));
        }
        s
    }
}

/// Mallen för en del (`style:header`, `style:footer`, `…-left`, `…-first`). Ett enda stycke med
/// flikar blir vänster|mitten|höger efter tabbstoppen; flera stycken slås ihop med mellanslag.
pub(super) fn slot(st: &Styles, n: &Node, header: bool, ctx: &mut HfCtx<'_, '_>) -> Option<HfSlot> {
    let mut parts: Vec<String> = Vec::new();
    let mut layout: Vec<Vec<String>> = Vec::new();
    let mut align = None;
    let mut style = HfStyle::default();
    for p in n.els().filter(|p| matches!(p.name.as_str(), "text:p" | "text:h")) {
        let mut segs = vec![String::new()];
        collect(p, &mut segs, ctx);
        let segs: Vec<String> = segs.iter().map(|s| s.trim().replace('|', "\\|")).collect();
        let s = segs.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        if s.is_empty() {
            continue;
        }
        let sname = p.attr("text:style-name");
        if align.is_none() {
            align = Some(st.align(sname));
            style = st.hf_look(sname, header);
        }
        parts.push(s);
        let tabs = st.tab_types(sname);
        layout.push(match segs.len() {
            // Bara ett högertabb: vänster och höger. Annars är standardens tabbstopp mitten och höger.
            2 if tabs.len() == 1 && tabs[0] == "right" => vec![segs[0].clone(), String::new(), segs[1].clone()],
            2 => vec![segs[0].clone(), segs[1].clone(), String::new()],
            _ => segs,
        });
    }
    if let [l] = layout.as_slice() {
        if l.len() == 3 {
            return Some(HfSlot { text: l.join("|"), align: Align::Left, style });
        }
    }
    (!parts.is_empty()).then(|| HfSlot { text: parts.join(" "), align: align.unwrap_or(Align::Left), style })
}

/// Ett enkelt värde med justering som uttrycklig mall (`||text`), för delar utan egen justering.
fn explicit(t: HfSlot) -> String {
    if style::split_parts(&t.text).is_some() {
        return t.text;
    }
    match t.align {
        Align::Center => format!("|{}|", t.text),
        Align::Right => format!("||{}", t.text),
        _ => t.text,
    }
}

/// Sidmallens (`Standard`) sidhuvud och sidfot som grundformat: varianter, utseende, avstånd och sidnummerformat.
pub(super) fn read_master_hf(st: &Styles, master: &Node, layout: Option<&Node>, d: &mut DocDefaults, ctx: &mut HfCtx<'_, '_>) {
    let mut get = |tag: &str, header: bool| master.child(tag).and_then(|n| slot(st, n, header, ctx));
    if let Some(s) = get("style:header", true) {
        d.header = Some(s.text);
        d.header_align = s.align;
        d.header_style = s.style;
    }
    if let Some(s) = get("style:footer", false) {
        d.footer = Some(s.text);
        d.footer_align = s.align;
        d.footer_style = s.style;
    }
    // Egen första sida (`style:header-first`/`style:footer-first`); en tom del blir en tom första sida.
    let (h1, f1) = (master.child("style:header-first"), master.child("style:footer-first"));
    if h1.is_some() || f1.is_some() {
        for (node, header) in [(h1, true), (f1, false)] {
            let found = node.and_then(|n| slot(st, n, header, ctx));
            let text = found.as_ref().map(|s| s.text.clone()).unwrap_or_default();
            let (slot_, align, have) = if header { (&mut d.header_first, &mut d.header_align, d.header.is_some()) } else { (&mut d.footer_first, &mut d.footer_align, d.footer.is_some()) };
            *slot_ = Some(text);
            if let Some(s) = found {
                if !have {
                    *align = s.align;
                }
            }
        }
    }
    // Udda och jämna sidor: `style:header-left` finns bara när sidorna skiljer sig åt.
    let (hl, fl) = (master.child("style:header-left"), master.child("style:footer-left"));
    if hl.is_some() || fl.is_some() {
        for (node, header) in [(hl, true), (fl, false)] {
            let text = node.and_then(|n| slot(st, n, header, ctx)).map(explicit).unwrap_or_default();
            if header {
                d.header_even = Some(text);
            } else {
                d.footer_even = Some(text);
            }
        }
    }
    let has = |o: &Option<String>| o.as_deref().is_some_and(|t| !t.is_empty());
    let mut m = d.margins();
    let (hh, ff) = (has(&d.header) || has(&d.header_first) || has(&d.header_even), has(&d.footer) || has(&d.footer_first) || has(&d.footer_even));
    if hh {
        d.header_dist = Some(m[0]);
    }
    if ff {
        d.footer_dist = Some(m[2]);
    }
    extend_margins(layout, hh, ff, &mut m);
    if hh || ff {
        d.page_margin = Some(m);
    }
}

/// Brödtexten börjar under sidhuvudet: marginalen (som i Word) blir sidans övre marginal (avståndet
/// till sidhuvudet) plus sidhuvudets minsta höjd och mellanrummet; likadant för sidfoten.
fn extend_margins(layout: Option<&Node>, header: bool, footer: bool, m: &mut [f32; 4]) {
    let extra = |tag: &str, gap_side: &str| -> f32 {
        let Some(p) = layout.and_then(|l| l.child(tag)).and_then(|h| h.child("style:header-footer-properties")) else { return 0.0 };
        let len = |k: &str| p.attr(k).and_then(style::parse_length).unwrap_or(0.0);
        len("svg:height").max(len("fo:min-height")) + len(gap_side)
    };
    if header {
        m[0] += extra("style:header-style", "fo:margin-bottom");
    }
    if footer {
        m[2] += extra("style:footer-style", "fo:margin-top");
    }
}

/// Som `read_master_hf` men för en sidmall som hör till ett avsnitt: lägger sidhuvudets och sidfotens höjd till marginalerna.
pub(super) fn add_hf_extent(st: &Styles, master: &Node, layout: &Node, m: &mut [f32; 4], ctx: &mut HfCtx<'_, '_>) {
    let mut present = |tags: [&str; 3], header: bool| {
        tags.iter().any(|t| master.child(t).and_then(|n| slot(st, n, header, ctx)).is_some_and(|s| !s.text.is_empty()))
    };
    let h = present(["style:header", "style:header-first", "style:header-left"], true);
    let f = present(["style:footer", "style:footer-first", "style:footer-left"], false);
    extend_margins(Some(layout), h, f, m);
}

/// Det en sidmall (för ett avsnitt) anger som skiljer sig från grundformatet `d`. En del som saknas
/// där grundformatet har en blir tom; lika delar ärvs.
pub(super) fn master_hf(st: &Styles, m: &Node, d: &DocDefaults, ctx: &mut HfCtx<'_, '_>) -> SecHf {
    let mut out = SecHf::default();
    let mut one = |tag: &str, header: bool, base: &Option<String>| -> Option<String> {
        let text = m.child(tag).and_then(|n| slot(st, n, header, ctx)).map(explicit);
        // Grundformatets värden är lagrade som mall + justering; jämför som uttryckliga mallar.
        let base_t = base.clone();
        match (text, base_t) {
            (Some(t), Some(b)) if t == b => None,
            (Some(t), _) => Some(t),
            (None, Some(b)) if !b.is_empty() => Some(String::new()),
            (None, _) => None,
        }
    };
    let base_h = d.header.clone().map(|t| explicit(HfSlot { text: t, align: d.header_align, style: HfStyle::default() }));
    let base_f = d.footer.clone().map(|t| explicit(HfSlot { text: t, align: d.footer_align, style: HfStyle::default() }));
    out.header = one("style:header", true, &base_h);
    out.footer = one("style:footer", false, &base_f);
    let first = m.child("style:header-first").is_some() || m.child("style:footer-first").is_some();
    if first {
        out.title_pg = Some(true);
        out.header_first = Some(m.child("style:header-first").and_then(|n| slot(st, n, true, ctx)).map(explicit).unwrap_or_default());
        out.footer_first = Some(m.child("style:footer-first").and_then(|n| slot(st, n, false, ctx)).map(explicit).unwrap_or_default());
    }
    if m.child("style:header-left").is_some() || m.child("style:footer-left").is_some() {
        out.header_even = Some(m.child("style:header-left").and_then(|n| slot(st, n, true, ctx)).map(explicit).unwrap_or_default());
        out.footer_even = Some(m.child("style:footer-left").and_then(|n| slot(st, n, false, ctx)).map(explicit).unwrap_or_default());
    }
    out
}

// ================================================================ skrivning

/// Fälten och bilderna i en rad som ODF-innehåll. Bilderna läggs i `media` (sökväg, data, typ).
pub(super) struct HfOut<'a> {
    pub resolve: &'a dyn Fn(&str) -> Option<Image>,
    pub media: Vec<(String, Vec<u8>, &'static str)>,
    by_src: HashMap<String, String>,
    n: usize,
}

/// Det styckena i sidmallarna behöver: bildhämtning, planen för avsnitten och vilken plats varje sidmall (`M1` …) har i den.
pub(super) struct HfEnv<'a> {
    pub out: HfOut<'a>,
    pub plan: crate::hf::Plan,
    pub master_idx: Vec<usize>,
}

impl<'a> HfEnv<'a> {
    /// `all` = dokumentets avsnitt, `masters` = de som får en egen sidmall.
    pub fn new(resolve: &'a dyn Fn(&str) -> Option<Image>, d: &DocDefaults, all: &[sections::Section], masters: &[sections::Section]) -> HfEnv<'a> {
        let master_idx = masters.iter().map(|m| all.iter().position(|s| s.start == m.start).unwrap_or(0)).collect();
        HfEnv { out: HfOut::new(resolve), plan: crate::hf::Plan::new(d, all), master_idx }
    }
}

impl<'a> HfOut<'a> {
    pub fn new(resolve: &'a dyn Fn(&str) -> Option<Image>) -> HfOut<'a> {
        HfOut { resolve, media: Vec::new(), by_src: HashMap::new(), n: 0 }
    }

    fn frame(&mut self, tok: &crate::hf::ImgTok) -> Option<String> {
        let img = (self.resolve)(&tok.src)?;
        let ext = image_ext(&img.bytes)?;
        let path = match self.by_src.get(&tok.src) {
            Some(p) => p.clone(),
            None => {
                self.n += 1;
                let p = format!("Pictures/kladd-hf-{}.{ext}", self.n);
                let mime = match ext {
                    "png" => "image/png",
                    "gif" => "image/gif",
                    "bmp" => "image/bmp",
                    _ => "image/jpeg",
                };
                self.media.push((p.clone(), img.bytes.clone(), mime));
                self.by_src.insert(tok.src.clone(), p.clone());
                p
            }
        };
        let (mut w, mut h) = (tok.w, tok.h);
        if w <= 0.0 || h <= 0.0 {
            let (nw, nh) = match img.extent {
                Some((cx, cy)) => (cx as f32 / 12700.0, cy as f32 / 12700.0),
                None => imagesize::blob_size(&img.bytes).map(|s| (s.width as f32 * 0.75, s.height as f32 * 0.75)).unwrap_or((48.0, 48.0)),
            };
            let k = if w > 0.0 { w / nw.max(1.0) } else if h > 0.0 { h / nh.max(1.0) } else { 1.0 };
            (w, h) = (nw * k, nh * k);
        }
        let anchor = match tok.at {
            Some((x, y)) => format!(" text:anchor-type=\"page\" svg:x=\"{}\" svg:y=\"{}\"", cm(x), cm(y)),
            None => " text:anchor-type=\"as-char\"".to_owned(),
        };
        Some(format!(
            "<draw:frame draw:name=\"Logga{}\"{anchor} svg:width=\"{}\" svg:height=\"{}\" draw:z-index=\"0\"><draw:image xlink:href=\"{path}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame>",
            self.n,
            cm(w),
            cm(h)
        ))
    }

    /// En del av en rad: text, fält (`text:page-number` med talformat …) och bilder.
    pub fn inline(&mut self, text: &str, f: &style::Fields) -> String {
        let mut inner = String::new();
        let mut rest = text;
        while let Some(i) = rest.find('{') {
            let Some(j) = rest[i..].find('}') else { break };
            let tok = &rest[i + 1..i + j];
            let (name, fmt_s) = match tok.split_once(':') {
                Some((n, f)) => (n, Some(f)),
                None => (tok, None),
            };
            let fmt = fmt_s.map(NumFmt::from_name);
            let num = |f: Option<NumFmt>| match f {
                Some(f) => format!(" style:num-format=\"{}\"", f.odf()),
                None => String::new(),
            };
            let x: Option<String> = match (name, fmt) {
                ("sida", None) => Some(format!("<text:page-number text:select-page=\"current\">{}</text:page-number>", f.fmt.format(f.page))),
                ("sida", Some(Some(fm))) => Some(format!("<text:page-number text:select-page=\"current\"{}>{}</text:page-number>", num(Some(fm)), fm.format(f.page))),
                ("sidor", None) => Some(format!("<text:page-count>{}</text:page-count>", f.pages)),
                ("sidor", Some(Some(fm))) => Some(format!("<text:page-count{}>{}</text:page-count>", num(Some(fm)), fm.format(f.pages))),
                ("avsnittssidor", None) => Some(format!("<text:page-count>{}</text:page-count>", f.sec_pages)),
                ("datum", None) => Some(format!("<text:date text:date-value=\"{d}\">{d}</text:date>", d = f.date)),
                ("titel", None) => Some(format!("<text:title>{}</text:title>", esc(&f.title))),
                ("fil", None) => Some(format!("<text:file-name text:display=\"name-and-extension\">{}</text:file-name>", esc(&f.file))),
                ("bild", _) => crate::hf::ImgTok::parse(tok).and_then(|t| self.frame(&t)),
                _ => None,
            };
            match x {
                Some(x) => {
                    inner.push_str(&hf_text(&rest[..i]));
                    inner.push_str(&x);
                }
                None => inner.push_str(&hf_text(&rest[..i + j + 1])),
            }
            rest = &rest[i + j + 1..];
        }
        inner.push_str(&hf_text(rest));
        inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const PNG: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0xd, 0xa, 0x1a, 0xa, 0x0, 0x0, 0x0, 0xd, 0x49, 0x48, 0x44, 0x52, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0x1, 0x8, 0x2, 0x0, 0x0, 0x0, 0x90, 0x77, 0x53, 0xde, 0x0,
        0x0, 0x0, 0xc, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x0, 0x0, 0x3, 0x1, 0x1, 0x0, 0xc9, 0xfe, 0x92, 0xef, 0x0, 0x0, 0x0, 0x0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
        0x60, 0x82,
    ];

    const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" office:version="1.3""#;

    fn odt(master: &str) -> Vec<u8> {
        let content = format!(r#"<?xml version="1.0"?><office:document-content {NS}><office:body><office:text><text:p text:style-name="Standard">Text</text:p></office:text></office:body></office:document-content>"#);
        let styles = format!(
            r##"<?xml version="1.0"?><office:document-styles {NS}><office:styles><style:style style:name="Standard" style:family="paragraph"/><style:style style:name="Header" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties><style:tab-stops><style:tab-stop style:position="15cm" style:type="right"/></style:tab-stops></style:paragraph-properties></style:style><style:style style:name="Hdr" style:family="paragraph" style:parent-style-name="Header"><style:paragraph-properties fo:border-bottom="0.74pt solid #156082"/><style:text-properties fo:font-size="9pt" fo:color="#595959"/></style:style></office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" style:num-format="i" fo:margin-top="1.27cm" fo:margin-bottom="1.27cm" fo:margin-left="2cm" fo:margin-right="2cm"/><style:header-style><style:header-footer-properties fo:min-height="1cm" fo:margin-bottom="0.2cm"/></style:header-style></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1">{master}</style:master-page></office:master-styles></office:document-styles>"##
        );
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            z.start_file("mimetype", o).unwrap();
            z.write_all(MIME.as_bytes()).unwrap();
            for (n, b) in [("content.xml", content.as_bytes()), ("styles.xml", styles.as_bytes()), ("Pictures/logo.png", &PNG[..])] {
                z.start_file(n, o).unwrap();
                z.write_all(b).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    fn frame() -> &'static str {
        r#"<draw:frame text:anchor-type="as-char" svg:width="1cm" svg:height="1cm"><draw:image xlink:href="Pictures/logo.png"><text:p/></draw:image></draw:frame>"#
    }

    fn head(md: &str) -> DocDefaults {
        DocDefaults::from_comment(md.lines().next().unwrap()).expect("grundformat")
    }

    #[test]
    fn logo_left_and_text_right_with_right_tab_and_even_pages() {
        let m = format!(
            r#"<style:header><text:p text:style-name="Hdr">{}<text:tab/>Skolrapport</text:p></style:header><style:header-left><text:p text:style-name="Hdr">Skolrapport<text:tab/>{}</text:p></style:header-left><style:footer><text:p text:style-name="Standard">Sida <text:page-number style:num-format="I">3</text:page-number> av <text:page-count>5</text:page-count></text:p></style:footer><style:header-first><text:p text:style-name="Header"/></style:header-first>"#,
            frame(),
            frame()
        );
        let l = crate::odt::read(&odt(&m)).unwrap();
        let d = head(&l.markdown);
        assert_eq!(d.header.as_deref(), Some("{bild:logo.png 28.3x28.3}||Skolrapport"), "{}", l.markdown);
        assert_eq!(d.header_even.as_deref(), Some("Skolrapport||{bild:logo.png 28.3x28.3}"));
        assert_eq!(d.footer.as_deref(), Some("Sida {sida:ROMAN} av {sidor}"));
        assert_eq!(d.header_first.as_deref(), Some(""));
        assert_eq!(d.page_format, Some(crate::hf::NumFmt::LowerRoman));
        assert_eq!(l.images.len(), 1);
        // Utseende och avstånd: texten 9 pt grå med linje under; marginalen = avstånd + höjd + mellanrum.
        assert_eq!(d.header_style.size, Some(9.0));
        assert_eq!(d.header_style.color, Some([0x59, 0x59, 0x59]));
        assert!(d.header_style.line.is_some());
        assert!((d.header_dist.unwrap() - 36.0).abs() < 0.1);
        assert!((d.margins()[0] - (36.0 + 28.35 + 5.67)).abs() < 0.2, "{:?}", d.margins());
    }

    #[test]
    fn written_odt_has_logo_page_formats_and_even_header_and_reads_back() {
        let md = "<!-- kladd: header: \"{bild:logo.png 40x20}|Titel|\"; footer: \"Sida {sida:roman} av {sidor}\"; header-even: \"Jämn\"; header-style: \"9pt #666666 line=#999999/0.5pt\"; page-margin: 30mm 20mm; page-format: roman -->\n\nText\n";
        let resolve = |s: &str| (s == "bild:logo.png").then(|| Image { bytes: PNG.to_vec(), extent: None });
        let out = crate::odt::write(md, "t", &resolve).unwrap().bytes;
        let mut z = zip::ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut styles = String::new();
        std::io::Read::read_to_string(&mut z.by_name("styles.xml").unwrap(), &mut styles).unwrap();
        assert!(styles.contains("<draw:frame") && styles.contains("Pictures/kladd-hf-1.png"), "{styles}");
        assert!(styles.contains("<style:header-left>") && styles.contains("style:num-format=\"i\""), "{styles}");
        assert!(styles.contains("fo:border-bottom=\"0.5pt solid #999999\""), "{styles}");
        let mut manifest = String::new();
        std::io::Read::read_to_string(&mut z.by_name("META-INF/manifest.xml").unwrap(), &mut manifest).unwrap();
        assert!(manifest.contains("Pictures/kladd-hf-1.png"), "{manifest}");
        assert!(z.by_name("Pictures/kladd-hf-1.png").is_ok());
        let back = crate::odt::read(&out).unwrap();
        let d = head(&back.markdown);
        assert_eq!(d.header.as_deref(), Some("{bild:kladd-hf-1.png 40x20}|Titel|"), "{}", back.markdown);
        assert_eq!(d.footer.as_deref(), Some("Sida {sida:roman} av {sidor}"));
        assert_eq!(d.header_even.as_deref(), Some("Jämn"));
        assert_eq!(d.page_format, Some(crate::hf::NumFmt::LowerRoman));
        // Marginalen överlever en sparning.
        assert!((d.margins()[0] - 85.04).abs() < 0.3, "{:?}", d.margins());
    }
}

