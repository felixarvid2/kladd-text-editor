//! Läsning av sidhuvuden och sidfötter ur en docx: text med fält (`{sida}` …, sidnummerformat),
//! bilder (inline och förankrade, med relationen i `header*.xml.rels`), tabbstopp, tabeller utan
//! ramar (logga | titel | datum), textens storlek och färg och linjen under sidhuvudet.
//! Resultatet är en mall som `crate::hf` ritar; se `docs/dev/dokumentlage.md`.

use super::*;
use crate::hf::{HfStyle, NumFmt, SecHf};

/// Bilder som hittats i sidhuvuden och sidfötter: namn → bild, och vilket namn en filplats fått.
#[derive(Default)]
pub(super) struct HfImages {
    pub list: Vec<(String, Image)>,
    pub by_target: HashMap<String, String>,
}

impl HfImages {
    /// Namnet bilden på `target` (sökvägen i paketet) fått; läser in den första gången.
    fn name_of(&mut self, pkg: &mut Package<'_>, target: &str, extent: Option<(i64, i64)>) -> Option<String> {
        if let Some(n) = self.by_target.get(target) {
            return Some(n.clone());
        }
        let bytes = pkg.part_bytes(target).ok().flatten().filter(|b| is_supported_image(b))?;
        let base: String = target
            .rsplit('/')
            .next()
            .unwrap_or("bild")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
            .collect();
        let mut name = base.clone();
        let mut n = 2;
        while self.list.iter().any(|(m, _)| *m == name) {
            name = format!("{n}-{base}");
            n += 1;
        }
        self.list.push((name.clone(), Image { bytes, extent }));
        self.by_target.insert(target.to_owned(), name.clone());
        Some(name)
    }
}

/// Det en `w:sectPr` säger om sidhuvud och sidfot.
#[derive(Default)]
pub(super) struct SectHfRead {
    /// Mallarna avsnittet själv anger (`None` = ingen referens: ärv).
    pub own: SecHf,
    pub header_dist: Option<f32>,
    pub footer_dist: Option<f32>,
    pub header_style: HfStyle,
    pub footer_style: HfStyle,
    /// Justering för ett enkelt värde utan `|`.
    pub header_align: Align,
    pub footer_align: Align,
}

/// Sidgeometrin som förankrade bilder räknas mot (pt).
#[derive(Clone, Copy)]
pub(super) struct PageGeo {
    pub size: (f32, f32),
    pub margin: [f32; 4],
}

/// `w:evenAndOddHeaders` i settings.xml.
pub(super) fn even_and_odd(settings: &str) -> bool {
    let mut r = Reader::from_str(settings);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if local(e.name().as_ref()) == b"evenAndOddHeaders" => return on_off(&e),
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

/// Läser sidhuvuden och sidfötter för en `w:sectPr`. `first` = första avsnittet (där en saknad
/// första sida blir tom), `even_odd` = dokumentet har olika udda/jämna sidor.
#[allow(clippy::too_many_arguments)]
pub(super) fn read_sect(
    sect: &str,
    first: bool,
    even_odd: bool,
    geo: PageGeo,
    pkg: &mut Package<'_>,
    rels: &HashMap<String, (String, String, bool)>,
    dir: &str,
    images: &mut HfImages,
) -> Result<SectHfRead, String> {
    let mut out = SectHfRead::default();
    // (huvud?, typ) → relations-id.
    let mut refs: Vec<(bool, String, String)> = Vec::new();
    let mut title_pg = false;
    let mut r = Reader::from_str(sect);
    let tw = |e: &BytesStart<'_>, n: &[u8]| attr(e, n).and_then(|v| v.parse::<f32>().ok());
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match local(e.name().as_ref()) {
                b"headerReference" | b"footerReference" => {
                    if let (Some(ty), Some(id)) = (attr(&e, b"type"), attr(&e, b"id")) {
                        refs.push((local(e.name().as_ref()) == b"headerReference", ty, id));
                    }
                }
                b"titlePg" => title_pg = on_off(&e),
                b"pgMar" => {
                    out.header_dist = tw(&e, b"header").map(|v| (v / 20.0).max(0.0));
                    out.footer_dist = tw(&e, b"footer").map(|v| (v / 20.0).max(0.0));
                }
                b"pgNumType" => {
                    out.own.page_start = attr(&e, b"start").and_then(|v| v.parse().ok());
                    out.own.page_fmt = attr(&e, b"fmt").and_then(|v| NumFmt::from_docx(&v));
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out.own.title_pg = (title_pg || first).then_some(title_pg);
    for header in [true, false] {
        let dist = if header { out.header_dist } else { out.footer_dist }.unwrap_or(36.0);
        for ty in ["default", "first", "even"] {
            let id = refs.iter().find(|(h, t, _)| *h == header && t == ty).map(|(_, _, id)| id.as_str());
            // Första sidan och jämna sidor finns bara om sektionen eller dokumentet säger det.
            let wanted = match ty {
                "first" => title_pg,
                "even" => even_odd,
                _ => true,
            };
            let mut found: Option<HfRead> = None;
            if let Some((target, _, false)) = id.and_then(|i| rels.get(i)).filter(|_| wanted) {
                let part = resolve_target(dir, target);
                if let Some(xml) = pkg.part(&part)? {
                    found = read_part(&xml, &part, header, geo, dist, pkg, images, rels, dir);
                }
            }
            let slot: Option<String> = match (&found, id.is_some() && wanted) {
                (Some(f), _) => Some(f.template.clone()),
                // Referensen finns men delen är tom eller oläsbar: ingen text.
                (None, true) => Some(String::new()),
                // En saknad första sida eller jämn sida i första avsnittet är tom.
                (None, false) if first && wanted && ty != "default" => Some(String::new()),
                _ => None,
            };
            if let Some(f) = &found {
                if ty == "default" || (out_style_empty(&out, header) && slot.is_some()) {
                    if header {
                        out.header_style = f.style.clone();
                        out.header_align = f.align;
                    } else {
                        out.footer_style = f.style.clone();
                        out.footer_align = f.align;
                    }
                }
            }
            let target = match (header, ty) {
                (true, "default") => &mut out.own.header,
                (true, "first") => &mut out.own.header_first,
                (true, _) => &mut out.own.header_even,
                (false, "default") => &mut out.own.footer,
                (false, "first") => &mut out.own.footer_first,
                (false, _) => &mut out.own.footer_even,
            };
            *target = slot;
        }
    }
    Ok(out)
}

fn out_style_empty(o: &SectHfRead, header: bool) -> bool {
    if header { o.header_style.is_default() } else { o.footer_style.is_default() }
}

/// Mallen för en del, med justering och utseende.
pub(super) struct HfRead {
    pub template: String,
    pub align: Align,
    pub style: HfStyle,
}

/// Fältkoden som mallens märke: `PAGE \* roman` → `{sida:roman}`.
fn field_template(instr: &str) -> Option<String> {
    let name = instr.split_whitespace().next()?.to_ascii_uppercase();
    let fmt = NumFmt::from_switch(instr);
    let with = |base: &str| match fmt {
        Some(f) => format!("{{{base}:{}}}", f.name()),
        None => format!("{{{base}}}"),
    };
    Some(match name.as_str() {
        "PAGE" => with("sida"),
        "NUMPAGES" => with("sidor"),
        "SECTIONPAGES" => "{avsnittssidor}".to_owned(),
        "DATE" | "TIME" | "SAVEDATE" | "PRINTDATE" => "{datum}".to_owned(),
        "TITLE" => "{titel}".to_owned(),
        "FILENAME" => "{fil}".to_owned(),
        _ => return None,
    })
}

/// En rad: delarna efter tabbstopp eller tabellceller, och styckets justering.
struct Line {
    parts: Vec<String>,
    /// Bara högertabb: två delar betyder vänster och höger.
    right_only: bool,
    align: Option<Align>,
}

/// Tolkar en header/footer-del (`w:hdr`/`w:ftr`). `None` om den inte har någon text eller bild.
fn read_part(
    xml: &str,
    part: &str,
    header: bool,
    geo: PageGeo,
    dist: f32,
    pkg: &mut Package<'_>,
    images: &mut HfImages,
    main_rels: &HashMap<String, (String, String, bool)>,
    main_dir: &str,
) -> Option<HfRead> {
    let (pdir, pfile) = part.rsplit_once('/').unwrap_or(("", part));
    let rels_name = if pdir.is_empty() { format!("_rels/{pfile}.rels") } else { format!("{pdir}/_rels/{pfile}.rels") };
    let prels = pkg.part(&rels_name).ok().flatten().map(|x| parse_rels(&x)).unwrap_or_default();

    let mut r = Reader::from_str(xml);
    let mut lines: Vec<Line> = Vec::new();
    // Stycket vi är i.
    let mut in_p = false;
    let mut in_ppr = false;
    let mut in_rpr = false;
    let mut in_pbdr = false;
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut tab_stops: Vec<String> = Vec::new();
    let mut jc: Option<Align> = None;
    let mut in_t = false;
    let mut in_instr = false;
    // Komplexa fält: (kod, i resultatet, ersättning).
    let mut fields: Vec<(String, bool, Option<String>)> = Vec::new();
    let mut simples: Vec<bool> = Vec::new();
    // Tabeller: bara ytterst räknas.
    let mut tbl_depth = 0usize;
    let mut cells: Vec<String> = Vec::new();
    let mut cell_align: Vec<Option<Align>> = Vec::new();
    let mut in_cell_depth = 0usize;
    // Hoppar över (textrutor, `mc:Fallback`).
    let mut skip = 0usize;
    // Bilden som läses.
    let mut drawing: Option<Drawing> = None;
    // Utseendet: första stycket med innehåll.
    let mut style = HfStyle::default();
    let mut style_done = false;
    let mut p_style = HfStyle::default();
    let mut any_content = false;

    // Lägger en bild som märke i raden.
    let push_image = |d: Drawing, cur: &mut String, pkg: &mut Package<'_>, images: &mut HfImages| {
        let Some((target, from_main)) = d.blip.as_deref().map(|t| t.split_once('\u{1}').map_or((t, false), |(t, _)| (t, true))) else { return };
        let full = resolve_target(if from_main { main_dir } else { pdir }, target);
        let Some(name) = images.name_of(pkg, &full, d.extent) else { return };
        let (w, h) = match d.extent {
            Some((cx, cy)) => (cx as f32 / 12700.0, cy as f32 / 12700.0),
            None => (0.0, 0.0),
        };
        let at = d.anchored.then(|| anchored_pos(&d.pos, w, h, geo, dist, header));
        let tok = crate::hf::ImgTok { src: format!("{IMAGE_SCHEME}{name}"), w, h, at };
        cur.push_str(&tok.to_token());
    };

    loop {
        let Ok(ev) = r.read_event() else { return None };
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(ev, Event::Empty(_));
                let name = local(e.name().as_ref()).to_vec();
                if skip > 0 {
                    if !empty {
                        skip += 1;
                    }
                    continue;
                }
                // Inuti en bild.
                if let Some(d) = drawing.as_mut() {
                    if !empty {
                        d.depth += 1;
                    }
                    match name.as_slice() {
                        b"blip" | b"imagedata" => {
                            if let Some(id) = attr(e, b"embed").or_else(|| attr(e, b"id")) {
                                // Relationen ligger i delens egen .rels, eller (docx4j) i dokumentets.
                                let hit = prels.get(&id).map(|r| (r, false)).or_else(|| main_rels.get(&id).map(|r| (r, true)));
                                if let Some(((t, _, ext), main)) = hit {
                                    if *ext {
                                        d.linked = true;
                                    } else if d.blip.is_none() {
                                        d.blip = Some(if main { format!("{t}\u{1}") } else { t.clone() });
                                    }
                                }
                            }
                        }
                        b"extent" if d.extent.is_none() => {
                            let n = |k: &[u8]| attr(e, k).and_then(|v| v.parse::<i64>().ok());
                            if let (Some(cx), Some(cy)) = (n(b"cx"), n(b"cy")) {
                                if cx > 0 && cy > 0 {
                                    d.extent = Some((cx, cy));
                                }
                            }
                        }
                        b"txbxContent" => {
                            if !empty {
                                d.depth -= 1;
                                skip = 1;
                            }
                        }
                        b"anchor" => {
                            d.anchored = true;
                            if matches!(attr(e, b"simplePos").as_deref(), Some("1" | "true")) {
                                d.pos.simple = Some((0, 0));
                            }
                        }
                        b"simplePos" if d.pos.simple.is_some() => {
                            let n = |k: &[u8]| attr(e, k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                            d.pos.simple = Some((n(b"x"), n(b"y")));
                        }
                        b"shape" if d.extent.is_none() => {
                            if let Some(st) = attr(e, b"style") {
                                vml_style(&st, d);
                            }
                        }
                        b"positionH" => {
                            d.pos.rel = attr(e, b"relativeFrom").unwrap_or_default();
                            d.in_pos = if empty { 0 } else { 1 };
                        }
                        b"positionV" => {
                            d.pos.rel_v = attr(e, b"relativeFrom").unwrap_or_default();
                            d.in_pos = if empty { 0 } else { 2 };
                        }
                        b"align" if d.in_pos != 0 && !empty => d.text_of = 1,
                        b"posOffset" if d.in_pos != 0 && !empty => d.text_of = 2,
                        _ => {}
                    }
                    continue;
                }
                match name.as_slice() {
                    // Alternativ som Word bara visar om det första inte går: bara det första.
                    b"Fallback" | b"txbxContent" => {
                        if !empty {
                            skip = 1;
                        }
                    }
                    b"tbl" => {
                        tbl_depth += 1;
                    }
                    b"tr" if tbl_depth == 1 => {
                        cells.clear();
                        cell_align.clear();
                    }
                    b"tc" if tbl_depth == 1 && !empty => {
                        in_cell_depth += 1;
                        cells.push(String::new());
                        cell_align.push(None);
                    }
                    b"p" if !empty => {
                        in_p = true;
                        cur.clear();
                        segs.clear();
                        tab_stops.clear();
                        jc = None;
                        p_style = HfStyle::default();
                                    }
                    b"pPr" => in_ppr = !empty,
                    b"rPr" => in_rpr = !empty,
                    b"pBdr" if in_ppr => in_pbdr = !empty,
                    // Linjen mot brödtexten: under sidhuvudet, över sidfoten.
                    b"bottom" | b"top" if in_pbdr => {
                        let want: &[u8] = if header { b"bottom" } else { b"top" };
                        if name == want && !matches!(attr(e, b"val").as_deref(), Some("nil" | "none")) {
                            let w = attr(e, b"sz").and_then(|v| v.parse::<f32>().ok()).unwrap_or(4.0) / 8.0;
                            let c = attr(e, b"color").and_then(|c| crate::hf::parse_hex(&c)).unwrap_or([0, 0, 0]);
                            p_style.line = Some((c, w.max(0.25)));
                        }
                    }
                    b"jc" if in_ppr && !in_rpr => jc = attr(e, b"val").map(|v| Align::from_jc(&v)),
                    b"sz" if in_rpr && p_style.size.is_none() => {
                        p_style.size = attr(e, b"val").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 2.0).filter(|v| *v > 0.0);
                    }
                    b"color" if in_rpr && p_style.color.is_none() => {
                        p_style.color = attr(e, b"val").and_then(|c| crate::hf::parse_hex(&c));
                    }
                    b"b" if in_rpr => p_style.bold = on_off(e),
                    b"i" if in_rpr => p_style.italic = on_off(e),
                    b"t" => in_t = !empty,
                    b"instrText" => in_instr = !empty,
                    b"tab" if in_p && !in_ppr && !in_rpr => segs.push(std::mem::take(&mut cur)),
                    b"ptab" if in_p => segs.push(std::mem::take(&mut cur)),
                    b"tab" if in_ppr => {
                        if let Some(v) = attr(e, b"val").filter(|v| v != "clear") {
                            tab_stops.push(v);
                        }
                    }
                    b"br" | b"cr" if in_p && !in_ppr && attr(e, b"type").as_deref() != Some("page") => cur.push(' '),
                    b"fldChar" => match attr(e, b"fldCharType").as_deref() {
                        Some("begin") => fields.push((String::new(), false, None)),
                        Some("separate") => {
                            if let Some(f) = fields.last_mut() {
                                f.1 = true;
                                f.2 = field_template(&f.0);
                                if let Some(ph) = &f.2 {
                                    cur.push_str(ph);
                                }
                            }
                        }
                        Some("end") => {
                            fields.pop();
                        }
                        _ => {}
                    },
                    b"fldSimple" => {
                        let ph = attr(e, b"instr").and_then(|i| field_template(&i));
                        if let Some(ph) = &ph {
                            cur.push_str(ph);
                        }
                        if !empty {
                            simples.push(ph.is_some());
                        }
                    }
                    b"drawing" | b"pict" if in_p && !empty => drawing = Some(Drawing { depth: 1, ..Default::default() }),
                    _ => {}
                }
            }
            Event::End(e) => {
                let name = local(e.name().as_ref()).to_vec();
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                if let Some(d) = drawing.as_mut() {
                    d.depth -= 1;
                    match name.as_slice() {
                        b"positionH" | b"positionV" => d.in_pos = 0,
                        b"align" | b"posOffset" => d.text_of = 0,
                        _ => {}
                    }
                    if d.depth > 0 {
                        continue;
                    }
                    let d = drawing.take().unwrap_or_default();
                    if !fields.iter().any(|f| !f.1) {
                        push_image(d, &mut cur, pkg, images);
                    }
                    continue;
                }
                match name.as_slice() {
                    b"pPr" => in_ppr = false,
                    b"rPr" => in_rpr = false,
                    b"pBdr" => in_pbdr = false,
                    b"t" => in_t = false,
                    b"instrText" => in_instr = false,
                    b"fldSimple" => {
                        simples.pop();
                    }
                    b"tc" if tbl_depth == 1 => in_cell_depth = in_cell_depth.saturating_sub(1),
                    b"tr" if tbl_depth == 1 => {
                        let parts: Vec<String> = cells.iter().map(|c| c.trim().replace('|', "\\|")).collect();
                        if parts.iter().any(|p| !p.is_empty()) {
                            any_content = true;
                            lines.push(Line { parts: table_zones(parts), right_only: false, align: cell_align.iter().flatten().next().copied() });
                        }
                    }
                    b"tbl" => tbl_depth = tbl_depth.saturating_sub(1),
                    b"p" if in_p => {
                        in_p = false;
                        segs.push(std::mem::take(&mut cur));
                        let parts: Vec<String> = segs.iter().map(|s| s.trim().replace('|', "\\|")).collect();
                        let text = parts.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
                        // Utseendet kommer från första stycket med text (inte bara en bild).
                        let has_text = crate::hf::pieces(&text).iter().any(|p| matches!(p, crate::hf::Piece::Text(t) if !t.trim().is_empty()));
                        if has_text && !style_done {
                            style = p_style.clone();
                            style_done = true;
                        }
                        if tbl_depth == 1 && in_cell_depth > 0 {
                            // Cellens stycken sätts ihop med mellanslag.
                            if let Some(c) = cells.last_mut() {
                                if !text.is_empty() {
                                    if !c.is_empty() {
                                        c.push(' ');
                                    }
                                    c.push_str(&text.replace("\\|", "|"));
                                }
                            }
                            if let Some(a) = cell_align.last_mut() {
                                if a.is_none() && !text.is_empty() {
                                    *a = jc;
                                }
                            }
                        } else if tbl_depth == 0 && !text.is_empty() {
                            any_content = true;
                            let right_only = tab_stops.len() == 1 && tab_stops[0] == "right";
                            lines.push(Line { parts, right_only, align: jc });
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) => {
                if let Some(d) = drawing.as_mut().filter(|d| d.text_of != 0) {
                    let t = text_of(&ev);
                    let v = d.in_pos == 2;
                    match (d.text_of, v) {
                        (1, false) => d.pos.align = Some(t.trim().to_ascii_lowercase()),
                        (1, true) => d.pos.align_v = Some(t.trim().to_ascii_lowercase()),
                        (_, false) => d.pos.off = t.trim().parse().ok(),
                        (_, true) => d.pos.off_v = t.trim().parse().ok(),
                    }
                    continue;
                }
                if skip > 0 {
                    continue;
                }
                let t = text_of(&ev);
                if in_instr {
                    if let Some(f) = fields.last_mut() {
                        f.0.push_str(&t);
                    }
                } else if in_t && in_p {
                    let hidden = fields.iter().any(|f| !f.1) || fields.iter().any(|f| f.1 && f.2.is_some()) || simples.iter().any(|&x| x);
                    if !hidden {
                        cur.push_str(&t.replace(['\n', '\r'], " "));
                    }
                }
            }
            _ => {}
        }
    }
    if !any_content {
        return None;
    }
    // Delarna: flera stycken eller rader slås ihop per del; en rad med en enda del följer den
    // första radens justering (som ett enda värde med `-align`).
    let first_idx = match lines.first().and_then(|l| l.align) {
        Some(Align::Center) => 1,
        Some(Align::Right) => 2,
        _ => 0,
    };
    let multi_line = lines.len() > 1;
    let mut zones: [Vec<String>; 3] = Default::default();
    for l in &lines {
        for (z, t) in zones.iter_mut().zip(line_zones(l, multi_line.then_some(first_idx))) {
            if !t.is_empty() {
                z.push(t);
            }
        }
    }
    let parts: [String; 3] = zones.map(|z| z.join(" "));
    let filled: Vec<usize> = (0..3).filter(|&i| !parts[i].is_empty()).collect();
    let tabbed = lines.len() == 1 && lines[0].parts.len() >= 2;
    let (template, align) = match filled.as_slice() {
        [] => return None,
        _ if tabbed => (parts.join("|"), Align::Left),
        [i] => (parts[*i].clone(), [Align::Left, Align::Center, Align::Right][*i]),
        _ => (parts.join("|"), Align::Left),
    };
    Some(HfRead { template, align, style })
}

/// Cellerna i en tabellrad som vänster, mitten, höger.
fn table_zones(p: Vec<String>) -> Vec<String> {
    match p.len() {
        0 | 1 => p,
        2 => vec![p[0].clone(), String::new(), p[1].clone()],
        n => {
            let mid = p[1..n - 1].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
            vec![p[0].clone(), mid, p[n - 1].clone()]
        }
    }
}

/// Vänster, mitten, höger för en rad. `single` = delen som en rad med bara en del ska hamna i.
fn line_zones(l: &Line, single: Option<usize>) -> [String; 3] {
    let p = &l.parts;
    match p.len() {
        0 => Default::default(),
        1 => {
            let i = single.unwrap_or(match l.align {
                Some(Align::Center) => 1,
                Some(Align::Right) => 2,
                _ => 0,
            });
            let mut z: [String; 3] = Default::default();
            z[i] = p[0].clone();
            z
        }
        2 if l.right_only => [p[0].clone(), String::new(), p[1].clone()],
        2 => [p[0].clone(), p[1].clone(), String::new()],
        3 => [p[0].clone(), p[1].clone(), p[2].clone()],
        n => [p[0].clone(), p[1..n - 1].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" "), p[n - 1].clone()],
    }
}

/// Övre vänstra hörnet (pt från sidans hörn) för en förankrad bild, via `crate::word::anchor`.
fn anchored_pos(a: &AnchorPos, w: f32, h: f32, geo: PageGeo, dist: f32, header: bool) -> (f32, f32) {
    let page = DocDefaults { page_size: Some(geo.size), page_margin: Some(geo.margin), ..Default::default() };
    let an = anchor_of(a, &page);
    let px = |pt: f32| pt / 0.75;
    let (pw, ph) = geo.size;
    let m = geo.margin;
    let ctx = crate::word::anchor::PageCtx {
        page_w: px(pw),
        page_h: px(ph),
        margin: m.map(px),
        col_left: px(m[3]),
        col_w: px((pw - m[1] - m[3]).max(1.0)),
    };
    let x = (ctx.col_left + an.x_in_column(px(w), &ctx)) * 0.75;
    let y = match an.y_at(px(h), &ctx) {
        // Stycket börjar vid sidhuvudets (sidfotens) kant; en sidfots stycke ligger ovanför kanten.
        crate::word::anchor::VAt::Para(d) => (if header { dist } else { ph - dist - 12.0 }) + d * 0.75,
        crate::word::anchor::VAt::Page(y) => y * 0.75,
    };
    ((x * 10.0).round() / 10.0, (y * 10.0).round() / 10.0)
}

// ================================================================ skrivning

const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PIC_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";

/// Ett skrivet sidhuvud eller en skriven sidfot: XML, bildrelationer (id, mål i `word/media`) och bilddata.
pub(super) struct HfWritten {
    pub xml: String,
    pub rels: Vec<(String, String)>,
    pub media: Vec<(String, Vec<u8>)>,
}

/// Körningar för en text: fälten (`{sida}` …) blir Word-fält med värdena i `f` som förhandsvisning
/// och `{bild:…}` blir bilder. `rpr` är textens utseende (`w:rPr`).
fn runs_for(text: &str, f: &style::Fields, rpr: &str, imgs: &mut ImgOut<'_>) -> String {
    let mut runs = String::new();
    let mut rest = text;
    let plain = |t: &str, runs: &mut String| {
        if !t.is_empty() {
            let mut x = String::new();
            xml_escape(t, &mut x);
            runs.push_str(&format!("<w:r>{rpr}<w:t xml:space=\"preserve\">{x}</w:t></w:r>"));
        }
    };
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        let inner = &rest[i + 1..i + j];
        let (name, fmt_s) = match inner.split_once(':') {
            Some((n, f)) => (n, Some(f)),
            None => (inner, None),
        };
        let fmt = fmt_s.map(NumFmt::from_name);
        let field = |code: &str, shown: String, f: Option<NumFmt>| {
            let instr = match f.and_then(NumFmt::switch) {
                Some(sw) => format!("{code} {sw}"),
                None if f == Some(NumFmt::Decimal) => format!("{code} \\* Arabic"),
                None => code.to_owned(),
            };
            let mut x = String::new();
            xml_escape(&shown, &mut x);
            format!(
                "<w:r>{rpr}<w:fldChar w:fldCharType=\"begin\"/></w:r><w:r>{rpr}<w:instrText xml:space=\"preserve\"> {instr} </w:instrText></w:r><w:r>{rpr}<w:fldChar w:fldCharType=\"separate\"/></w:r><w:r>{rpr}<w:t>{x}</w:t></w:r><w:r>{rpr}<w:fldChar w:fldCharType=\"end\"/></w:r>"
            )
        };
        let rendered: Option<String> = match (name, fmt) {
            ("sida", None) => Some(field("PAGE", f.page.to_string(), None)),
            ("sida", Some(Some(fm))) => Some(field("PAGE", fm.format(f.page), Some(fm))),
            ("sidor", None) => Some(field("NUMPAGES", f.pages.to_string(), None)),
            ("sidor", Some(Some(fm))) => Some(field("NUMPAGES", fm.format(f.pages), Some(fm))),
            ("avsnittssidor", None) => Some(field("SECTIONPAGES", f.sec_pages.to_string(), None)),
            ("datum", None) => Some(field("DATE \\@ \"yyyy-MM-dd\"", f.date.clone(), None)),
            ("titel", None) => Some(field("TITLE", f.title.clone(), None)),
            ("fil", None) => Some(field("FILENAME", f.file.clone(), None)),
            _ if name == "bild" => crate::hf::ImgTok::parse(inner).and_then(|tok| imgs.drawing(&tok, rpr)),
            _ => None,
        };
        // Ett okänt märke är vanlig text.
        match rendered {
            Some(r) => {
                plain(&rest[..i], &mut runs);
                runs.push_str(&r);
            }
            None => plain(&rest[..i + j + 1], &mut runs),
        }
        rest = &rest[i + j + 1..];
    }
    plain(rest, &mut runs);
    runs
}

/// Bilderna i en del som skrivs: relationer och bilddata.
pub(super) struct ImgOut<'a> {
    resolve: &'a dyn Fn(&str) -> Option<Image>,
    taken: &'a mut std::collections::HashSet<String>,
    rels: Vec<(String, String)>,
    media: Vec<(String, Vec<u8>)>,
    by_src: HashMap<String, String>,
    next_id: u32,
    page: (f32, f32),
}

impl ImgOut<'_> {
    /// `w:drawing` (inline eller förankrad) för en bild; `None` om bilden saknas.
    fn drawing(&mut self, tok: &crate::hf::ImgTok, rpr: &str) -> Option<String> {
        let img = (self.resolve)(&tok.src)?;
        let ext = image_ext(&img.bytes)?;
        let rid = match self.by_src.get(&tok.src) {
            Some(r) => r.clone(),
            None => {
                let mut n = 1;
                let file = loop {
                    let f = format!("kladd-hf-{n}.{ext}");
                    if self.taken.insert(format!("word/media/{f}").to_lowercase()) {
                        break f;
                    }
                    n += 1;
                };
                let rid = format!("rId{}", self.rels.len() + 1);
                self.rels.push((rid.clone(), file.clone()));
                self.media.push((file, img.bytes.clone()));
                self.by_src.insert(tok.src.clone(), rid.clone());
                rid
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
        let (cx, cy) = ((w * 12700.0) as i64, (h * 12700.0) as i64);
        self.next_id += 1;
        let id = self.next_id;
        let graphic = format!(
            "<a:graphic xmlns:a=\"{A_NS}\"><a:graphicData uri=\"{PIC_NS}\"><pic:pic xmlns:pic=\"{PIC_NS}\"><pic:nvPicPr><pic:cNvPr id=\"{id}\" name=\"Bild {id}\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>"
        );
        let common = format!("<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>");
        let tail = format!("<wp:docPr id=\"{id}\" name=\"Bild {id}\"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a=\"{A_NS}\" noChangeAspect=\"1\"/></wp:cNvGraphicFramePr>{graphic}");
        let _ = self.page;
        Some(match tok.at {
            None => format!("<w:r>{rpr}<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">{common}{tail}</wp:inline></w:drawing></w:r>"),
            Some((x, y)) => format!(
                "<w:r>{rpr}<w:drawing><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"251658240\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionV>{common}<wp:wrapNone/>{tail}</wp:anchor></w:drawing></w:r>",
                (x * 12700.0) as i64,
                (y * 12700.0) as i64
            ),
        })
    }
}

/// `w:rPr` för mallens utseende (tom om inget avviker).
fn rpr_of(st: &HfStyle) -> String {
    let mut x = String::new();
    if st.bold {
        x.push_str("<w:b/>");
    }
    if st.italic {
        x.push_str("<w:i/>");
    }
    if let Some(c) = st.color {
        x.push_str(&format!("<w:color w:val=\"{:02X}{:02X}{:02X}\"/>", c[0], c[1], c[2]));
    }
    if let Some(s) = st.size {
        let hp = (s * 2.0).round() as i64;
        x.push_str(&format!("<w:sz w:val=\"{hp}\"/><w:szCs w:val=\"{hp}\"/>"));
    }
    if x.is_empty() { x } else { format!("<w:rPr>{x}</w:rPr>") }
}

/// Ett sidhuvud (`hdr`) eller en sidfot (`ftr`) med ett stycke. Fälten blir Word-fält och bilderna
/// `w:drawing` (relationerna i delens egen .rels). En mall med `|` blir vänster, mitten och höger
/// med flikar (mitt- och högertabb över textbredden `text_w` i twips). `st.line` ger en ramlinje
/// (`w:pBdr`) under sidhuvudet och över sidfoten.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_part(
    root: &str,
    template: &str,
    align: Align,
    st: &HfStyle,
    text_w: i64,
    f: &style::Fields,
    resolve: &dyn Fn(&str) -> Option<Image>,
    taken: &mut std::collections::HashSet<String>,
    page: (f32, f32),
) -> HfWritten {
    let mut imgs = ImgOut { resolve, taken, rels: Vec::new(), media: Vec::new(), by_src: HashMap::new(), next_id: 0, page };
    let rpr = rpr_of(st);
    let mut ppr = String::new();
    if let Some((c, w)) = st.line {
        let side = if root == "hdr" { "bottom" } else { "top" };
        let sz = ((w * 8.0).round() as i64).clamp(2, 96);
        ppr.push_str(&format!("<w:pBdr><w:{side} w:val=\"single\" w:sz=\"{sz}\" w:space=\"4\" w:color=\"{:02X}{:02X}{:02X}\"/></w:pBdr>", c[0], c[1], c[2]));
    }
    let runs = if style::split_parts(template).is_some() {
        let p = style::parts_of(template, align);
        // Delarna efter `parts_of` har `\|` utskrivet som `|`; fälten läses vidare i `runs_for`.
        let tab = format!("<w:r>{rpr}<w:tab/></w:r>");
        ppr.push_str(&format!("<w:tabs><w:tab w:val=\"center\" w:pos=\"{}\"/><w:tab w:val=\"right\" w:pos=\"{text_w}\"/></w:tabs>", text_w / 2));
        let a = runs_for(&p[0], f, &rpr, &mut imgs);
        let b = runs_for(&p[1], f, &rpr, &mut imgs);
        let c = runs_for(&p[2], f, &rpr, &mut imgs);
        format!("{a}{tab}{b}{tab}{c}")
    } else {
        let [l, c, r] = style::parts_of(template, align);
        let text = [l, c, r].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
        if align != Align::Left {
            ppr.push_str(&format!("<w:jc w:val=\"{}\"/>", align.as_jc()));
        }
        runs_for(&text, f, &rpr, &mut imgs)
    };
    let ppr = if ppr.is_empty() { ppr } else { format!("<w:pPr>{ppr}</w:pPr>") };
    let xml = format!(
        "{XML_DECL}\n<w:{root} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" xmlns:wp=\"{WP_NS}\"><w:p>{ppr}{runs}</w:p></w:{root}>"
    );
    HfWritten { xml, rels: imgs.rels, media: imgs.media }
}

/// Relationsdelen för ett sidhuvud med bilder.
pub(super) fn rels_xml(rels: &[(String, String)]) -> String {
    let mut x = format!("{XML_DECL}\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for (id, file) in rels {
        x.push_str(&format!("<Relationship Id=\"{id}\" Type=\"{REL_NS}/image\" Target=\"media/{file}\"/>"));
    }
    x.push_str("</Relationships>");
    x
}

/// Lägger `<w:evenAndOddHeaders/>` i (eller tar bort det ur) settings.xml.
pub(super) fn with_even_odd(xml: &str, on: bool) -> String {
    let has = xml.contains("<w:evenAndOddHeaders");
    if on == has {
        return xml.to_owned();
    }
    if !on {
        return remove_tag(xml, "<w:evenAndOddHeaders");
    }
    // Schemats ordning: efter `defaultTableStyle` m.fl., före `characterSpacingControl` och `footnotePr`.
    let at = [
        "<w:bookFoldRevPrinting",
        "<w:bookFoldPrinting",
        "<w:bookFoldPrintingSheets",
        "<w:drawingGridHorizontalSpacing",
        "<w:drawingGridVerticalSpacing",
        "<w:displayHorizontalDrawingGridEvery",
        "<w:displayVerticalDrawingGridEvery",
        "<w:doNotUseMarginsForDrawingGridOrigin",
        "<w:drawingGridHorizontalOrigin",
        "<w:drawingGridVerticalOrigin",
        "<w:doNotShadeFormData",
        "<w:noPunctuationKerning",
        "<w:characterSpacingControl",
        "<w:printTwoOnOne",
        "<w:strictFirstAndLastChars",
        "<w:noLineBreaksAfter",
        "<w:noLineBreaksBefore",
        "<w:savePreviewPicture",
        "<w:footnotePr",
        "<w:endnotePr",
        "<w:compat",
        "<w:docVars",
        "<w:rsids",
        "<m:mathPr",
        "<w:themeFontLang",
        "<w:clrSchemeMapping",
        "</w:settings>",
    ]
    .iter()
    .filter_map(|m| xml.find(m))
    .min();
    match at {
        Some(at) => format!("{}<w:evenAndOddHeaders/>{}", &xml[..at], &xml[at..]),
        None => xml.to_owned(),
    }
}

/// Sidhuvud och sidfot som `defaults` (första avsnittet) anger, som ett avsnitts egna värden.
pub(super) fn own_of_defaults(d: &DocDefaults) -> SecHf {
    SecHf {
        header: d.header.clone(),
        footer: d.footer.clone(),
        header_first: d.header_first.clone(),
        footer_first: d.footer_first.clone(),
        header_even: d.header_even.clone(),
        footer_even: d.footer_even.clone(),
        title_pg: Some(d.different_first()),
        page_start: d.page_start,
        page_fmt: d.page_format,
    }
}

#[cfg(test)]
#[path = "docx_hf_tests.rs"]
mod tests;
