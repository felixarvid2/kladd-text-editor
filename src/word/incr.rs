//! Inkrementell analys för stora dokument (hundratals sidor): efter en liten ändring tolkas bara
//! stycket med ändringen, plus ett stycke på var sida som "kanariefågel", i stället för hela texten.
//!
//! Resultatet ska vara exakt detsamma som `markup::analyze`. Därför återanvänds det gamla bara när
//! ändringen är liten och lokal och när kanariestyckena fick samma bitar som i den gamla analysen
//! (annars har ändringen påverkat något som sträcker sig över styckegränsen, t.ex. ett kodstaket
//! eller en `<span>`, och hela texten tolkas om). Konstruktioner som är beroende av hela dokumentet
//! (referenslänkar, kodstaket, indragen kod) ger alltid full analys. Fotnoter och avsnitt, som är vanliga i
//! Word-dokument, går bra så länge fönstret inte rör dem (fotnotsreferenser och -definitioner och avsnittsrader
//! finns bara i fönster som innehåller `[^` respektive `avsnitt`, och de ger full analys).

use std::ops::Range;

use super::markup::{self, Analysis, DocDefaults, Piece};

/// Under så här många byte tolkas alltid hela texten (små dokument beter sig som förut).
pub const MIN_BYTES: usize = 200_000;
/// Största ändring (byte) och största fönster som tolkas om.
const MAX_EDIT: usize = 4_000;
const MAX_WINDOW: usize = 120_000;

/// Vad som ändrades vid en inkrementell analys, så att cachar som beror på analysen kan behållas.
#[derive(Debug, Default)]
pub struct Delta {
    /// Fönstrets början (byte); allt före är oförändrat och ligger kvar på samma plats.
    pub ws: usize,
    /// Fönstrets slut i den gamla respektive nya texten: allt efter det är oförändrat men flyttat
    /// (`we_new - we_old` byte). Utanför fönstret är bitarna desamma som förut, så cachar byggda på dem kan behållas
    /// (`layout_memo`).
    pub we_old: usize,
    pub we_new: usize,
    /// Någon tabellbit fanns i det omtolkade fönstret (före eller efter).
    pub touched_table: bool,
    /// Bilderna i fönstret är desamma som förut (i samma ordning).
    pub same_images: bool,
    /// Fotnotsreferenserna och -definitionerna som bitarna i fönstret tolkats som har ändrats (fotnotsplanen i layouten
    /// beror på vilka referenser som finns i hela dokumentet).
    pub notes_changed: bool,
    /// Typsnitten som fönstrets nya bitar behöver.
    pub fonts: std::collections::BTreeSet<(String, bool, bool)>,
    /// Hur mycket det som kan komma med i en innehållsförteckning ändrats i fönstret (`nav::toc_sig_diff`; 0 = inget).
    pub toc_delta: isize,
}

/// Förekomster i hela texten av det som gör att en ändring var som helst kan ändra tolkningen av resten (tre tilda,
/// referenslänkar och oavslutade HTML-kommentarer). De räknas per rad, så att en ändring bara behöver räkna om fönstret
/// (`Facts::minus`/`plus`) i stället för att söka igenom hela texten vid varje tangent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    tildes: usize,
    link_defs: usize,
    open_comments: usize,
    /// Kodstaket (```) och hur många av dem som står först på en rad.
    fences: usize,
    fence_lines: usize,
}

impl Facts {
    /// Förekomsterna i `t`, som ska börja och sluta vid en radgräns (hela texten eller ett fönster mellan stycken).
    pub fn of(t: &str) -> Facts {
        let b = t.as_bytes();
        Facts {
            tildes: t.matches("~~~").count(),
            link_defs: t.match_indices("]:").filter(|(i, _)| !b[line_start(b, *i)..].starts_with(b"[^")).count(),
            open_comments: t.match_indices("<!--").filter(|(i, _)| !t[*i..line_end(b, *i)].contains("-->")).count(),
            fences: t.matches("```").count(),
            fence_lines: t.match_indices("```").filter(|(i, _)| t[line_start(b, *i)..*i].len() <= 3 && t[line_start(b, *i)..*i].bytes().all(|c| c == b' ')).count(),
        }
    }

    fn swap(&self, old_win: &Facts, new_win: &Facts) -> Facts {
        Facts {
            tildes: (self.tildes + new_win.tildes).saturating_sub(old_win.tildes),
            link_defs: (self.link_defs + new_win.link_defs).saturating_sub(old_win.link_defs),
            open_comments: (self.open_comments + new_win.open_comments).saturating_sub(old_win.open_comments),
            fences: (self.fences + new_win.fences).saturating_sub(old_win.fences),
            fence_lines: (self.fence_lines + new_win.fence_lines).saturating_sub(old_win.fence_lines),
        }
    }
}

/// Uppdaterar `old` (analysen av `old_text`) på plats så att den blir analysen av `new_text`.
/// `None` (och `old` orörd) om det inte går att göra säkert; då görs en full analys.
#[cfg(test)]
pub fn analyze_incremental(old_text: &str, old: &mut Analysis, new_text: &str) -> Option<Delta> {
    analyze_incremental_with(old_text, old, new_text, &mut None)
}

/// Som `analyze_incremental`, men med `facts` (för `old_text`; räknas fram om de saknas) som uppdateras till att gälla
/// `new_text`; `None` efter en full analys.
pub fn analyze_incremental_with(old_text: &str, old: &mut Analysis, new_text: &str, facts: &mut Option<Facts>) -> Option<Delta> {
    #[cfg(test)]
    if DISABLED.load(std::sync::atomic::Ordering::Relaxed) {
        *facts = None;
        return None;
    }
    #[cfg(test)]
    let min = MIN_OVERRIDE.with(|m| m.get()).unwrap_or(MIN_BYTES);
    #[cfg(not(test))]
    let min = MIN_BYTES;
    let d = incremental(old_text, old, new_text, min, facts);
    if d.is_none() {
        *facts = None;
    }
    d
}

// Prov: lägre gräns för storleken (per tråd, så att andra prov inte påverkas).
#[cfg(test)]
thread_local! {
    pub static MIN_OVERRIDE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Längden på det gemensamma prefixet (byte).
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    // Stora block först (en jämförelse per block går fortare än många små i en lång text).
    while i + 4096 <= n && a[i..i + 4096] == b[i..i + 4096] {
        i += 4096;
    }
    while i + 64 <= n && a[i..i + 64] == b[i..i + 64] {
        i += 64;
    }
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

fn common_suffix(a: &[u8], b: &[u8], limit: usize) -> usize {
    let n = a.len().min(b.len()).min(limit);
    let mut i = 0;
    while i + 4096 <= n && a[a.len() - i - 4096..a.len() - i] == b[b.len() - i - 4096..b.len() - i] {
        i += 4096;
    }
    while i + 64 <= n && a[a.len() - i - 64..a.len() - i] == b[b.len() - i - 64..b.len() - i] {
        i += 64;
    }
    while i < n && a[a.len() - 1 - i] == b[b.len() - 1 - i] {
        i += 1;
    }
    i
}

fn line_start(t: &[u8], pos: usize) -> usize {
    t[..pos.min(t.len())].iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1)
}

/// Slutet (exklusivt, utan radbrytningen) på raden som börjar vid `ls`.
fn line_end(t: &[u8], ls: usize) -> usize {
    t[ls..].iter().position(|&c| c == b'\n').map_or(t.len(), |i| ls + i)
}

fn blank(t: &[u8], a: usize, b: usize) -> bool {
    t[a..b].iter().all(|c| matches!(c, b' ' | b'\t' | b'\r'))
}

/// Början av stycket (raderna efter en blankrad) som innehåller `pos`.
fn para_start(t: &[u8], pos: usize) -> Option<usize> {
    let mut ls = line_start(t, pos);
    let floor = ls.saturating_sub(MAX_WINDOW);
    while ls > 0 {
        let prev = line_start(t, ls - 1);
        if blank(t, prev, ls - 1) {
            return Some(ls);
        }
        if prev < floor {
            return None;
        }
        ls = prev;
    }
    Some(0)
}

/// Slutet på stycket som innehåller `pos`: början på blankraden efter det (eller textens slut).
fn para_end(t: &[u8], pos: usize) -> Option<usize> {
    let mut ls = line_start(t, pos);
    let ceil = pos + MAX_WINDOW;
    loop {
        let le = line_end(t, ls);
        if blank(t, ls, le) && ls > 0 && ls != line_start(t, pos) {
            return Some(ls);
        }
        if le >= t.len() {
            return Some(t.len());
        }
        ls = le + 1;
        if ls > ceil {
            return None;
        }
    }
}

/// Längst sträcka (byte) som söks efter gränserna mellan fotnotsdefinitioner.
const MAX_DEF: usize = 16_000;

/// Börjar en fotnotsdefinition (`[^etikett]:`) på raden som börjar vid `ls`, enligt analysen `a` av `t`? Etiketten är en
/// egen bit (`footnote_def`) direkt efter `[^` (för slutnoter ett tecken senare).
fn def_start(t: &[u8], a: &Analysis, ls: usize) -> bool {
    if !t[ls.min(t.len())..].starts_with(b"[^") {
        return false;
    }
    let i = a.pieces.partition_point(|p| p.range.start < ls + 2);
    a.pieces.get(i).is_some_and(|p| p.footnote_def.is_some() && p.range.start <= ls + 3)
}

/// Början på raden före raden som börjar vid `ls` om den inte är tom (samma stycke).
fn prev_line(t: &[u8], ls: usize) -> Option<usize> {
    if ls == 0 {
        return None;
    }
    let p = line_start(t, ls - 1);
    (!blank(t, p, ls - 1)).then_some(p)
}

/// Början på raden efter raden som börjar vid `ls` om den finns och inte är tom (samma stycke).
fn next_line(t: &[u8], ls: usize) -> Option<usize> {
    let n = line_end(t, ls) + 1;
    (n < t.len() && !blank(t, n, line_end(t, n))).then_some(n)
}

/// Början på den sista definitionen i stycket före `at` (början på ett stycke, eller en rad mitt i ett), om det slutar med
/// en fotnotsdefinition: kanariefågeln före behöver då bara vara den definitionen.
fn last_def_start(t: &[u8], a: &Analysis, at: usize) -> Option<usize> {
    if at == 0 {
        return None;
    }
    let mut l = line_start(t, at - 1);
    while blank(t, l, line_end(t, l)) {
        if l == 0 {
            return None;
        }
        l = line_start(t, l - 1);
    }
    loop {
        if def_start(t, a, l) {
            return Some(l);
        }
        if at - l > MAX_DEF {
            return None;
        }
        l = prev_line(t, l)?;
    }
}

/// Slutet på den första definitionen i stycket efter `from` (efter blankraderna), om stycket är ett block av
/// fotnotsdefinitioner med fler än en definition: början på nästa definitionsrad.
fn first_def_end(t: &[u8], a: &Analysis, from: usize) -> Option<usize> {
    let mut ls = line_start(t, from.min(t.len()));
    while ls < t.len() && blank(t, ls, line_end(t, ls)) {
        ls = line_end(t, ls) + 1;
    }
    if ls >= t.len() || !def_start(t, a, ls) {
        return None;
    }
    let start = ls;
    loop {
        ls = next_line(t, ls)?;
        if ls - start > MAX_DEF {
            return None;
        }
        if def_start(t, a, ls) {
            return Some(ls);
        }
    }
}

/// Fotnotsdefinitionen med en ändring och grannarna i ett block av definitioner (se `def_window`).
struct DefWin {
    /// Början på definitionen före (`None` om ändringen står i blockets första definition).
    prev: Option<usize>,
    /// Början på definitionen med ändringen.
    start: usize,
    /// Början på definitionen efter, och på den efter den (`None` vid blockets slut).
    next: Option<usize>,
    after: Option<usize>,
    /// Blockets slut (början på blankraden efter det, eller textens slut).
    block_end: usize,
}

/// Står ändringen `pre..end` (i `t`, analyserad som `a`) i ett block av fotnotsdefinitioner utan tomrader emellan? Varje
/// definitionsrad börjar en ny definition, så det räcker att tolka om definitionen (eller definitionerna) med ändringen och
/// en definition på var sida som kanariefåglar: texten utanför tolkas likadant. Ändringar som inte står i ett sådant block
/// (eller där gränserna är för långt bort) ger `None`.
fn def_window(t: &[u8], a: &Analysis, pre: usize, end: usize) -> Option<DefWin> {
    let first = line_start(t, pre);
    if first >= t.len() || blank(t, first, line_end(t, first)) {
        return None;
    }
    let mut start = first;
    while !def_start(t, a, start) {
        start = prev_line(t, start)?;
        if pre - start > MAX_DEF {
            return None;
        }
    }
    let mut prev = None;
    let mut l = start;
    while let Some(p) = prev_line(t, l) {
        if start - p > MAX_DEF {
            return None;
        }
        if def_start(t, a, p) {
            prev = Some(p);
            break;
        }
        l = p;
    }
    // Definitionen efter den sista raden som ändringen rör, och den efter den.
    let last = line_start(t, end.max(pre).min(t.len()));
    let mut block_end = t.len();
    let mut found: Vec<usize> = Vec::new();
    let mut ls = last;
    while found.len() < 2 {
        let n = line_end(t, ls) + 1;
        if n >= t.len() {
            block_end = t.len();
            break;
        }
        if blank(t, n, line_end(t, n)) {
            block_end = n;
            break;
        }
        if n - last > 2 * MAX_DEF {
            return None;
        }
        if def_start(t, a, n) {
            found.push(n);
        }
        ls = n;
    }
    Some(DefWin { prev, start, next: found.first().copied(), after: found.get(1).copied(), block_end })
}

/// Finns det en oavslutad (eller överflödigt avslutad) HTML-kommentar? En sådan sväljer text över blankrader och ändrar
/// tolkningen av stycken långt från ändringen.
#[cfg(test)]
fn html_comments_open(t: &str) -> bool {
    let b = t.as_bytes();
    t.match_indices("<!--").any(|(i, _)| !t[i..line_end(b, i)].contains("-->"))
}

/// HTML-block som kan fortsätta över blankrader (oavslutad kommentar, `<pre>`, `<script>` …) och därför gör att något
/// utanför fönstret kan tolkas annorlunda när en ändring avslutar eller öppnar dem.
fn html_ctx(w: &str) -> bool {
    if w.matches("<!--").count() != w.matches("-->").count() {
        return true;
    }
    if !w.contains('<') {
        return false;
    }
    let l = w.to_ascii_lowercase();
    ["<pre", "<script", "<style", "<textarea", "<?", "<![cdata["].iter().any(|m| l.contains(m))
}

/// Etiketterna i fotnotsdefinitionerna (`[^etikett]:` först på en rad) i `w`, i ordning.
fn def_labels(w: &str) -> Vec<&str> {
    w.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("[^")?;
            let (label, after) = rest.split_once(']')?;
            after.starts_with(':').then_some(label)
        })
        .collect()
}

/// Bitarna i `pieces` (sorterade, utan glapp) klippta till `from..to`.
fn clip(pieces: &[Piece], from: usize, to: usize) -> Vec<Piece> {
    let i0 = pieces.partition_point(|p| p.range.end <= from);
    let mut out = Vec::new();
    for p in &pieces[i0..] {
        if p.range.start >= to {
            break;
        }
        let mut q = p.clone();
        q.range = p.range.start.max(from)..p.range.end.min(to);
        out.push(q);
    }
    out
}

/// Står alla kodstaket (```) i `t[..end]` först på en rad och är de ett jämnt antal (så att `end` ligger utanför ett kodblock)?
fn fences_before_are_balanced(t: &str, end: usize) -> bool {
    let mut n = 0;
    for (i, _) in t[..end].match_indices("```") {
        let ls = line_start(t.as_bytes(), i);
        if t[ls..i].len() > 3 || !t[ls..i].bytes().all(|c| c == b' ') {
            return false;
        }
        n += 1;
    }
    n % 2 == 0
}

/// Finns det referenslänkar (`[namn]: adress`)? De ändrar tolkningen av `[text][namn]` var som helst i dokumentet.
/// Fotnotsdefinitioner (`[^n]: text`) räknas inte.
fn has_link_defs(text: &str) -> bool {
    let b = text.as_bytes();
    text.match_indices("]:").any(|(i, _)| !b[line_start(b, i)..].starts_with(b"[^"))
}

/// Finns det text (utanför grundformatsraden på första raden) i `prefix`? Avsnittsscanningen räknar en avsnittsrad
/// som första avsnittet om ingen text föregår den.
fn has_content_before(prefix: &str) -> bool {
    prefix.split_inclusive('\n').enumerate().any(|(i, l)| {
        let t = l.trim();
        !t.is_empty() && !(i == 0 && super::markup::DocDefaults::from_comment(t).is_some())
    })
}

/// Konstruktioner vars tolkning beror på resten av dokumentet: då görs full analys.
fn risky(s: &str) -> bool {
    if s.contains("```") || s.contains("~~~") || has_link_defs(s) {
        return true;
    }
    // Indragen kod och fortsättningsrader i listor.
    s.starts_with("    ") || s.starts_with('\t') || s.contains("\n    ") || s.contains("\n\t")
}

/// Etiketterna i fotnotsreferenserna (`[^etikett]`) i `w`; `None` om någon är ofullständig eller konstig.
fn note_labels(w: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for (i, _) in w.match_indices("[^") {
        let rest = &w[i + 2..];
        let end = rest.find(']')?;
        let l = &rest[..end];
        if l.is_empty() || l.len() > 40 || l.chars().any(|c| c.is_whitespace() || matches!(c, '[' | ']' | '(' | ')' | '\\')) {
            return None;
        }
        out.push(l.to_owned());
    }
    Some(out)
}

/// Finns definitionen `[^etikett]:` först på en rad i texten? (Definitioner står oftast sist: sök bakifrån.)
fn has_def(t: &str, label: &str) -> bool {
    let pat = format!("\n[^{label}]:");
    t.starts_with(&pat[1..]) || t.rfind(&pat).is_some()
}

fn shifted(p: &Piece, delta: isize, base: usize) -> Piece {
    let mut q = p.clone();
    let mv = |x: usize| (x as isize + delta) as usize + base;
    q.range = mv(p.range.start)..mv(p.range.end);
    q
}

/// Mätningar (`bench`): slår av den inkrementella analysen för att jämföra med full analys vid varje ändring.
#[cfg(test)]
pub static DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    pub static WHY: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Avstår från den inkrementella vägen (i tester noteras varför).
fn decline(_why: usize) -> Option<Delta> {
    #[cfg(test)]
    WHY.with(|w| w.borrow_mut().push(_why));
    None
}

fn incremental(old_text: &str, old: &mut Analysis, new_text: &str, min_bytes: usize, facts: &mut Option<Facts>) -> Option<Delta> {
    if old_text.len() < min_bytes || new_text.len() < min_bytes {
        return decline(1);
    }
    // Kodstaket och referenslänkar påverkar tolkningen över hela dokumentet (fotnotsdefinitioner `[^n]: text` gör inte det).
    let (ob, nb) = (old_text.as_bytes(), new_text.as_bytes());
    let pre = common_prefix(ob, nb);
    if pre == ob.len() && pre == nb.len() {
        return Some(Delta { ws: new_text.len(), we_old: new_text.len(), we_new: new_text.len(), same_images: true, ..Default::default() });
    }
    let suf = common_suffix(ob, nb, ob.len().min(nb.len()) - pre);
    let (old_changed, new_changed) = (pre..ob.len() - suf, pre..nb.len() - suf);
    if old_changed.len() > MAX_EDIT || new_changed.len() > MAX_EDIT {
        return decline(2);
    }
    // Dokumentets grundformat står på första raden, som en liten ändring kan ändra (en vanlig första rad går bra).
    let first_line_end = line_end(ob, 0);
    if pre <= first_line_end {
        let defaults_line = |t: &[u8], e: usize| std::str::from_utf8(&t[..e]).is_ok_and(|l| DocDefaults::from_comment(l.trim()).is_some());
        if defaults_line(ob, first_line_end) || defaults_line(nb, line_end(nb, 0)) {
            return decline(3);
        }
    }
    // Ett första avsnitt som tagit över en avsnittsrad kan ändras av text som skrivs före raden.
    if old.sections.first().is_some_and(|s| s.marker.is_some()) {
        return decline(14);
    }
    let delta = nb.len() as isize - ob.len() as isize;

    // Stycket efter: hoppa över blankraderna och ta nästa stycke.
    let next_end = |from: usize| -> Option<usize> {
        if from >= ob.len() {
            return Some(ob.len());
        }
        let mut ls = line_start(ob, from);
        while ls < ob.len() && blank(ob, ls, line_end(ob, ls)) {
            ls = line_end(ob, ls) + 1;
        }
        if ls >= ob.len() { Some(ob.len()) } else { para_end(ob, ls) }
    };
    // Kanariestycket efter `from` (slutet på ett stycke): nästa stycke, eller bara dess första definition om det är ett
    // block av fotnotsdefinitioner (Word-läsaren skriver dem utan tomrader emellan, så blocket kan vara hela slutet av
    // dokumentet).
    let next_unit = |from: usize| -> Option<usize> {
        match first_def_end(ob, old, from) {
            Some(e) => Some(e),
            None => next_end(from),
        }
    };
    // Kanariestycket före stycket som börjar vid `at`: föregående stycke, eller dess sista definition.
    let prev_unit = |at: usize| -> Option<usize> {
        if at == 0 {
            return Some(0);
        }
        match last_def_start(ob, old, at) {
            Some(l) => Some(l),
            None => para_start(ob, at - 1),
        }
    };
    // Fönstret: stycket med ändringen och ett stycke på var sida (kanariefåglar); i ett block av fotnotsdefinitioner
    // definitionen med ändringen och en definition på var sida.
    let (mut ws, mut we_old) = match def_window(ob, old, pre, old_changed.end) {
        Some(DefWin { prev, start, next, after, block_end }) => {
            let ws = match prev {
                Some(p) => p,
                None => prev_unit(start)?,
            };
            let we = match (next, after) {
                (Some(_), Some(a)) => a,
                (Some(_), None) => block_end,
                (None, _) => next_unit(block_end)?,
            };
            (ws, we)
        }
        None => {
            let edit_para = para_start(ob, pre)?;
            let ws = prev_unit(edit_para)?;
            let mid_end = para_end(ob, old_changed.end.min(ob.len().saturating_sub(1)).max(pre))?.max(old_changed.end);
            (ws, next_unit(mid_end)?)
        }
    };
    // `<div align>` runt stycken (justerad text i Word-dokument): fönstret ska innehålla hela `<div>`-block, annars
    // syns inte justeringen vid tolkning av fönstret för sig. Det vidgas till stycken tills blocken är hela.
    for _ in 0..8 {
        let w = &old_text[ws..we_old];
        let (open, close) = (w.matches("<div").count(), w.matches("</div").count());
        if open == close {
            break;
        }
        if open > close {
            if we_old >= ob.len() {
                return decline(16);
            }
            we_old = next_end(we_old)?;
        } else if ws == 0 {
            return decline(16);
        } else {
            ws = para_start(ob, ws - 1)?;
        }
    }
    if we_old - ws > MAX_WINDOW {
        return decline(4);
    }
    let we_new = (we_old as isize + delta) as usize;
    if we_new < ws || we_new > nb.len() || !new_text.is_char_boundary(ws) || !new_text.is_char_boundary(we_new) {
        return decline(5);
    }
    let (old_win, new_win) = (&old_text[ws..we_old], &new_text[ws..we_new]);
    if risky(old_win) || risky(new_win) {
        return decline(6);
    }
    // Tre tilda, oavslutade kommentarer och (när fönstret har hakparenteser) referenslänkar ändrar tolkningen över hela dokumentet.
    let f_old = facts.take().unwrap_or_else(|| Facts::of(old_text));
    let f_new = f_old.swap(&Facts::of(old_win), &Facts::of(new_win));
    let brackets = |w: &str| w.contains(['[', ']']);
    if f_new.tildes > 0 || f_new.open_comments > 0 || (f_new.link_defs > 0 && (brackets(old_win) || brackets(new_win))) {
        return decline(13);
    }
    // Finns kodstaket i dokumentet: alla ska stå först på en rad och vara ett jämnt antal (annars vet vi inte vad som är kod),
    // ett jämnt antal ska ligga före fönstret (fönstret är då inte i ett kodblock) och fönstret får inte ha något.
    if f_new.fences > 0 && (f_new.fences != f_new.fence_lines || f_new.fences % 2 != 0 || !fences_before_are_balanced(old_text, ws)) {
        return decline(13);
    }
    // Giltigheten av en fotnotsreferens beror på vilka definitioner som finns: får fönstret andra definitionsetiketter
    // (en definition har lagts till, tagits bort eller bytt etikett) ger det full analys, men text i en definition går bra.
    if def_labels(old_win) != def_labels(new_win) {
        return decline(17);
    }
    // Avsnittsrader i fönstret: de får inte ändras eller beröras av ändringen (då räknas avsnitten om med full analys);
    // annars flyttas de med texten.
    let sec_lines = |w: &str| -> Vec<(usize, String)> {
        let mut off = 0;
        w.split_inclusive('\n').filter_map(|l| {
            let at = off;
            off += l.len();
            let t = l.trim();
            (t.starts_with("<!--") && t.contains("avsnitt")).then(|| (at, t.to_owned()))
        }).collect()
    };
    let (sl_old, sl_new) = (sec_lines(old_win), sec_lines(new_win));
    if sl_old.len() != sl_new.len() || sl_old.iter().zip(&sl_new).any(|(o, n)| o.1 != n.1) {
        return decline(3);
    }
    // Ändringen får inte röra en avsnittsrad (raden ska ligga helt före eller helt efter).
    if sl_old.iter().any(|(at, l)| {
        let (s, e) = (ws + at, ws + at + l.len() + 2);
        pre < e && s < old_changed.end.max(pre + 1)
    }) {
        return decline(3);
    }
    // Texten före första avsnittsraden får inte ha blivit tom (då blir raden det första avsnittet).
    if let Some(m) = old.sections.iter().skip(1).find_map(|s| s.marker.as_ref()).map(|m| m.start) {
        if pre < m && !has_content_before(&new_text[..(m as isize + delta) as usize]) {
            return decline(14);
        }
    }
    // Fönstret slutar mellan stycken, men bitar slås ihop över styckegränser (vanlig text i flera stycken är en bit).
    // De gamla bitarna i fönstret klipps därför vid kanterna; delarna utanför behålls som egna bitar.
    let i0 = old.pieces.partition_point(|p| p.range.end <= ws);
    let i1 = old.pieces.partition_point(|p| p.range.start < we_old);
    if i0 >= i1 {
        return decline(7);
    }
    let mut ow: Vec<Piece> = old.pieces[i0..i1].to_vec();
    let head = (old.pieces[i0].range.start < ws).then(|| Piece { range: old.pieces[i0].range.start..ws, ..old.pieces[i0].clone() });
    let tail_piece = (old.pieces[i1 - 1].range.end > we_old)
        .then(|| shifted(&Piece { range: we_old..old.pieces[i1 - 1].range.end, ..old.pieces[i1 - 1].clone() }, delta, 0));
    if let Some(f) = ow.first_mut() {
        f.range.start = f.range.start.max(ws);
    }
    if let Some(l) = ow.last_mut() {
        l.range.end = l.range.end.min(we_old);
    }
    // Fortfarande ohela `<div>`-block (i den nya texten eller efter fönstrets vidgning): full analys.
    let unbalanced = |w: &str| w.matches("<div").count() != w.matches("</div").count();
    if unbalanced(old_win) || unbalanced(new_win) {
        return decline(16);
    }
    if html_ctx(old_win) || html_ctx(new_win) {
        return decline(18);
    }
    if ow.iter().any(|p| p.align != markup::Align::Left) && !old_win.contains("align") {
        return decline(19);
    }
    // Fotnotsreferenser (`[^n]`) tolkas bara om definitionen finns i dokumentet: fönstret tolkas med en definition per
    // etikett efter sig (som finns i texten, annars full analys) och bitarna efter fönstret kastas.
    let (Some(l_old), Some(l_new)) = (note_labels(old_win), note_labels(new_win)) else { return decline(20) };
    let mut labels = l_old;
    labels.extend(l_new);
    labels.sort_unstable();
    labels.dedup();
    if labels.iter().any(|l| !has_def(new_text, l)) {
        return decline(20);
    }
    let mut mid: Vec<Piece> = if labels.is_empty() {
        markup::pieces_of(new_win)
    } else {
        // Etiketter som fönstret självt definierar behöver ingen tillfällig definition.
        let own = def_labels(new_win);
        let src = format!("{new_win}{}", labels.iter().filter(|l| !own.contains(&l.as_str())).map(|l| format!("\n\n[^{l}]: x")).collect::<String>());
        let mut v = markup::pieces_of(&src);
        v.retain(|p| p.range.start < new_win.len());
        if let Some(l) = v.last_mut() {
            l.range.end = l.range.end.min(new_win.len());
        }
        v
    };
    for p in &mut mid {
        *p = shifted(p, 0, ws);
    }
    // Kontroll: bitarna i raderna före och efter raden med ändringen ska bli desamma som i den gamla analysen (klippta
    // vid raderna, så att en lång bit som sträcker sig över dem också jämförs); annars har ändringen eller fönstrets
    // kant påverkat något som sträcker sig över styckegränsen.
    let before_end = line_start(ob, pre).max(ws);
    if clip(&ow, ws, before_end) != clip(&mid, ws, before_end) {
        return decline(8);
    }
    let after_old_start = (line_end(ob, old_changed.end.min(ob.len())) + 1).min(we_old);
    let after_new_start = (after_old_start as isize + delta) as usize;
    let after_old: Vec<Piece> = clip(&ow, after_old_start, we_old).iter().map(|p| shifted(p, delta, 0)).collect();
    if after_new_start > we_new || after_old != clip(&mid, after_new_start, we_new) {
        return decline(10);
    }
    // Täckning utan glapp (som `analyze` ger); annars full analys.
    if mid.first().is_none_or(|p| p.range.start != ws) || mid.last().is_none_or(|p| p.range.end != we_new) {
        return decline(12);
    }
    let delta_info = Delta {
        ws,
        we_old,
        we_new,
        touched_table: ow.iter().chain(mid.iter()).any(|p| p.block == markup::Block::Table),
        same_images: ow.iter().filter_map(|p| p.image.as_ref()).eq(mid.iter().filter_map(|p| p.image.as_ref())),
        notes_changed: {
            fn labels(v: &[Piece]) -> Vec<(bool, &str)> {
                let mut l: Vec<(bool, &str)> = v
                    .iter()
                    .flat_map(|p| p.footnote_ref.as_deref().map(|x| (true, x)).into_iter().chain(p.footnote_def.as_deref().map(|x| (false, x))))
                    .collect();
                l.sort_unstable();
                l.dedup();
                l
            }
            labels(&ow) != labels(&mid)
        },
        fonts: super::layout::needed_fonts(&Analysis { pieces: mid.clone(), defaults: old.defaults.clone(), sections: Vec::new() }),
        toc_delta: super::nav::toc_sig_diff(&super::nav::toc_sig_in(new_text, &mid), &super::nav::toc_sig_in(old_text, &ow)),
    };
    // Först nu ändras analysen: bitarna efter fönstret flyttas på plats, fönstrets byts ut.
    if delta != 0 {
        for p in &mut old.pieces[i1..] {
            p.range = (p.range.start as isize + delta) as usize..(p.range.end as isize + delta) as usize;
        }
    }
    let (n_head, n_mid) = (usize::from(head.is_some()), mid.len());
    let mut repl: Vec<Piece> = head.into_iter().chain(mid).chain(tail_piece).collect();
    let n_tail = repl.len() - n_head - n_mid;
    // Bitar med samma egenskaper bredvid varandra är en bit i `analyze`: slå ihop skarvarna.
    let same = |a: &Piece, b: &Piece| a.range.end == b.range.start && Piece { range: a.range.clone(), ..b.clone() } == *a;
    let joint = if n_tail == 1 { n_head + n_mid } else { repl.len() };
    let merge_right = if joint < repl.len() { same(&repl[joint - 1], &repl[joint]) } else { old.pieces.get(i1).is_some_and(|nx| same(&repl[repl.len() - 1], nx)) };
    let mut i1 = i1;
    if merge_right {
        if joint < repl.len() {
            let end = repl[joint].range.end;
            repl[joint - 1].range.end = end;
            repl.remove(joint);
        } else {
            let end = old.pieces[i1].range.end;
            let last = repl.len() - 1;
            repl[last].range.end = end;
            i1 += 1;
        }
    }
    let first_mid = n_head;
    let left_last = if n_head == 1 { Some(repl[0].clone()) } else { i0.checked_sub(1).map(|k| old.pieces[k].clone()) };
    if left_last.as_ref().is_some_and(|l| same(l, &repl[first_mid])) {
        let end = repl[first_mid].range.end;
        if n_head == 1 {
            repl[0].range.end = end;
            repl.remove(1);
        } else if let Some(k) = i0.checked_sub(1) {
            old.pieces[k].range.end = end;
            repl.remove(0);
        }
    }
    old.pieces.splice(i0..i1, repl);
    *facts = Some(f_new);
    // Avsnitten efter ändringen flyttas med texten (avsnittsrader i fönstret är oförändrade, se ovan).
    if delta != 0 {
        for sec in old.sections.iter_mut().filter(|s| s.start >= old_changed.end.max(pre + 1) && s.marker.is_some()) {
            sec.start = (sec.start as isize + delta) as usize;
            if let Some(m) = sec.marker.as_mut() {
                *m = (m.start as isize + delta) as usize..(m.end as isize + delta) as usize;
            }
        }
    }
    Some(delta_info)
}

/// Hjälp för tester och mätning: ändringens byteintervall mellan två texter.
#[allow(dead_code)]
pub fn changed_range(old: &str, new: &str) -> Range<usize> {
    let pre = common_prefix(old.as_bytes(), new.as_bytes());
    let suf = common_suffix(old.as_bytes(), new.as_bytes(), old.len().min(new.len()) - pre);
    pre..new.len() - suf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::bench::Rng;

    fn doc(r: &mut Rng, paras: usize, rich: bool) -> String {
        // Ibland börjar dokumentet med text i stället för grundformatsraden.
        let mut s = if r.below(4) == 0 { String::from("Första raden med text\n\n") } else { String::from("<!-- kladd: sida=a4 -->\n\n") };
        let words = ["alfa", "beta", "gamma", "delta", "åäö", "text", "rad", "stycke"];
        let mut notes = 0;
        for i in 0..paras {
            let kind = if rich { r.below(18) } else { r.below(4) };
            match kind {
                0 | 1 | 2 => {
                    for k in 0..(3 + r.below(12)) {
                        let w = words[r.below(words.len())];
                        match r.below(12) {
                            0 => s.push_str(&format!("**{w}** ")),
                            1 => s.push_str(&format!("*{w}* ")),
                            2 => s.push_str(&format!("<u>{w}</u> ")),
                            3 => s.push_str(&format!("[{w}](http://x.se/{k}) ")),
                            _ => s.push_str(&format!("{w} ")),
                        }
                    }
                }
                3 => s.push_str(&format!("## Rubrik {i}")),
                4 => s.push_str("- punkt ett\n- punkt två\n- punkt tre"),
                5 => s.push_str("1. först\n2. sedan"),
                6 => s.push_str("| a | b |\n| --- | --- |\n| 1 | 2 |"),
                7 => s.push_str("> citat rad\n> mer"),
                8 => s.push_str("kod ** här"),
                9 => s.push_str("<span style=\"color:#ff0000\">röd text</span> och mer"),
                10 => s.push_str("<div align=\"center\">\n\nmittad\n\n</div>"),
                11 => s.push_str("text med fotnot"),
                12 => s.push_str("    indragen kod"),
                14 | 15 => {
                    notes += 1;
                    s.push_str(&format!("text med fotnot[^{notes}] och mer text"));
                }
                16 => s.push_str("<!-- kladd: avsnitt; columns: 2; type: continuous -->"),
                17 if rich && r.below(3) == 0 => s.push_str("```\nkod ** här\n\nmer kod\n```"),
                17 if rich && r.below(3) == 0 => s.push_str("[namn]: http://x.se/y"),
                _ => s.push_str("![bild](a.png)"),
            }
            s.push_str("\n\n");
        }
        for n in 1..=notes {
            s.push_str(&format!("[^{n}]: Fotnot {n} med text.\n\n"));
        }
        s
    }

    /// Slumpmässiga små ändringar: antingen blir resultatet exakt `analyze`, eller så avstår
    /// inkrementell analys (`None`).
    fn text_around(t: &str, p: &[Piece], i: usize) -> String {
        t.get(p[i].range.clone()).unwrap_or("").to_owned()
    }

    /// Fotnotsdefinitionerna som Word-läsaren skriver dem: ett block utan tomrader, ibland med fortsättningsrader, en
    /// referens i en definition och slutnoter efter.
    fn word_notes(r: &mut Rng, n: usize) -> String {
        let mut s = String::new();
        for k in 1..=n {
            s.push_str(&format!("[^{k}]: Fotnot {k} med *text* och <u>mer</u> text"));
            match r.below(10) {
                0 => s.push_str("\nfortsättning på noten"),
                1 => s.push_str(" och en referens[^1]"),
                2 => s.push_str(" **fet"),
                _ => {}
            }
            s.push('\n');
        }
        if r.below(2) == 0 {
            s.push_str("\n[^ei]: Slutnot.\n[^eii]: Slutnot två.\n");
        }
        s
    }

    /// Som `doc`, men med fotnotsdefinitionerna som ett block (Word-dialekten).
    fn doc_word(r: &mut Rng, paras: usize, rich: bool) -> String {
        let text = doc(r, paras, rich);
        let cut = text.find("[^1]: ").unwrap_or(text.len());
        let n = text[cut..].matches("]: ").count();
        format!("{}{}", &text[..cut], word_notes(r, n))
    }

    fn fuzz(rich: bool, seed: u64) -> (usize, usize) {
        fuzz_with(rich, seed, false)
    }

    /// `word`: fotnotsdefinitionerna står i ett block och två tredjedelar av ändringarna görs i eller nära det.
    fn fuzz_with(rich: bool, seed: u64, word: bool) -> (usize, usize) {
        let mut r = Rng(seed);
        let mut text = if word { doc_word(&mut r, 120, rich) } else { doc(&mut r, 120, rich) };
        let mut a = markup::analyze(&text);
        let mut facts: Option<Facts> = None;
        let (mut used, mut declined) = (0, 0);
        let edits = ["x", " ", "\n", "\n\n", "**", "*", "<u>", "</u>", "`", "```", "<", ">", "[", "]", "(", ")", "#", "- ", "| ", "ä", "<span>", "</span>", "<div>", "</div>", "\n    ", "[^", "]:"];
        for _ in 0..400 {
            let mut t = text.clone();
            let at = {
                let focus = t.find("\n[^1]:").filter(|_| word && r.below(3) != 0);
                let mut p = match focus {
                    Some(f) => f.saturating_sub(300) + r.below(t.len() - f.saturating_sub(300)),
                    None => r.below(t.len()),
                };
                while !t.is_char_boundary(p) {
                    p -= 1;
                }
                p
            };
            match r.below(3) {
                0 => t.insert_str(at, edits[r.below(edits.len())]),
                1 => {
                    let mut e = (at + 1 + r.below(6)).min(t.len());
                    while !t.is_char_boundary(e) {
                        e += 1;
                    }
                    t.replace_range(at..e, "");
                }
                _ => t.insert(at, 'x'),
            }
            // En ändring som lägger in kodstaket eller referenslänkar i ett dokument utan sådana kontrolleras men behålls inte
            // (annars avstår alla senare ändringar).
            let global = |x: &str| ["```", "~~~"].iter().any(|m| x.contains(m)) || has_link_defs(x) || html_comments_open(x);
            let poison = global(&t) && !global(&text);
            let full = markup::analyze(&t);
            let mut inc = a.clone();
            match incremental(&text, &mut inc, &t, 0, &mut facts) {
                Some(_) => {
                    if inc.pieces != full.pieces {
                        let i = inc.pieces.iter().zip(&full.pieces).position(|(a, b)| a != b).unwrap_or(0);
                        let lo = text_around(&t, &full.pieces, i);
                        panic!("första skillnad vid bit {i} ({} vs {} bitar): inc {:?} full {:?}; text {lo:?}; ändring {:?} i {:?}", inc.pieces.len(), full.pieces.len(), inc.pieces.get(i), full.pieces.get(i), changed_range(&text, &t), &t[at.saturating_sub(120)..(at + 120).min(t.len())]);
                    }
                    assert_eq!(inc.pieces, full.pieces, "avvikelse efter ändring vid {at} ({:?})", changed_range(&text, &t));
                    assert_eq!(inc.defaults, full.defaults);
                    assert_eq!(inc.sections, full.sections);
                    // De uppdaterade förekomsterna är desamma som en ny räkning av hela texten.
                    assert_eq!(facts, Some(Facts::of(&t)), "förekomster efter ändring vid {at} ({:?}): {:?}", changed_range(&text, &t), &t[at.saturating_sub(60)..(at + 60).min(t.len())]);
                    used += 1;
                }
                None => {
                    declined += 1;
                    facts = None;
                }
            }
            if poison && !rich {
                facts = None;
                continue;
            }
            text = t;
            a = full;
        }
        WHY.with(|w| {
            let mut c = std::collections::BTreeMap::new();
            for x in w.borrow().iter() {
                *c.entry(*x).or_insert(0) += 1;
            }
            eprintln!("avstår: {c:?}");
        });
        (used, declined)
    }

    #[test]
    fn incremental_equals_full_on_plain_documents() {
        let (used, _) = fuzz(false, 11);
        assert!(used > 100, "den inkrementella vägen används ({used})");
    }

    #[test]
    fn incremental_equals_full_on_rich_documents() {
        let n: u64 = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        for seed in [3, 5, 8, 13, 21, 34].into_iter().chain(100..100 + n) {
            fuzz(true, seed);
        }
    }

    /// Ändringar i och nära ett block av fotnotsdefinitioner utan tomrader (Word-dialekten): fönstret är då bara
    /// definitionen med ändringen och grannarna (`def_window`), och resultatet ska vara detsamma som `analyze`.
    #[test]
    fn incremental_equals_full_in_footnote_definition_blocks() {
        let n: u64 = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let (mut used, mut declined) = (0, 0);
        for seed in [2, 7, 19, 31].into_iter().chain(500..500 + n) {
            let (u, d) = fuzz_with(seed % 2 == 1, seed, true);
            used += u;
            declined += d;
        }
        // Dokumenten har också indragen kod, kodstaket och ohela `<div>` (som alltid ger full analys).
        assert!(used * 8 > used + declined, "den inkrementella vägen används ({used} av {})", used + declined);
    }

    /// Text i en fotnotsdefinition i ett stort block tolkar bara om definitionen och grannarna, och stycket före
    /// blocket tar bara blockets första definition som kanariefågel.
    #[test]
    fn footnote_block_window_is_one_definition_on_each_side() {
        let mut r = Rng(3);
        let mut text = doc(&mut r, 3000, false);
        let cut = text.find("[^1]: ").unwrap_or(text.len());
        text.truncate(cut);
        text.push_str("Sista stycket i brödtexten med en referens[^1] och mer.\n\n");
        let refs = (1..=2000).map(|k| format!("ref[^{k}] ")).collect::<String>();
        text.insert_str(text.len() / 2, &format!("{refs}\n\n"));
        for k in 1..=2000 {
            text.push_str(&format!("[^{k}]: Fotnot {k} med lite text i.\n"));
        }
        // Slutnoterna (synliga definitioner) i ett eget block efter.
        text.push('\n');
        for k in 1..=300 {
            text.push_str(&format!("[^{}]: Slutnot {k} med text.\n", markup::endnote_label(k)));
        }
        let a = markup::analyze(&text);
        let at = text.find(&format!("[^{}]: Slutnot", markup::endnote_label(299))).unwrap() + 20;
        let mut t = text.clone();
        t.insert(at, 'x');
        let mut inc = a.clone();
        let d = analyze_incremental(&text, &mut inc, &t).expect("inkrementell");
        assert!(d.we_old - d.ws < 200, "fönstret i slutnoterna är tre definitioner ({})", d.we_old - d.ws);
        assert_eq!(inc.pieces, markup::analyze(&t).pieces);
        let at = text.find("[^1000]: Fotnot").unwrap() + 15;
        let mut t = text.clone();
        t.insert(at, 'x');
        let mut inc = a.clone();
        let d = analyze_incremental(&text, &mut inc, &t).expect("inkrementell");
        assert!(d.we_old - d.ws < 200, "fönstret är tre definitioner, inte hela blocket ({})", d.we_old - d.ws);
        assert_eq!(inc.pieces, markup::analyze(&t).pieces);
        let at = text.find("Sista stycket").unwrap() + 3;
        let mut t = text.clone();
        t.insert(at, 'x');
        let mut inc = a.clone();
        let d = analyze_incremental(&text, &mut inc, &t).expect("inkrementell");
        assert!(d.we_old - at < 200, "kanariefågeln efter är första definitionen ({})", d.we_old - at);
        assert_eq!(inc.pieces, markup::analyze(&t).pieces);
    }

    #[test]
    fn small_documents_and_big_edits_use_full_analysis() {
        let mut r = Rng(1);
        let text = doc(&mut r, 20, false);
        let a = markup::analyze(&text);
        assert!(analyze_incremental(&text, &mut a.clone(), &format!("{text}x")).is_none(), "under tröskeln");
        let big = doc(&mut r, 8000, false);
        let ab = markup::analyze(&big);
        assert!(big.len() > MIN_BYTES);
        let mut t = big.clone();
        t.insert_str(big.len() / 2, "x");
        assert!(analyze_incremental(&big, &mut ab.clone(), &t).is_some());
        let mut u = big.clone();
        u.insert_str(big.len() / 2, &"y".repeat(MAX_EDIT + 1));
        assert!(analyze_incremental(&big, &mut ab.clone(), &u).is_none(), "stor ändring");
        // Ändring på första raden: grundformatet gäller hela dokumentet (full analys), en vanlig första rad går bra.
        let mut f = big.clone();
        f.insert(3, 'q');
        let mut inc = ab.clone();
        let got = analyze_incremental(&big, &mut inc, &f);
        if big.starts_with("<!-- kladd") {
            assert!(got.is_none());
        } else {
            assert!(got.is_some());
            assert_eq!(inc.pieces, markup::analyze(&f).pieces);
        }
        let defaults_first = format!("<!-- kladd: sida=a4 -->\n\n{}", big.split_once("\n\n").unwrap().1);
        let mut f = defaults_first.clone();
        f.insert(3, 'q');
        assert!(analyze_incremental(&defaults_first, &mut markup::analyze(&defaults_first), &f).is_none());
    }

    /// Ett dokument med kodblock (Word-dokument med kodstycken) ska ändå ha inkrementell analys utanför kodblocken.
    #[test]
    fn code_blocks_keep_the_incremental_path() {
        let mut r = Rng(5);
        let mut text = doc(&mut r, 6000, false);
        // Kodblock tidigt, i mitten och sist.
        for frac in [0.1, 0.5, 0.9] {
            let mut p = (text.len() as f64 * frac) as usize;
            while !text.is_char_boundary(p) {
                p += 1;
            }
            let at = text[p..].find("\n\n").unwrap() + p + 2;
            text.insert_str(at, "```rust\nfn main() {}\n\n    let x = 1;\n```\n\n");
        }
        let a = markup::analyze(&text);
        let fences: Vec<usize> = text.match_indices("```").map(|(i, _)| i).collect();
        assert_eq!(fences.len(), 6);
        // Utanför kodblocken: före, mellan och efter.
        for at in [fences[0] / 2, (fences[1] + fences[2]) / 2, (fences[3] + fences[4]) / 2, (fences[5] + text.len()) / 2] {
            let mut p = at;
            while !text.is_char_boundary(p) || text.as_bytes()[p] == b'\n' || text.as_bytes()[p - 1] == b'\n' {
                p += 1;
            }
            let mut t = text.clone();
            t.insert(p, 'x');
            let mut inc = a.clone();
            assert!(analyze_incremental(&text, &mut inc, &t).is_some(), "vid {at}: ska vara inkrementell ({:?})", WHY.with(|w| w.borrow().last().copied()));
            assert_eq!(inc.pieces, markup::analyze(&t).pieces, "vid {at}");
        }
        // I kodblocket (och intill dess staket) gäller full analys.
        let mut t = text.clone();
        t.insert(fences[3] + 12, 'x');
        assert!(analyze_incremental(&text, &mut a.clone(), &t).is_none());
        // Ett staket som inte står först på raden gör att kod och text inte går att skilja åt utan full analys.
        let mut t = text.clone();
        let (i, _) = t.match_indices("Rubrik").next().unwrap_or((20_000, ""));
        t.insert_str(i, "text ```kod``` ");
        let mut u = t.clone();
        u.insert(t.len() / 2, 'x');
        assert!(analyze_incremental(&t, &mut markup::analyze(&t), &u).is_none());
    }

    /// Referenslänkar (`[namn]: adress`) gäller hela dokumentet, men bara ett fönster med hakparenteser kan påverkas.
    #[test]
    fn link_definitions_only_block_windows_with_brackets() {
        // Text utan hakparenteser, och en referenslänk sist.
        let mut text = String::from("<!-- kladd: sida=a4 -->\n\n");
        for i in 0..4000 {
            text.push_str(&format!("Stycke {i} med vanlig text utan någon länk eller parentes, bara ord som fyller raden.\n\n"));
        }
        text.push_str("[namn]: http://x.se/y\n\n");
        assert!(text.len() > MIN_BYTES);
        let a = markup::analyze(&text);
        let p = text.find("Stycke 1500").unwrap() + 8;
        let mut t = text.clone();
        t.insert(p, 'x');
        let mut inc = a.clone();
        assert!(analyze_incremental(&text, &mut inc, &t).is_some(), "{:?}", WHY.with(|w| w.borrow().last().copied()));
        assert_eq!(inc.pieces, markup::analyze(&t).pieces);
        // Med hakparenteser i fönstret kan en referens ha tillkommit eller försvunnit.
        let mut t = text.clone();
        t.insert_str(p, "[namn]");
        assert!(analyze_incremental(&text, &mut a.clone(), &t).is_none());
    }

    #[test]
    fn open_fence_across_the_boundary_falls_back() {
        let mut r = Rng(2);
        let mut text = doc(&mut r, 8000, false);
        let mid = text.len() / 2;
        let at = text[mid..].find("\n\n").unwrap() + mid + 2;
        text.insert_str(at, "kod\n\n");
        let a = markup::analyze(&text);
        // Att öppna ett kodstaket påverkar allt efter det: aldrig en inkrementell väg.
        let mut t = text.clone();
        t.insert_str(at, "```");
        assert!(analyze_incremental(&text, &mut a.clone(), &t).is_none());
    }

    /// Word-dokument har fotnoter och avsnitt: en vanlig skrivändring ska ändå vara inkrementell och ge exakt `analyze`.
    #[test]
    fn footnotes_and_sections_keep_the_incremental_path() {
        let mut r = Rng(9);
        let mut text = doc(&mut r, 6000, false);
        let mut mid = text.len() / 2;
        while !text.is_char_boundary(mid) {
            mid += 1;
        }
        let at = text[mid..].find("\n\n").unwrap() + mid + 2;
        text.insert_str(at, "Med fotnot[^1] här.\n\n<!-- kladd: avsnitt; columns: 2; type: continuous -->\n\nTvå spalter.\n\n<!-- kladd: avsnitt; type: continuous -->\n\n");
        text.push_str("[^1]: Fotnotstext.\n\n");
        let a = markup::analyze(&text);
        assert_eq!(a.sections.len(), 3);
        for frac in [0.1, 0.3, 0.45, 0.6, 0.9] {
            let mut p = (text.len() as f64 * frac) as usize;
            while !text.is_char_boundary(p) || text.as_bytes()[p] == b'\n' {
                p += 1;
            }
            let mut t = text.clone();
            t.insert(p, 'x');
            let mut inc = a.clone();
            let d = analyze_incremental(&text, &mut inc, &t);
            assert!(d.is_some(), "bit {frac}: ska vara inkrementell ({:?})", WHY.with(|w| w.borrow().iter().rev().take(3).copied().collect::<Vec<_>>()));
            let full = markup::analyze(&t);
            assert_eq!(inc.pieces, full.pieces, "bit {frac}");
            assert_eq!(inc.sections, full.sections, "avsnitten flyttas med texten ({frac})");
        }
        // Rör ändringen en avsnittsrad görs full analys (och en ändring vid en fotnotsreferens ger samma bitar som `analyze`).
        let fnr = text.find("[^1] här").unwrap();
        let mut t = text.clone();
        t.insert(fnr + 1, 'x');
        let mut inc = a.clone();
        if analyze_incremental(&text, &mut inc, &t).is_some() {
            assert_eq!(inc.pieces, markup::analyze(&t).pieces);
        }
        let sec = text.find("<!-- kladd: avsnitt; columns").unwrap();
        let mut t = text.clone();
        t.insert(sec + 6, 'x');
        assert!(analyze_incremental(&text, &mut a.clone(), &t).is_none());
        // Ett stycke bredvid en avsnittsrad (raden ligger i fönstret men rörs inte): avsnittet flyttas med texten.
        for at in [text[..sec - 2].rfind("\n\n").unwrap() + 3, text[sec..].find("\n\n").unwrap() + sec + 4] {
            let mut t = text.clone();
            t.insert(at, 'x');
            let mut inc = a.clone();
            assert!(analyze_incremental(&text, &mut inc, &t).is_some(), "ska vara inkrementell vid {at} ({:?})", WHY.with(|w| w.borrow().last().copied()));
            let full = markup::analyze(&t);
            assert_eq!(inc.pieces, full.pieces);
            assert_eq!(inc.sections, full.sections, "avsnitten vid {at}");
        }
        // Text i en fotnotsdefinition går bra (etiketterna är desamma); att ändra själva etiketten gör det inte.
        let def = text.find("Fotnotstext").unwrap();
        let mut t = text.clone();
        t.insert(def + 3, 'x');
        let mut inc = a.clone();
        assert!(analyze_incremental(&text, &mut inc, &t).is_some(), "text i definition: {:?}", WHY.with(|w| w.borrow().last().copied()));
        assert_eq!(inc.pieces, markup::analyze(&t).pieces);
        let mut t = text.clone();
        t.insert(text.rfind("[^1]:").unwrap() + 2, '2');
        assert!(analyze_incremental(&text, &mut a.clone(), &t).is_none(), "ny etikett");
        // Referenslänkar gäller hela dokumentet.
        let mut t = text.clone();
        t.push_str("[namn]: http://x.se\n\n");
        let mut u = t.clone();
        u.insert(at - 3, 'x');
        assert!(analyze_incremental(&t, &mut markup::analyze(&t), &u).is_none());
    }
}
