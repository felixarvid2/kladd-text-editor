//! Den gemensamma textmotorn: formning (rustybuzz), radbrytning, radavstånd, punkter och
//! placering av glyfer. Allt text i presentationer går hit (former, tabeller, SmartArt, diagram),
//! och resultatet är färdigplacerade glyfer i en `Scene`, så att skärm och PDF blir identiska.
//!
//! Indata är redan upplöst (arvet görs i `inherit.rs`/`render.rs`): stycken med körningar och
//! färdiga stilar i punkter. `normAutofit fontScale` ska redan vara tillämpad på storlekarna;
//! `lnSpcReduction` tillämpas här på procentuella radavstånd.

use std::sync::Arc;

use super::color::Rgba;
use super::fonts::{self, FontBook, FontFace, Resolved};
use super::scene::{Affine, GlyphRun, Item, Paint, Path, PlacedGlyph, Stroke};

/// Versaler.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Caps {
    #[default]
    None,
    All,
    Small,
}

/// En körnings färdiga stil.
#[derive(Clone, Debug)]
pub struct RunStyle {
    pub latin: String,
    pub ea: String,
    pub cs: String,
    /// `a:sym`: typsnitt för symboltecken (används inte än).
    #[allow(dead_code)]
    pub sym: Option<String>,
    /// Storlek i punkter (med `fontScale`).
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: Rgba,
    pub outline: Option<Stroke>,
    pub underline: bool,
    pub strike: bool,
    /// Upphöjt/nedsänkt som andel av storleken (0.3 = 30 % upp).
    pub baseline: f32,
    /// Extra teckenavstånd i punkter.
    pub spacing: f32,
    pub caps: Caps,
    pub highlight: Option<Rgba>,
}

impl Default for RunStyle {
    fn default() -> Self {
        RunStyle {
            latin: "Calibri".into(),
            ea: String::new(),
            cs: String::new(),
            sym: None,
            size: 18.0,
            bold: false,
            italic: false,
            color: Rgba::BLACK,
            outline: None,
            underline: false,
            strike: false,
            baseline: 0.0,
            spacing: 0.0,
            caps: Caps::None,
            highlight: None,
        }
    }
}

/// En körning: text (med `\t` för tabb) eller en radbrytning (`a:br`).
#[derive(Clone, Debug)]
pub struct Run {
    pub text: String,
    pub style: RunStyle,
    pub line_break: bool,
}

/// Radavstånd eller avstånd före/efter: procent av en rad (1.0 = 100 %) eller punkter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    Pct(f32),
    Pts(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
    Distributed,
}

/// En punkt eller ett nummer framför stycket.
#[derive(Clone, Debug)]
pub struct Bullet {
    pub text: String,
    pub style: RunStyle,
}

/// Ett stycke.
#[derive(Clone, Debug)]
pub struct Para {
    pub runs: Vec<Run>,
    pub align: Align,
    pub mar_l: f32,
    pub mar_r: f32,
    pub indent: f32,
    pub spc_bef: Spacing,
    pub spc_aft: Spacing,
    pub ln_spc: Spacing,
    pub bullet: Option<Bullet>,
    pub rtl: bool,
    pub def_tab: f32,
    pub tabs: Vec<f32>,
    /// Stilen för styckets slut (`a:endParaRPr`): ger höjden för tomma stycken.
    pub end_style: RunStyle,
}

impl Default for Para {
    fn default() -> Self {
        Para {
            runs: Vec::new(),
            align: Align::Left,
            mar_l: 0.0,
            mar_r: 0.0,
            indent: 0.0,
            spc_bef: Spacing::Pts(0.0),
            spc_aft: Spacing::Pts(0.0),
            ln_spc: Spacing::Pct(1.0),
            bullet: None,
            rtl: false,
            def_tab: 72.0,
            tabs: Vec::new(),
            end_style: RunStyle::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Anchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// En textruta: stycken och `bodyPr`.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub paras: Vec<Para>,
    /// Inre marginaler (vänster, topp, höger, botten) i punkter.
    pub insets: (f32, f32, f32, f32),
    pub anchor: Anchor,
    /// `anchorCtr`: centrera textblocket vågrätt.
    pub anchor_ctr: bool,
    pub wrap: bool,
    /// `lnSpcReduction` som andel (0.1 = 10 %).
    pub ln_spc_reduction: f32,
    /// Antal spalter och avståndet mellan dem.
    pub columns: (u32, f32),
}

/// Måtten som styr radhöjden (kalibrerade mot PowerPoints PDF-export, se `docs/dev/pptx.md`).
///
/// Uppmätt i PowerPoints PDF (Calibri, `cppcon18-say-nothing` bild 18): en rad vid 100 % är
/// `LINE_FACTOR` · storlek hög och delas mellan upp- och nedstapel i förhållandet winAscent:winDescent.
/// Vid lägre procent (t.ex. 90 %) tas skillnaden bort från uppstapeln, nedstapeln är oförändrad;
/// mellanrummet mellan två baslinjer = föregående rads nedstapel + radens (minskade) uppstapel.
/// `spcBef` gäller inte första stycket i rutan.
pub mod metrics {
    pub const LINE_FACTOR: f32 = 1.2;
}

/// Ett formningssegment: samma typsnitt och stil.
struct Seg {
    face: Arc<FontFace>,
    res: Resolved,
    style: RunStyle,
    text: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Normal,
    Space,
    Tab,
    Break,
}

/// Ett kluster: minsta odelbara enhet vid radbrytning.
struct Cl {
    seg: usize,
    range: (u32, u32),
    glyphs: Vec<(u16, f32, f32, f32)>,
    width: f32,
    kind: Kind,
    /// Får raden brytas efter klustret?
    break_after: bool,
    rtl: bool,
}

struct Line {
    from: usize,
    to: usize,
    start_x: f32,
    avail: f32,
    width: f32,
    /// Uppstapel och största teckenstorlek (för senare bruk, t.ex. `fontAlgn`).
    #[allow(dead_code)]
    ascent: f32,
    descent: f32,
    #[allow(dead_code)]
    size: f32,
    single: f32,
    hard_end: bool,
    first: bool,
}

fn break_after_char(c: char, next: Option<char>) -> bool {
    if c == ' ' || c == '\u{3000}' || c == '-' || c == '\u{2010}' || c == '/' && next.is_some_and(|n| n.is_alphanumeric()) {
        return true;
    }
    fonts::is_cjk(c) || next.is_some_and(fonts::is_cjk)
}

/// Riktningsklass: 2 = höger-till-vänster-bokstav, 1 = bokstav eller siffra (vänster-till-höger), 0 = neutral.
fn char_dir(c: char) -> u8 {
    if matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF) {
        2
    } else if c.is_alphanumeric() {
        1
    } else {
        0
    }
}

/// Gör om texten efter versalinställningen.
fn apply_caps(s: &str, caps: Caps) -> String {
    match caps {
        Caps::None => s.to_owned(),
        Caps::All | Caps::Small => s.to_uppercase(),
    }
}

/// Teckenkategori: 0 latin, 1 östasiatiskt, 2 komplex skrift.
fn script_class(c: char) -> u8 {
    if fonts::is_cjk(c) {
        1
    } else if fonts::is_complex(c) {
        2
    } else {
        0
    }
}

/// Delar en körning i segment efter skrift och teckentäckning och formar dem.
fn shape_run(run: &Run, book: &FontBook, segs: &mut Vec<Seg>, cls: &mut Vec<Cl>, tab_stops_on: bool) {
    let style = &run.style;
    let size = if style.caps == Caps::Small { style.size * 0.8 } else { style.size };
    if run.line_break {
        let res = book.resolve(&style.latin, style.bold, style.italic);
        segs.push(Seg { face: res.face.clone(), res, style: style.clone(), text: String::new() });
        cls.push(Cl { seg: segs.len() - 1, range: (0, 0), glyphs: Vec::new(), width: 0.0, kind: Kind::Break, break_after: true, rtl: false });
        return;
    }
    let text = apply_caps(&run.text, style.caps);
    if text.is_empty() {
        return;
    }
    // Typsnitt per kategori.
    let fam = |cls: u8| -> &str {
        match cls {
            1 if !style.ea.is_empty() => &style.ea,
            2 if !style.cs.is_empty() => &style.cs,
            _ => &style.latin,
        }
    };
    let mut pieces: Vec<(Resolved, Arc<FontFace>, String)> = Vec::new();
    let mut cur: Option<(Resolved, Arc<FontFace>, String)> = None;
    let chars: Vec<char> = text.chars().collect();
    for (i, &c0) in chars.iter().enumerate() {
        let _ = i;
        let class = script_class(c0);
        let res = book.resolve(fam(class), style.bold, style.italic);
        let c = match res.symbol_map {
            Some(sym) => fonts::symbol_to_unicode(sym, c0),
            None => c0,
        };
        let mut face = res.face.clone();
        let mut c_out = c;
        if !c.is_whitespace() && !c.is_control() {
            let has = face.face().is_some_and(|f| f.glyph_index(c).is_some());
            if !has {
                // Symboltypsnitt som finns installerat: tecknet kan ligga i U+F0xx.
                let sym = char::from_u32(0xF000 + (c as u32 & 0xFF));
                if face.symbol_cmap && (c as u32) < 0x100 && sym.is_some_and(|s| face.face().is_some_and(|f| f.glyph_index(s).is_some())) {
                    c_out = sym.unwrap_or(c);
                } else if let Some(fb) = book.fallback(c, &face) {
                    face = fb;
                }
            }
        }
        // Byt bit när skriftriktningen byter (höger-till-vänster-bokstäver mot latinska bokstäver och
        // siffror); neutrala tecken (blanksteg, skiljetecken) följer med biten de står i.
        let dir = char_dir(c_out);
        let same_dir = |s: &str| match dir {
            0 => true,
            d => s.chars().rev().map(char_dir).find(|&x| x != 0).is_none_or(|x| x == d),
        };
        match &mut cur {
            Some((r, f, s)) if Arc::ptr_eq(f, &face) && r.x_scale == res.x_scale && r.fake_bold == res.fake_bold && r.fake_italic == res.fake_italic && same_dir(s) => {
                s.push(c_out);
            }
            _ => {
                if let Some(p) = cur.take() {
                    pieces.push(p);
                }
                cur = Some((res, face, c_out.to_string()));
            }
        }
    }
    if let Some(p) = cur.take() {
        pieces.push(p);
    }
    for (res, face, piece) in pieces {
        let seg_idx = segs.len();
        let mut st = style.clone();
        st.size = size;
        let spacing = st.spacing;
        segs.push(Seg { face: face.clone(), res: res.clone(), style: st, text: piece.clone() });
        let Some(rb) = face.shaper() else { continue };
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(&piece);
        buf.guess_segment_properties();
        // Riktningen bestäms av bitens starka tecken (siffror i hebreisk text formas vänster-till-höger).
        let rtl = piece.chars().any(|c| char_dir(c) == 2);
        buf.set_direction(if rtl { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
        let out = rustybuzz::shape(&rb, &[], buf);
        let scale = size / face.upm * res.x_scale;
        let infos = out.glyph_infos();
        let pos = out.glyph_positions();
        // Glyfer per kluster (logisk ordning).
        let mut order: Vec<usize> = (0..infos.len()).collect();
        order.sort_by_key(|&i| (infos[i].cluster, if rtl { usize::MAX - i } else { i }));
        let mut groups: Vec<(u32, Vec<usize>)> = Vec::new();
        for i in order {
            let c = infos[i].cluster;
            match groups.last_mut() {
                Some((gc, v)) if *gc == c => v.push(i),
                _ => groups.push((c, vec![i])),
            }
        }
        let n = groups.len();
        for gi in 0..n {
            let start = groups[gi].0;
            let end = if gi + 1 < n { groups[gi + 1].0 } else { piece.len() as u32 };
            let s = &piece[start as usize..(end as usize).max(start as usize).min(piece.len())];
            let first = s.chars().next().unwrap_or(' ');
            let last = s.chars().last().unwrap_or(' ');
            let next = piece[(end as usize).min(piece.len())..].chars().next();
            let kind = if first == '\t' {
                Kind::Tab
            } else if first == ' ' || first == '\u{3000}' {
                Kind::Space
            } else {
                Kind::Normal
            };
            let mut glyphs = Vec::new();
            let mut width = 0.0;
            // I höger-till-vänster-text är glyferna inom ett kluster i visuell ordning omvänt.
            let idxs = &groups[gi].1;
            for &i in idxs {
                let adv = pos[i].x_advance as f32 * scale;
                glyphs.push((infos[i].glyph_id as u16, adv, pos[i].x_offset as f32 * scale, pos[i].y_offset as f32 * size / face.upm));
                width += adv;
            }
            if kind == Kind::Tab {
                glyphs.clear();
                width = 0.0;
            } else {
                width += spacing;
            }
            let _ = tab_stops_on;
            cls.push(Cl {
                seg: seg_idx,
                range: (start, end),
                glyphs,
                width,
                kind,
                break_after: break_after_char(last, next) || kind == Kind::Tab,
                rtl,
            });
        }
    }
}

/// Radhöjd vid 100 % och typsnittets uppstapel för en storlek.
fn face_metrics(face: &FontFace, size: f32) -> (f32, f32, f32) {
    let total = (face.win_ascent + face.win_descent).max(0.1);
    let single = size * metrics::LINE_FACTOR;
    let asc = single * face.win_ascent / total;
    (asc, single - asc, single)
}

/// Höjden av ett avstånd före/efter (procent räknas på styckets första rad).
fn spacing_pts(s: Spacing, size: f32, single: f32) -> f32 {
    let _ = size;
    match s {
        Spacing::Pts(p) => p,
        Spacing::Pct(p) => p * single,
    }
}

/// Resultatet av en utläggning.
pub struct Laid {
    /// Utritningen i textrutans koordinater (origo i rutans övre vänstra hörn, före insättningar).
    pub items: Vec<Item>,
    /// Textens höjd (utan insättningar).
    pub height: f32,
}

/// Lägger ut en textruta med storleken `w`×`h` (punkter).
pub fn layout(frame: &Frame, w: f32, h: f32, book: &FontBook) -> Laid {
    let (il, it, ir, ib) = frame.insets;
    let ncol = frame.columns.0.max(1) as usize;
    let col_gap = frame.columns.1.max(0.0);
    let inner_w = (w - il - ir).max(1.0);
    let col_w = if ncol > 1 { ((inner_w - col_gap * (ncol - 1) as f32) / ncol as f32).max(1.0) } else { inner_w };
    let wrap_w = if frame.wrap { col_w } else { f32::INFINITY };

    // 1. Forma och bryt varje stycke.
    struct PLaid {
        segs: Vec<Seg>,
        cls: Vec<Cl>,
        lines: Vec<Line>,
        bullet: Option<(Seg, Vec<(u16, f32, f32, f32)>, f32)>,
        spc_bef: f32,
        spc_aft: f32,
        line_heights: Vec<f32>,
        asc_parts: Vec<f32>,
    }
    let mut plaid: Vec<PLaid> = Vec::new();
    for (pi, p) in frame.paras.iter().enumerate() {
        let mut segs = Vec::new();
        let mut cls = Vec::new();
        for r in &p.runs {
            shape_run(r, book, &mut segs, &mut cls, true);
        }
        // Punkten (bara om stycket har text).
        let has_text = p.runs.iter().any(|r| !r.line_break && !r.text.is_empty());
        let bullet = p.bullet.as_ref().filter(|_| has_text).and_then(|b| {
            let mut bs = Vec::new();
            let mut bc = Vec::new();
            shape_run(&Run { text: b.text.clone(), style: b.style.clone(), line_break: false }, book, &mut bs, &mut bc, false);
            let seg = bs.into_iter().next()?;
            // (id, x, y, framflyttning) som `emit_glyphs` vill ha.
            let mut gx = 0.0;
            let glyphs: Vec<_> = bc
                .iter()
                .flat_map(|c| c.glyphs.iter().copied())
                .map(|(id, adv, xo, yo)| {
                    let g = (id, gx + xo, -yo, adv);
                    gx += adv;
                    g
                })
                .collect();
            let w: f32 = bc.iter().map(|c| c.width).sum();
            Some((seg, glyphs, w))
        });
        // Radbrytning.
        let first_x = (p.mar_l + p.indent).max(0.0);
        let text_x = match &bullet {
            Some((_, _, bw)) => {
                let after = first_x + bw;
                if p.mar_l >= after - 0.01 && p.indent < 0.0 {
                    p.mar_l
                } else {
                    // Punkten är bredare än det hängande indraget: nästa tabbstopp.
                    let tab = p.def_tab.max(1.0);
                    if p.indent < 0.0 { (after / tab).ceil() * tab } else { after }
                }
            }
            None => first_x,
        };
        let mut lines: Vec<Line> = Vec::new();
        let mut i = 0usize;
        let n = cls.len();
        let mut first = true;
        loop {
            let start_x = if first { text_x } else { p.mar_l };
            let avail = (wrap_w - start_x - p.mar_r).max(1.0);
            let mut x = 0.0f32;
            let mut j = i;
            let mut last_break: Option<usize> = None;
            let mut hard = false;
            while j < n {
                let c = &cls[j];
                if c.kind == Kind::Break {
                    j += 1;
                    hard = true;
                    break;
                }
                let cw = if c.kind == Kind::Tab {
                    // Nästa tabbstopp från rutans vänsterkant.
                    let abs = start_x + x;
                    let next = p.tabs.iter().copied().find(|&t| t > abs + 0.01).unwrap_or_else(|| {
                        let tab = p.def_tab.max(1.0);
                        ((abs / tab).floor() + 1.0) * tab
                    });
                    next - abs
                } else {
                    c.width
                };
                let fits = x + cw <= avail + 0.01 || c.kind == Kind::Space;
                if !fits && j > i {
                    break;
                }
                x += cw;
                if c.break_after {
                    last_break = Some(j);
                }
                j += 1;
            }
            // Bryt vid senaste brytmöjlighet om raden blev för lång.
            let mut end = j;
            if !hard && j < n {
                if let Some(b) = last_break {
                    if b + 1 > i && b + 1 < j + 1 {
                        end = b + 1;
                    }
                }
                if end <= i {
                    end = (i + 1).min(n);
                }
            }
            let line_end_content = if hard { end - 1 } else { end };
            // Bredd utan avslutande blanksteg.
            let mut width = 0.0;
            let mut k = line_end_content;
            while k > i && matches!(cls[k - 1].kind, Kind::Space) {
                k -= 1;
            }
            let mut xx = 0.0f32;
            for c in &cls[i..k] {
                if c.kind == Kind::Tab {
                    let abs = start_x + xx;
                    let next = p.tabs.iter().copied().find(|&t| t > abs + 0.01).unwrap_or_else(|| {
                        let tab = p.def_tab.max(1.0);
                        ((abs / tab).floor() + 1.0) * tab
                    });
                    xx += next - abs;
                } else {
                    xx += c.width;
                }
            }
            width += xx;
            // Radens mått.
            let (mut asc, mut desc, mut size, mut single) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for c in &cls[i..end.max(i)] {
                let s = &segs[c.seg];
                let (a, d, sg) = face_metrics(&s.face, s.style.size);
                asc = asc.max(a);
                desc = desc.max(d);
                single = single.max(sg);
                size = size.max(s.style.size);
            }
            if size == 0.0 {
                // Tom rad: styckets slutstil.
                let res = book.resolve(&p.end_style.latin, p.end_style.bold, p.end_style.italic);
                let (a, d, sg) = face_metrics(&res.face, p.end_style.size);
                asc = a;
                desc = d;
                single = sg;
                size = p.end_style.size;
            }
            if let Some((bseg, _, _)) = bullet.as_ref().filter(|_| first) {
                let (a, d, sg) = face_metrics(&bseg.face, bseg.style.size);
                asc = asc.max(a);
                desc = desc.max(d);
                single = single.max(sg);
            }
            lines.push(Line { from: i, to: end.max(i), start_x, avail, width, ascent: asc, descent: desc, size, single, hard_end: hard, first });
            first = false;
            i = end.max(i);
            if i >= n {
                // Ett hårt radslut sist ger en tom rad till.
                if hard {
                    let res = book.resolve(&p.end_style.latin, p.end_style.bold, p.end_style.italic);
                    let (a, d, sg) = face_metrics(&res.face, p.end_style.size);
                    lines.push(Line { from: n, to: n, start_x: p.mar_l, avail, width: 0.0, ascent: a, descent: d, size: p.end_style.size, single: sg, hard_end: false, first: false });
                }
                break;
            }
        }
        // Radavstånd.
        let red = frame.ln_spc_reduction;
        let line_heights: Vec<f32> = lines
            .iter()
            .map(|l| match p.ln_spc {
                Spacing::Pct(pct) => (pct - red).max(0.1) * l.single,
                Spacing::Pts(pt) => pt,
            })
            .collect();
        let first_single = lines.first().map_or(0.0, |l| l.single);
        // Var baslinjen ligger i radens låda.
        let asc_parts: Vec<f32> = lines
            .iter()
            .zip(&line_heights)
            .map(|(l, &lh)| match p.ln_spc {
                // Minskningen tas från uppstapeln.
                Spacing::Pct(_) | Spacing::Pts(_) => (lh - l.descent).max(lh * 0.5),
            })
            .collect();
        let spc_bef = if pi == 0 { 0.0 } else { spacing_pts(p.spc_bef, 0.0, first_single) };
        let spc_aft = spacing_pts(p.spc_aft, 0.0, lines.last().map_or(0.0, |l| l.single));
        plaid.push(PLaid { segs, cls, lines, bullet, spc_bef, spc_aft, line_heights, asc_parts });
    }

    // 2. Lodrät placering: varje rad är en låda med radhöjden, baslinjen `asc_parts` ner i lådan.
    struct Placed {
        para: usize,
        line: usize,
        col: usize,
        baseline: f32,
    }
    let mut placed: Vec<Placed> = Vec::new();
    let avail_h = (h - it - ib).max(0.0);
    let mut col = 0usize;
    let mut y = 0.0f32;
    let mut col_heights = vec![0.0f32; ncol];
    let mut prev_aft = 0.0f32;
    for (pi, pl) in plaid.iter().enumerate() {
        y += prev_aft + pl.spc_bef;
        for li in 0..pl.lines.len() {
            let lh = pl.line_heights[li];
            if ncol > 1 && col + 1 < ncol && y + lh > avail_h + 0.5 && y > 0.0 {
                col_heights[col] = y;
                col += 1;
                y = 0.0;
            }
            placed.push(Placed { para: pi, line: li, col, baseline: y + pl.asc_parts[li] });
            y += lh;
        }
        prev_aft = pl.spc_aft;
    }
    col_heights[col] = y;
    let total = col_heights.iter().copied().fold(0.0f32, f32::max);
    let offset = match frame.anchor {
        Anchor::Top => it,
        Anchor::Middle => it + (avail_h - total) / 2.0,
        Anchor::Bottom => it + avail_h - total,
    };

    // anchorCtr: textblocket centreras vågrätt (bredaste raden).
    let block_shift = if frame.anchor_ctr {
        let widest = plaid.iter().flat_map(|p| p.lines.iter().map(|l| l.start_x + l.width)).fold(0.0f32, f32::max);
        ((inner_w - widest) / 2.0).max(0.0)
    } else {
        0.0
    };

    // 3. Utritning.
    let mut items = Vec::new();
    for pl_ref in &placed {
        let pl = &plaid[pl_ref.para];
        let p = &frame.paras[pl_ref.para];
        let line = &pl.lines[pl_ref.line];
        let base_y = offset + pl_ref.baseline;
        let col_x = il + pl_ref.col as f32 * (col_w + col_gap) + block_shift;
        let line_avail = if frame.wrap { line.avail } else { (inner_w - line.start_x - p.mar_r).max(line.width) };
        let slack = (line_avail - line.width).max(0.0);
        let last_line = pl_ref.line + 1 == pl.lines.len() || line.hard_end;
        let (mut x, extra_per_space) = match p.align {
            Align::Center => (line.start_x + slack / 2.0, 0.0),
            Align::Right => (line.start_x + slack, 0.0),
            Align::Justify | Align::Distributed if !last_line || p.align == Align::Distributed => {
                let spaces = pl.cls[line.from..line.to].iter().filter(|c| c.kind == Kind::Space).count();
                if spaces > 0 { (line.start_x, slack / spaces as f32) } else { (line.start_x, 0.0) }
            }
            _ => (line.start_x, 0.0),
        };
        x += col_x;
        // Punkten.
        if line.first {
            if let Some((bseg, bglyphs, _)) = &pl.bullet {
                let bx = col_x + (p.mar_l + p.indent).max(0.0);
                emit_glyphs(&mut items, bseg, bglyphs.iter().copied(), bx, base_y, &bseg.text);
            }
        }
        // Visuell ordning: höger-till-vänster-kluster i följd vänds.
        let mut order: Vec<usize> = (line.from..line.to).filter(|&k| pl.cls[k].kind != Kind::Break).collect();
        // Ta bort avslutande blanksteg ur ritningen (de syns inte, men påverkar inte heller justering).
        while order.last().is_some_and(|&k| pl.cls[k].kind == Kind::Space) {
            order.pop();
        }
        // Förenklad bidi (UAX #9, regel L2): nivå 1 för höger-till-vänster-kluster, 0 eller 2 för
        // vänster-till-höger beroende på styckets riktning; sedan vänds varje följd med nivå ≥ n.
        let levels: Vec<u8> = order
            .iter()
            .map(|&k| match (pl.cls[k].rtl, p.rtl) {
                (true, _) => 1,
                (false, false) => 0,
                (false, true) => 2,
            })
            .collect();
        let max = levels.iter().copied().max().unwrap_or(0);
        for lvl in (1..=max).rev() {
            let mut i = 0;
            while i < order.len() {
                if levels[i] >= lvl {
                    let mut e = i;
                    while e < order.len() && levels[e] >= lvl {
                        e += 1;
                    }
                    order[i..e].reverse();
                    i = e;
                } else {
                    i += 1;
                }
            }
        }
        // Gruppera i körningar med samma segment.
        let mut run_start = x;
        let mut cur_seg: Option<usize> = None;
        let mut buf: Vec<(u16, f32, f32, f32)> = Vec::new();
        let mut text = String::new();
        let mut deco: Vec<(usize, f32, f32)> = Vec::new();
        let flush = |items: &mut Vec<Item>, seg: Option<usize>, buf: &mut Vec<(u16, f32, f32, f32)>, text: &mut String, start: f32| {
            if let Some(s) = seg {
                if !buf.is_empty() {
                    emit_glyphs(items, &pl.segs[s], buf.drain(..), start, base_y, text);
                }
            }
            buf.clear();
            text.clear();
        };
        let mut xx = x;
        for &ci in &order {
            let c = &pl.cls[ci];
            if cur_seg != Some(c.seg) {
                flush(&mut items, cur_seg, &mut buf, &mut text, run_start);
                cur_seg = Some(c.seg);
                run_start = xx;
            }
            let seg = &pl.segs[c.seg];
            let cw = match c.kind {
                Kind::Tab => {
                    let abs = xx - col_x;
                    let next = p.tabs.iter().copied().find(|&t| t > abs + 0.01).unwrap_or_else(|| {
                        let tab = p.def_tab.max(1.0);
                        ((abs / tab).floor() + 1.0) * tab
                    });
                    next - abs
                }
                Kind::Space => c.width + extra_per_space,
                _ => c.width,
            };
            if c.kind == Kind::Normal || c.kind == Kind::Space {
                let mut gx = xx - run_start;
                for &(gid, adv, xo, yo) in &c.glyphs {
                    buf.push((gid, gx + xo, -yo, adv));
                    gx += adv;
                }
                let (a, b) = (c.range.0 as usize, c.range.1 as usize);
                if let Some(t) = seg.text.get(a..b) {
                    text.push_str(t);
                }
            }
            if seg.style.underline || seg.style.strike || seg.style.highlight.is_some() {
                deco.push((c.seg, xx, cw));
            }
            xx += cw;
        }
        flush(&mut items, cur_seg, &mut buf, &mut text, run_start);
        // Understrykning, genomstrykning och markering.
        let mut d = 0;
        while d < deco.len() {
            let (s, x0, _) = deco[d];
            let mut x1 = deco[d].1 + deco[d].2;
            let mut e = d + 1;
            while e < deco.len() && deco[e].0 == s && (deco[e].1 - x1).abs() < 0.5 {
                x1 = deco[e].1 + deco[e].2;
                e += 1;
            }
            let st = &pl.segs[s].style;
            let thick = (st.size * 0.06).max(0.5);
            if let Some(hl) = st.highlight {
                let (a, dsc, _) = face_metrics(&pl.segs[s].face, st.size);
                items.insert(0, Item::Fill { path: Path::rect(x0, base_y - a, x1 - x0, a + dsc), paint: Paint::Solid(hl), even_odd: false });
            }
            if st.underline {
                items.push(Item::Fill { path: Path::rect(x0, base_y + st.size * 0.1, x1 - x0, thick), paint: Paint::Solid(st.color), even_odd: false });
            }
            if st.strike {
                items.push(Item::Fill { path: Path::rect(x0, base_y - st.size * 0.3, x1 - x0, thick), paint: Paint::Solid(st.color), even_odd: false });
            }
            d = e;
        }
    }
    Laid { items, height: total }
}

/// Skriver en glyfkörning. `glyphs` är (id, x relativt start, y, framflyttning).
fn emit_glyphs(items: &mut Vec<Item>, seg: &Seg, glyphs: impl Iterator<Item = (u16, f32, f32, f32)>, x: f32, y: f32, text: &str) {
    let base_shift = -seg.style.baseline * seg.style.size;
    let placed: Vec<PlacedGlyph> = glyphs.map(|(id, gx, gy, adv)| PlacedGlyph { id, x: gx, y: gy, advance: adv }).collect();
    if placed.is_empty() {
        return;
    }
    items.push(Item::Glyphs(GlyphRun {
        font: seg.face.clone(),
        size: seg.style.size,
        t: Affine::scale(1.0, 1.0).then(&Affine::translate(x, y + base_shift)),
        glyphs: placed,
        paint: Paint::Solid(seg.style.color),
        outline: seg.style.outline.clone(),
        text: text.to_owned(),
        fake_bold: seg.res.fake_bold,
        fake_italic: seg.res.fake_italic,
        x_scale: seg.res.x_scale,
    }));
}

/// Flyttar utlagd text till bildkoordinater.
pub fn transform_items(items: Vec<Item>, t: &Affine) -> Vec<Item> {
    items
        .into_iter()
        .map(|it| match it {
            Item::Fill { path, paint, even_odd } => Item::Fill { path: path.transformed(t), paint, even_odd },
            Item::Stroke { path, stroke } => Item::Stroke { path: path.transformed(t), stroke },
            Item::Glyphs(mut g) => {
                g.t = g.t.then(t);
                Item::Glyphs(g)
            }
            Item::Image { image, t: it, crop, alpha } => Item::Image { image, t: it.then(t), crop, alpha },
            Item::Svg { tree, t: it, crop, alpha } => Item::Svg { tree, t: it.then(t), crop, alpha },
            Item::PushClip { path } => Item::PushClip { path: path.transformed(t) },
            Item::PopClip => Item::PopClip,
        })
        .collect()
}
