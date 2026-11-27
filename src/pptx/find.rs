//! Sök i presentationen: bildernas text ur visningslistans glyfkörningar (så att träffarna kan
//! markeras där texten faktiskt ritas) och anteckningarnas text. Rena funktioner; vyn bygger
//! registret i en bakgrundstråd och ritar rektanglarna.

use super::scene::{GlyphRun, Item, Scene};

/// En glyfkörnings text i bildens söktext, med läget för varje tecken.
#[derive(Clone, Debug)]
pub struct RunText {
    /// Byteintervall i [`SlideText::text`].
    pub start: usize,
    pub end: usize,
    /// Tecknens byteoffset i `SlideText::text` (ett per tecken i körningen).
    chars: Vec<usize>,
    /// Vågrätt läge (x0, x1) i körningens koordinater per tecken, när tecken och glyfer går ett
    /// till ett (vänster till höger); annars `None` och hela körningen markeras.
    xs: Option<Vec<(f32, f32)>>,
    /// Körningens utsträckning (x0, x1) i körningens koordinater.
    x0: f32,
    x1: f32,
    size: f32,
    t: super::scene::Affine,
}

/// En bilds text för sökning.
#[derive(Clone, Debug, Default)]
pub struct SlideText {
    pub text: String,
    pub runs: Vec<RunText>,
}

/// Var en träff finns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Slide,
    Notes,
}

/// En träff: bild, plats och byteintervall i bildens söktext eller anteckningstext.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub slide: usize,
    pub place: Place,
    pub start: usize,
    pub end: usize,
}

fn run_extent(g: &GlyphRun) -> (f32, f32) {
    let x0 = g.glyphs.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
    let x1 = g.glyphs.iter().map(|p| p.x + p.advance.max(0.0)).fold(f32::NEG_INFINITY, f32::max);
    if x0.is_finite() && x1.is_finite() { (x0, x1) } else { (0.0, 0.0) }
}

/// Bildens text ur visningslistan. Körningar på samma baslinje som fortsätter där förra slutade
/// fogas ihop utan mellanrum (ett ord kan vara uppdelat på flera formateringar); annars skiljs de
/// åt med ett blanksteg, så att en fras över en radbrytning hittas.
pub fn slide_text(scene: &Scene) -> SlideText {
    let mut st = SlideText::default();
    let mut prev: Option<(f32, f32, f32)> = None;
    for it in &scene.items {
        let Item::Glyphs(g) = it else { continue };
        if g.text.is_empty() {
            continue;
        }
        let (x0, x1) = run_extent(g);
        let (sx, sy) = g.t.apply(x0, 0.0);
        let glued = prev.is_some_and(|(ex, ey, size)| (sy - ey).abs() < 0.3 * size && (sx - ex).abs() < 0.25 * size);
        if !st.text.is_empty() && !glued && !st.text.ends_with(' ') {
            st.text.push(' ');
        }
        let start = st.text.len();
        let mut chars = Vec::new();
        for (o, _) in g.text.char_indices() {
            chars.push(start + o);
        }
        st.text.push_str(&g.text);
        // Tecken ↔ glyf ett till ett bara när antalen stämmer och glyferna går åt höger.
        let ltr = g.glyphs.windows(2).all(|w| w[1].x >= w[0].x - 0.01);
        let xs = (chars.len() == g.glyphs.len() && ltr).then(|| g.glyphs.iter().map(|p| (p.x, p.x + p.advance.max(0.0))).collect());
        st.runs.push(RunText { start, end: st.text.len(), chars, xs, x0, x1, size: g.size, t: g.t });
        let (ex, ey) = g.t.apply(x1, 0.0);
        prev = Some((ex, ey, g.size.max(1.0)));
    }
    st
}

/// Små och stora bokstäver lika: jämför tecknens gemena former.
fn same(a: char, b: char, case: bool) -> bool {
    if case { a == b } else { a == b || a.to_lowercase().eq(b.to_lowercase()) }
}

/// Alla förekomster av `needle` i `hay` som byteintervall (ej överlappande).
pub fn find_all(hay: &str, needle: &str, case: bool) -> Vec<(usize, usize)> {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() {
        return Vec::new();
    }
    let h: Vec<(usize, char)> = hay.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= h.len() {
        if (0..n.len()).all(|k| same(h[i + k].1, n[k], case)) {
            let end = h.get(i + n.len()).map_or(hay.len(), |(o, _)| *o);
            out.push((h[i].0, end));
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Träffarna för en bild: först i bilden, sedan i anteckningarna.
pub fn hits_for(slide: usize, st: &SlideText, notes: &str, needle: &str, case: bool) -> Vec<Hit> {
    let mut out: Vec<Hit> = find_all(&st.text, needle, case).into_iter().map(|(start, end)| Hit { slide, place: Place::Slide, start, end }).collect();
    out.extend(find_all(notes, needle, case).into_iter().map(|(start, end)| Hit { slide, place: Place::Notes, start, end }));
    out
}

/// Rektanglar (x0, y0, x1, y1) i punkter på bilden som täcker byteintervallet i bildens text, en
/// per körning som intervallet berör.
pub fn hit_rects(st: &SlideText, start: usize, end: usize) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    for r in &st.runs {
        if r.end <= start || r.start >= end {
            continue;
        }
        let (a, b) = match &r.xs {
            Some(xs) => {
                let first = r.chars.iter().position(|&o| o >= start).unwrap_or(0);
                let last = r.chars.iter().rposition(|&o| o < end).unwrap_or(r.chars.len().saturating_sub(1));
                match (xs.get(first), xs.get(last)) {
                    (Some(f), Some(l)) => (f.0, l.1.max(f.0)),
                    _ => (r.x0, r.x1),
                }
            }
            None => (r.x0, r.x1),
        };
        // Från uppstapel till nedstapel ungefär, relativt baslinjen (y nedåt).
        let (top, bottom) = (-0.9 * r.size, 0.25 * r.size);
        let pts = [r.t.apply(a, top), r.t.apply(b, top), r.t.apply(a, bottom), r.t.apply(b, bottom)];
        let x0 = pts.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let y0 = pts.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let x1 = pts.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
        let y1 = pts.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        if x1 > x0 && y1 > y0 {
            out.push([x0, y0, x1, y1]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hittar_utan_skiftlägeskänslighet_och_med_å_ä_ö() {
        assert_eq!(find_all("Åsa och åsna", "ÅS", false), vec![(0, 3), (9, 12)]);
        assert_eq!(find_all("Åsa och åsna", "Ås", true), vec![(0, 3)]);
        assert!(find_all("Åsa och åsna", "ÅS", true).is_empty());
        assert_eq!(find_all("aaaa", "aa", false), vec![(0, 2), (2, 4)]);
        assert!(find_all("abc", "", false).is_empty());
        assert!(find_all("ab", "abc", false).is_empty());
    }
}
