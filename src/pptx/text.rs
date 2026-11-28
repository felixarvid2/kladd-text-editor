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
use super::scene::{Affine, GlyphRun, Item, NO_CHAR, Paint, Path, PlacedGlyph, Stroke};

/// Versaler.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Caps {
    #[default]
    None,
    All,
    Small,
}

/// Understrykningens utseende (`u`). `Single` gäller också när bara `underline` är satt.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Underline {
    #[default]
    Single,
    Double,
    Heavy,
    Dotted,
    DottedHeavy,
    Dash,
    DashHeavy,
    DashLong,
    DashLongHeavy,
    DotDash,
    DotDashHeavy,
    DotDotDash,
    DotDotDashHeavy,
    Wavy,
    WavyHeavy,
    WavyDouble,
    /// Bara orden (inte blankstegen).
    Words,
}

impl Underline {
    /// `ST_TextUnderlineType` (`none` ger `None`).
    pub fn from_attr(v: &str) -> Option<Underline> {
        use Underline::*;
        Some(match v {
            "none" => return None,
            "dbl" => Double,
            "heavy" => Heavy,
            "dotted" => Dotted,
            "dottedHeavy" => DottedHeavy,
            "dash" => Dash,
            "dashHeavy" => DashHeavy,
            "dashLong" => DashLong,
            "dashLongHeavy" => DashLongHeavy,
            "dotDash" => DotDash,
            "dotDashHeavy" => DotDashHeavy,
            "dotDotDash" => DotDotDash,
            "dotDotDashHeavy" => DotDotDashHeavy,
            "wavy" => Wavy,
            "wavyHeavy" => WavyHeavy,
            "wavyDbl" => WavyDouble,
            "words" => Words,
            _ => Single,
        })
    }

    fn heavy(self) -> bool {
        use Underline::*;
        matches!(self, Heavy | DottedHeavy | DashHeavy | DashLongHeavy | DotDashHeavy | DotDotDashHeavy | WavyHeavy)
    }

    /// Streckmönster i linjebredder.
    fn dash(self) -> &'static [f32] {
        use Underline::*;
        match self {
            Dotted | DottedHeavy => &[1.0, 1.0],
            Dash | DashHeavy => &[4.0, 2.0],
            DashLong | DashLongHeavy => &[8.0, 3.0],
            DotDash | DotDashHeavy => &[4.0, 2.0, 1.0, 2.0],
            DotDotDash | DotDotDashHeavy => &[4.0, 2.0, 1.0, 2.0, 1.0, 2.0],
            _ => &[],
        }
    }
}

/// En körnings färdiga stil.
#[derive(Clone, Debug)]
pub struct RunStyle {
    pub latin: String,
    pub ea: String,
    pub cs: String,
    /// `a:sym`: typsnitt för privata symboltecken (U+F020–F0FF).
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
    /// Texteffekter (`a:rPr/a:effectLst`: skugga, glöd …).
    pub effects: Vec<super::scene::Effect>,
    /// Understrykningens utseende och färg (`uFill`; annars textens färg).
    pub underline_kind: Underline,
    pub underline_color: Option<Rgba>,
    /// Dubbel genomstrykning (`dblStrike`).
    pub strike_double: bool,
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
            effects: Vec::new(),
            underline_kind: Underline::Single,
            underline_color: None,
            strike_double: false,
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
    /// Körningen (index i `Para::runs`) och tecknet i körningens text där segmentet börjar
    /// (för redigeringens markörlägen, `layout_carets`).
    run: usize,
    run_char: usize,
    /// Segmentets första tecken i styckets text (för animeringar bokstav för bokstav).
    para_char: usize,
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
    /// Bidinivå (UAX #9): udda = höger till vänster.
    level: u8,
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

/// Bidinivåer (UAX #9) per tecken för styckets körningar, i samma teckenföljd som `shape_run` formar
/// (efter versalinställningen). Radbrytningar räknas som radavgränsare (inte nytt stycke).
fn para_levels(runs: &[Run], rtl: bool) -> Vec<Vec<u8>> {
    let texts: Vec<String> = runs.iter().map(|r| if r.line_break { "\u{2028}".to_owned() } else { apply_caps(&r.text, r.style.caps) }).collect();
    let all: String = texts.concat();
    let base = if rtl { 1 } else { 0 };
    if !rtl && !all.chars().any(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x202B | 0x202E | 0x2067)) {
        // Bara vänster-till-höger: nivå 0 överallt (snabbt fall).
        return texts.iter().map(|t| vec![0; t.chars().count()]).collect();
    }
    let level = if rtl { unicode_bidi::Level::rtl() } else { unicode_bidi::Level::ltr() };
    let info = unicode_bidi::BidiInfo::new(&all, Some(level));
    let mut out = Vec::with_capacity(texts.len());
    let mut off = 0usize;
    for t in &texts {
        out.push(t.char_indices().map(|(i, _)| info.levels.get(off + i).map_or(base, |l| l.number())).collect());
        off += t.len();
    }
    out
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

/// Storleken för upphöjd/nedsänkt text.
const SCRIPT_SIZE: f32 = 2.0 / 3.0;

/// Kapitälernas storlek (gemener ritas som versaler i den här andelen av storleken).
const SMALL_CAPS: f32 = 0.8;

/// Delar en körning i segment efter skrift, teckentäckning och bidinivå och formar dem. `levels` är
/// bidinivån per tecken (efter versalinställningen); saknas den gäller nivå 0.
fn shape_run(run: &Run, book: &FontBook, segs: &mut Vec<Seg>, cls: &mut Vec<Cl>, levels: &[u8]) {
    let style = &run.style;
    // Upphöjt/nedsänkt ritas i två tredjedelar av storleken (uppmätt i PowerPoints PDF: cppcon14-
    // parallel-overview bild 7, cppcon19-charconv bild 50).
    let size = if style.baseline != 0.0 { style.size * SCRIPT_SIZE } else { style.size };
    if run.line_break {
        let res = book.resolve(&style.latin, style.bold, style.italic);
        segs.push(Seg { face: res.face.clone(), res, style: style.clone(), text: String::new(), run: 0, run_char: 0, para_char: 0 });
        cls.push(Cl { seg: segs.len() - 1, range: (0, 0), glyphs: Vec::new(), width: 0.0, kind: Kind::Break, break_after: true, level: levels.first().copied().unwrap_or(0) });
        return;
    }
    if style.caps == Caps::Small {
        // Kapitäler: gemener blir mindre versaler, övriga tecken behåller storleken.
        let mut groups: Vec<(String, bool)> = Vec::new();
        for c in run.text.chars() {
            let small = c.is_lowercase();
            match groups.last_mut() {
                Some((g, sm)) if *sm == small => g.extend(c.to_uppercase()),
                _ => groups.push((c.to_uppercase().collect(), small)),
            }
        }
        let mut k = 0usize;
        for (g, small) in groups {
            let n = g.chars().count();
            let mut st = style.clone();
            st.caps = Caps::None;
            if small {
                st.size *= SMALL_CAPS;
            }
            let lv = levels.get(k..(k + n).min(levels.len())).unwrap_or(&[]);
            shape_run(&Run { text: g, style: st, line_break: false }, book, segs, cls, lv);
            k += n;
        }
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
    let mut pieces: Vec<(Resolved, Arc<FontFace>, String, u8)> = Vec::new();
    let mut cur: Option<(Resolved, Arc<FontFace>, String, u8)> = None;
    let chars: Vec<char> = text.chars().collect();
    for (i, &c0) in chars.iter().enumerate() {
        let level = levels.get(i).copied().unwrap_or(0);
        let class = script_class(c0);
        // Löser tecknets typsnitt; `ok` = tecknet finns i det slutliga typsnittet.
        let pick = |family: &str| {
            let res = book.resolve(family, style.bold, style.italic);
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
            let ok = c_out.is_whitespace() || c_out.is_control() || (!face.family.contains("Unifont") && face.face().is_some_and(|f| f.glyph_index(c_out).is_some()));
            (res, face, c_out, ok)
        };
        let (mut res, mut face, mut c_out, ok) = pick(fam(class));
        // Ett privat tecken (U+F020–F0FF) som ingen reserv kan rita ritas i `a:sym`-typsnittet (Wingdings-pilen i
        // `cppcon20-stl-features` bild 33 blev en tom ruta). Tecken som redan ritas som förut lämnas (msrc-identity 33).
        if let Some(sym) = style.sym.as_deref().filter(|_| !ok && (0xF020..=0xF0FF).contains(&(c0 as u32))) {
            (res, face, c_out, _) = pick(sym);
            // Wingdings smileys är ca 15 % smalare än DejaVus (mätt mot PowerPoints PDF, msrc-identity 33); pilarna
            // har samma bredd (cppcon20-stl-features 22 blev sämre med skalning).
            if matches!(c_out, '\u{263A}' | '\u{2639}' | '\u{1F610}') {
                res.x_scale *= 0.85;
            }
        }
        // Byt bit när bidinivån byter (UAX #9).
        match &mut cur {
            Some((r, f, s, lv)) if Arc::ptr_eq(f, &face) && r.x_scale == res.x_scale && r.fake_bold == res.fake_bold && r.fake_italic == res.fake_italic && *lv == level => {
                s.push(c_out);
            }
            _ => {
                if let Some(p) = cur.take() {
                    pieces.push(p);
                }
                cur = Some((res, face, c_out.to_string(), level));
            }
        }
    }
    if let Some(p) = cur.take() {
        pieces.push(p);
    }
    for (res, face, piece, level) in pieces {
        let seg_idx = segs.len();
        let mut st = style.clone();
        st.size = size;
        let spacing = st.spacing;
        segs.push(Seg { face: face.clone(), res: res.clone(), style: st, text: piece.clone(), run: 0, run_char: 0, para_char: 0 });
        let Some(rb) = face.shaper() else { continue };
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(&piece);
        buf.guess_segment_properties();
        // Riktningen ur bidinivån (siffror i hebreisk text har jämn nivå och formas vänster till höger).
        let rtl = level % 2 == 1;
        buf.set_direction(if rtl { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
        // Kerning alltid (som förut). `kern` (minsta storlek för kerning) prövades: bara rutor med
        // 9–11 pt brödtext ändrades, och måttet blev bättre i två och sämre i två presentationer.
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
            cls.push(Cl {
                seg: seg_idx,
                range: (start, end),
                glyphs,
                width,
                kind,
                break_after: break_after_char(last, next) || kind == Kind::Tab,
                level,
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

/// Ett markörläge i en utlagd textruta (för redigeringen): början av ett kluster, en radbrytning
/// eller styckets slut. Koordinaterna är textrutans, som `Laid::items`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretStop {
    pub para: usize,
    /// Körningen i `Para::runs` (`usize::MAX` = styckets slut).
    pub run: usize,
    /// Tecknet i körningens text.
    pub ch: usize,
    /// Klustrets vänsterkant och bredd.
    pub x: f32,
    pub w: f32,
    pub base: f32,
    pub asc: f32,
    pub desc: f32,
    /// Radens nummer i rutan (räknat över alla stycken).
    pub line: usize,
}

/// Lägger ut en textruta med storleken `w`×`h` (punkter).
pub fn layout(frame: &Frame, w: f32, h: f32, book: &FontBook) -> Laid {
    layout_impl(frame, w, h, book, None)
}

/// Som `layout`, och dessutom markörlägena (samma utläggning, så markören står där texten ritas).
pub fn layout_carets(frame: &Frame, w: f32, h: f32, book: &FontBook) -> (Laid, Vec<CaretStop>) {
    let mut stops = Vec::new();
    let laid = layout_impl(frame, w, h, book, Some(&mut stops));
    (laid, stops)
}

fn layout_impl(frame: &Frame, w: f32, h: f32, book: &FontBook, mut carets: Option<&mut Vec<CaretStop>>) -> Laid {
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
        /// Ordnumret för varje tecken i stycket (`scene::word_index`).
        words: Vec<u32>,
    }
    let mut plaid: Vec<PLaid> = Vec::new();
    for (pi, p) in frame.paras.iter().enumerate() {
        let mut segs = Vec::new();
        let mut cls = Vec::new();
        let levels = para_levels(&p.runs, p.rtl);
        let mut para_chars = 0usize;
        for (ri, (r, lv)) in p.runs.iter().zip(&levels).enumerate() {
            let s0 = segs.len();
            shape_run(r, book, &mut segs, &mut cls, lv);
            let mut ch = 0;
            for s in &mut segs[s0..] {
                s.run = ri;
                s.run_char = ch;
                s.para_char = para_chars + ch;
                ch += s.text.chars().count();
            }
            para_chars += ch;
        }
        let words = super::scene::word_index(&segs.iter().flat_map(|s| s.text.chars()).collect::<Vec<char>>());
        // Punkten (bara om stycket har text).
        let has_text = p.runs.iter().any(|r| !r.line_break && !r.text.is_empty());
        let bullet = p.bullet.as_ref().filter(|_| has_text).and_then(|b| {
            let mut bs = Vec::new();
            let mut bc = Vec::new();
            shape_run(&Run { text: b.text.clone(), style: b.style.clone(), line_break: false }, book, &mut bs, &mut bc, &[]);
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
                    // Punkten är bredare än det hängande indraget: texten börjar direkt efter punkten
                    // (som i PowerPoints PDF-export, t.ex. cncf-overview-ja bild 38 och owasp-2010-es).
                    after
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
            // En radbrytning större än radens text (`a:br` utan `sz` ger 18 pt) höjer inte raden: PowerPoint
            // ritar `owasp-top10-2017-en` bild 23 med tät radhöjd trots det. En lika stor eller mindre
            // radbrytning räknas som förut (`cncf-overview-ja` blev sämre när den hoppades över).
            let text_size = cls[i..end.max(i)].iter().filter(|c| c.kind != Kind::Break).map(|c| segs[c.seg].style.size).fold(0.0f32, f32::max);
            for c in cls[i..end.max(i)].iter().filter(|c| !(c.kind == Kind::Break && text_size > 0.0 && segs[c.seg].style.size > text_size)) {
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
        plaid.push(PLaid { segs, cls, lines, bullet, spc_bef, spc_aft, line_heights, asc_parts, words });
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
    let mut cur_para = None;
    for pl_ref in &placed {
        // Styckemärke (för animeringar stycke för stycke).
        if cur_para != Some(pl_ref.para) {
            cur_para = Some(pl_ref.para);
            items.push(Item::Para(pl_ref.para as u32));
        }
        let pl = &plaid[pl_ref.para];
        let p = &frame.paras[pl_ref.para];
        let line = &pl.lines[pl_ref.line];
        let base_y = offset + pl_ref.baseline;
        let col_x = il + pl_ref.col as f32 * (col_w + col_gap) + block_shift;
        let line_avail = if frame.wrap { line.avail } else { (inner_w - line.start_x - p.mar_r).max(line.width) };
        let slack = (line_avail - line.width).max(0.0);
        let last_line = pl_ref.line + 1 == pl.lines.len() || line.hard_end;
        // Höger till vänster (`rtl="1"`): indrag och punkt räknas från högerkanten; `algn` är absolut
        // (l = vänster), men ett justerat styckes sista rad ställs till höger.
        let left = if p.rtl { p.mar_r } else { line.start_x };
        let (mut x, extra_per_space) = match p.align {
            Align::Center => (left + slack / 2.0, 0.0),
            Align::Right => (left + slack, 0.0),
            Align::Justify | Align::Distributed if !last_line || p.align == Align::Distributed => {
                let spaces = pl.cls[line.from..line.to].iter().filter(|c| c.kind == Kind::Space).count();
                if spaces > 0 { (left, slack / spaces as f32) } else { (left, 0.0) }
            }
            Align::Justify if p.rtl => (left + slack, 0.0),
            _ => (left, 0.0),
        };
        x += col_x;
        // Punkten.
        if line.first {
            if let Some((bseg, bglyphs, bw)) = &pl.bullet {
                let first_x = (p.mar_l + p.indent).max(0.0);
                let bx = if p.rtl {
                    col_x + col_w - first_x - bw
                } else if matches!(p.align, Align::Center | Align::Right) {
                    // Centrerad/högerställd text: punkten hänger det vanliga avståndet före texten i stället för
                    // att stå kvar vid vänstermarginalen.
                    x - (line.start_x - first_x)
                } else {
                    col_x + first_x
                };
                emit_glyphs(&mut items, bseg, bglyphs.iter().map(|&(id, x, y, a)| (id, x, y, a, NO_CHAR, NO_CHAR)), bx, base_y, &bseg.text);
            }
        }
        // Markörlägen (redigeringen): klustren i logisk ordning, med avslutande blanksteg.
        if let Some(stops) = carets.as_deref_mut() {
            let line_no = stops.last().map_or(0, |s: &CaretStop| s.line + 1);
            let mut xx = x;
            for c in &pl.cls[line.from..line.to] {
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
                    Kind::Break => 0.0,
                    Kind::Normal => c.width,
                };
                let ch = seg.run_char + seg.text.get(..c.range.0 as usize).map_or(0, |t| t.chars().count());
                stops.push(CaretStop { para: pl_ref.para, run: seg.run, ch, x: xx, w: cw, base: base_y, asc: line.ascent, desc: line.descent, line: line_no });
                xx += cw;
            }
            if pl_ref.line + 1 == pl.lines.len() {
                stops.push(CaretStop { para: pl_ref.para, run: usize::MAX, ch: 0, x: xx, w: 0.0, base: base_y, asc: line.ascent, desc: line.descent, line: line_no });
            }
        }
        // Visuell ordning: höger-till-vänster-kluster i följd vänds.
        let mut order: Vec<usize> = (line.from..line.to).filter(|&k| pl.cls[k].kind != Kind::Break).collect();
        // Ta bort avslutande blanksteg ur ritningen (de syns inte, men påverkar inte heller justering).
        while order.last().is_some_and(|&k| pl.cls[k].kind == Kind::Space) {
            order.pop();
        }
        // UAX #9, regel L2: från högsta nivån ner till lägsta udda nivån vänds varje följd med nivå ≥ n.
        let mut levels: Vec<u8> = order.iter().map(|&k| pl.cls[k].level).collect();
        let max = levels.iter().copied().max().unwrap_or(0);
        let min_odd = levels.iter().copied().min().unwrap_or(0) | 1;
        for lvl in (min_odd..=max).rev() {
            let mut i = 0;
            while i < order.len() {
                if levels[i] >= lvl {
                    let mut e = i;
                    while e < order.len() && levels[e] >= lvl {
                        e += 1;
                    }
                    order[i..e].reverse();
                    levels[i..e].reverse();
                    i = e;
                } else {
                    i += 1;
                }
            }
        }
        // Gruppera i körningar med samma segment.
        let mut run_start = x;
        let mut cur_seg: Option<usize> = None;
        let mut buf: Vec<(u16, f32, f32, f32, u32, u32)> = Vec::new();
        let mut text = String::new();
        let mut deco: Vec<(usize, f32, f32)> = Vec::new();
        let flush = |items: &mut Vec<Item>, seg: Option<usize>, buf: &mut Vec<(u16, f32, f32, f32, u32, u32)>, text: &mut String, start: f32| {
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
                let ch = seg.para_char + seg.text.get(..c.range.0 as usize).map_or(0, |t| t.chars().count());
                let word = pl.words.get(ch).copied().unwrap_or(NO_CHAR);
                for &(gid, adv, xo, yo) in &c.glyphs {
                    buf.push((gid, gx + xo, -yo, adv, ch as u32, word));
                    gx += adv;
                }
                let (a, b) = (c.range.0 as usize, c.range.1 as usize);
                if let Some(t) = seg.text.get(a..b) {
                    // Texten i logisk ordning (för sökning och PDF:ens textlager): höger-till-vänster-kluster
                    // kommer i visuell ordning och läggs därför först.
                    if c.level % 2 == 1 {
                        text.insert_str(0, t);
                    } else {
                        text.push_str(t);
                    }
                }
            }
            let words_gap = c.kind == Kind::Space && seg.style.underline_kind == Underline::Words && !seg.style.strike && seg.style.highlight.is_none();
            if (seg.style.underline || seg.style.strike || seg.style.highlight.is_some()) && !words_gap {
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
                underline(&mut items, st, x0, x1, base_y + st.size * 0.1, thick);
            }
            if st.strike {
                let y = base_y - st.size * 0.3;
                if st.strike_double {
                    items.push(Item::Fill { path: Path::rect(x0, y - thick, x1 - x0, thick * 0.7), paint: Paint::Solid(st.color), even_odd: false });
                    items.push(Item::Fill { path: Path::rect(x0, y + thick * 0.6, x1 - x0, thick * 0.7), paint: Paint::Solid(st.color), even_odd: false });
                } else {
                    items.push(Item::Fill { path: Path::rect(x0, y, x1 - x0, thick), paint: Paint::Solid(st.color), even_odd: false });
                }
            }
            d = e;
        }
    }
    Laid { items, height: total }
}

/// Ritar en understrykning från `x0` till `x1` med överkanten på `y`.
fn underline(items: &mut Vec<Item>, st: &RunStyle, x0: f32, x1: f32, y: f32, thick: f32) {
    let k = st.underline_kind;
    let color = st.underline_color.unwrap_or(st.color);
    let w = if k.heavy() { thick * 2.0 } else { thick };
    let fill = |items: &mut Vec<Item>, y: f32, h: f32| items.push(Item::Fill { path: Path::rect(x0, y, x1 - x0, h), paint: Paint::Solid(color), even_odd: false });
    let stroke = |items: &mut Vec<Item>, path: Path, dash: Vec<f32>| {
        items.push(Item::Stroke { path, stroke: Stroke { paint: Paint::Solid(color), width: w, dash, cap: super::scene::Cap::Butt, join: super::scene::Join::Round } })
    };
    match k {
        Underline::Double => {
            fill(items, y, thick * 0.7);
            fill(items, y + thick * 1.4, thick * 0.7);
        }
        Underline::Wavy | Underline::WavyHeavy | Underline::WavyDouble => {
            let amp = (w * 1.2).max(0.6);
            let period = (w * 6.0).max(2.0);
            let rows: &[f32] = if k == Underline::WavyDouble { &[0.0, 2.5] } else { &[0.0] };
            for &dy in rows {
                let yy = y + w / 2.0 + dy * w;
                let mut p = Path::default();
                p.move_to(x0, yy);
                let mut x = x0;
                let mut up = true;
                while x < x1 - 0.01 {
                    let nx = (x + period / 2.0).min(x1);
                    p.quad_to((x + nx) / 2.0, yy + if up { -amp } else { amp }, nx, yy);
                    x = nx;
                    up = !up;
                }
                stroke(items, p, Vec::new());
            }
        }
        _ if !k.dash().is_empty() => {
            let mut p = Path::default();
            p.move_to(x0, y + w / 2.0);
            p.line_to(x1, y + w / 2.0);
            stroke(items, p, k.dash().iter().map(|d| d * w).collect());
        }
        _ => fill(items, y, w),
    }
}

/// Skriver en glyfkörning. `glyphs` är (id, x relativt start, y, framflyttning, tecken, ord).
fn emit_glyphs(items: &mut Vec<Item>, seg: &Seg, glyphs: impl Iterator<Item = (u16, f32, f32, f32, u32, u32)>, x: f32, y: f32, text: &str) {
    // Förskjutningen räknas på körningens fulla storlek (inte den förminskade).
    let full = if seg.style.baseline != 0.0 { seg.style.size / SCRIPT_SIZE } else { seg.style.size };
    let base_shift = -seg.style.baseline * full;
    let placed: Vec<PlacedGlyph> = glyphs.map(|(id, gx, gy, adv, ch, word)| PlacedGlyph { id, x: gx, y: gy, advance: adv, ch, word }).collect();
    if placed.is_empty() {
        return;
    }
    // Tunna vikter (Calibri Light …) som ritas med ett typsnitt i normal vikt görs tunnare.
    let thin = if seg.style.bold || seg.text.chars().any(fonts::is_cjk) { 0.0 } else { fonts::thin_amount(&seg.style.latin, &seg.face) * seg.style.size };
    let fx = !seg.style.effects.is_empty() || thin > 0.0;
    if fx {
        let mut e = seg.style.effects.clone();
        if thin > 0.0 {
            e.push(super::scene::Effect::Thin { width: thin });
        }
        items.push(Item::BeginEffect(e));
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
    if fx {
        items.push(Item::EndEffect);
    }
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
            Item::BeginEffect(mut e) => {
                for x in &mut e {
                    if let super::scene::Effect::OuterShadow { t: st, .. } = x {
                        if *st != Affine::IDENTITY {
                            if let Some(inv) = t.invert() {
                                *st = inv.then(st).then(t);
                            }
                        }
                    }
                }
                Item::BeginEffect(e)
            }
            Item::EndEffect => Item::EndEffect,
            Item::BeginShape { id } => Item::BeginShape { id },
            Item::EndShape => Item::EndShape,
            Item::Para(n) => Item::Para(n),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(paras: Vec<Para>) -> Frame {
        Frame { paras, insets: (0.0, 0.0, 0.0, 0.0), wrap: true, ..Default::default() }
    }

    fn para(text: &str, rtl: bool) -> Para {
        let style = RunStyle { latin: "Liberation Sans".into(), cs: "Liberation Sans".into(), size: 12.0, ..Default::default() };
        Para { runs: vec![Run { text: text.into(), style, line_break: false }], rtl, align: if rtl { Align::Right } else { Align::Left }, ..Default::default() }
    }

    /// (text, vänsterkant) för glyfkörningarna.
    fn runs(laid: &Laid) -> Vec<(String, f32)> {
        laid.items
            .iter()
            .filter_map(|it| match it {
                Item::Glyphs(g) => Some((g.text.clone(), g.t.apply(g.glyphs[0].x, 0.0).0)),
                _ => None,
            })
            .collect()
    }

    fn x_of(r: &[(String, f32)], needle: &str) -> f32 {
        r.iter().find(|(t, _)| t.contains(needle)).unwrap_or_else(|| panic!("{needle} saknas i {r:?}")).1
    }

    #[test]
    fn bidi_hoger_till_vanster_med_latin_och_parentes() {
        let book = FontBook::default();
        // Höger-till-vänster-stycke: den latinska delen (med parenteser) hamnar till vänster om hebreiskan.
        let laid = layout(&frame(vec![para("שלום (Injection)", true)]), 400.0, 100.0, &book);
        let r = runs(&laid);
        assert!(x_of(&r, "Injection") < x_of(&r, "שלום"), "{r:?}");
        // Högerjusterat: texten slutar vid högerkanten.
        assert!(x_of(&r, "שלום") > 300.0, "{r:?}");
        // Vänster-till-höger-stycke med hebreiska i mitten: latinska ordens ordning behålls.
        let laid = layout(&frame(vec![para("abc שלום עולם def", false)]), 400.0, 100.0, &book);
        let r = runs(&laid);
        assert!(x_of(&r, "abc") < x_of(&r, "def"), "{r:?}");
        // Hebreiska ord i följd vänds: det första ordet (logiskt) står till höger om det andra.
        let levels = para_levels(&[Run { text: "abc שלום עולם def".into(), style: RunStyle::default(), line_break: false }], false);
        assert_eq!(levels[0][0], 0);
        assert_eq!(levels[0][4], 1);
        assert_eq!(levels[0][14], 0);
    }

    #[test]
    fn punkt_till_hoger_i_hoger_till_vanster_stycke() {
        let book = FontBook::default();
        let mut p = para("שלום עולם", true);
        p.mar_l = 20.0;
        p.indent = -20.0;
        p.bullet = Some(Bullet { text: "•".into(), style: p.runs[0].style.clone() });
        let laid = layout(&frame(vec![p]), 300.0, 100.0, &book);
        let r = runs(&laid);
        let bullet = x_of(&r, "•");
        assert!(bullet > 280.0, "punkten vid högerkanten: {r:?}");
        assert!(x_of(&r, "שלום") < bullet - 5.0, "{r:?}");
    }

    #[test]
    fn punkt_hanger_fore_centrerad_text() {
        let book = FontBook::default();
        let mk = |align| {
            let mut p = para("Mitten", false);
            p.align = align;
            p.mar_l = 20.0;
            p.indent = -20.0;
            p.bullet = Some(Bullet { text: "•".into(), style: p.runs[0].style.clone() });
            runs(&layout(&frame(vec![p]), 400.0, 100.0, &book))
        };
        let (l, c) = (mk(Align::Left), mk(Align::Center));
        // Vänsterställt: punkten vid marginalen (x = 0), texten efter indraget.
        assert!(x_of(&l, "•") < 1.0 && (x_of(&l, "Mitten") - 20.0).abs() < 1.0, "{l:?}");
        // Centrerat: punkten hänger lika långt före texten, och texten ligger i mitten.
        let (bx, tx) = (x_of(&c, "•"), x_of(&c, "Mitten"));
        assert!(((tx - bx) - 20.0).abs() < 1.0, "{c:?}");
        assert!(bx > 100.0, "punkten stod kvar vid vänstermarginalen: {c:?}");
    }

    #[test]
    fn privat_tecken_ritas_i_sym_typsnittet() {
        let book = FontBook::default();
        let mut p = para("a\u{F0E8}b", false);
        p.runs[0].style.sym = Some("Wingdings".into());
        let r = runs(&layout(&frame(vec![p]), 300.0, 100.0, &book));
        if book.resolve("Wingdings", false, false).face.family.contains("Wingdings") {
            // Riktig Wingdings finns (Windows/macOS i CI): tecknet ritas med den som det är.
            assert!(r.iter().any(|g| g.0.contains('\u{F0E8}')), "{r:?}");
        } else {
            // Wingdings-pilen blir ett riktigt Unicode-tecken i stället för en tom ruta.
            assert!(r.iter().any(|g| g.0.contains('\u{2794}')), "{r:?}");
            assert!(!r.iter().any(|g| g.0.contains('\u{F0E8}')), "{r:?}");
        }
    }

    #[test]
    fn kapitaler_gemener_mindre() {
        let book = FontBook::default();
        let mut p = para("Ab", false);
        p.runs[0].style.caps = Caps::Small;
        let laid = layout(&frame(vec![p]), 300.0, 100.0, &book);
        let sizes: Vec<(String, f32)> = laid.items.iter().filter_map(|it| if let Item::Glyphs(g) = it { Some((g.text.clone(), g.size)) } else { None }).collect();
        assert_eq!(sizes, vec![("A".to_owned(), 12.0), ("B".to_owned(), 12.0 * SMALL_CAPS)]);
    }
}
