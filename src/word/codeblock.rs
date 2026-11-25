//! Inhägnade kodblock (```språk … ```) i dokumentläget: var de står, deras språk och färgning.
//!
//! Källtexten är oförändrad Markdown; blocken hittas ur analysens bitar. Färgningen (`codehl`)
//! ger bara färger per byteintervall och ändrar aldrig texten eller markörlägena. Inhägnadens
//! rader är dold markup (som andra markörer) och visas när markören står på dem.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use super::markup::{Analysis, Block as Blk};
use crate::codehl::{self, Class, Hl};

/// En färgad bit av ett kodblock (byteintervall i hela dokumentet).
#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub range: Range<usize>,
    pub class: Class,
    pub bold: bool,
    pub italic: bool,
}

/// Ett inhägnat kodblock.
#[derive(Debug)]
pub struct Block {
    /// Öppningsraden (inhägnaden och språket), utan radbrytning.
    pub open: Range<usize>,
    /// Koden: från raden efter öppningen till stängningsraden (med den sista radbrytningen).
    pub content: Range<usize>,
    /// Stängningsraden; saknas när blocket inte är avslutat.
    pub close: Option<Range<usize>>,
    /// Språket ur inhägnaden i gemener ("rust"); tomt utan språk.
    pub lang: String,
    key: OnceLock<u64>,
    segs: OnceLock<Arc<Vec<Seg>>>,
}

impl Block {
    /// Blockets hela intervall (öppning till stängning).
    pub fn whole(&self) -> Range<usize> {
        self.open.start..self.close.as_ref().map_or(self.content.end.max(self.open.end), |c| c.end)
    }

    /// Koden utan avslutande radbrytning (det som kopieras).
    pub fn code<'a>(&self, text: &'a str) -> &'a str {
        let c = text.get(self.content.clone()).unwrap_or("");
        let c = c.strip_suffix('\n').unwrap_or(c);
        c.strip_suffix('\r').unwrap_or(c)
    }

    /// Nyckeln för blockets diagram (```mermaid), under vilken vyn lägger diagrammets storlek bland bildstorlekarna.
    pub fn diagram_key(&self, text: &str) -> Option<String> {
        crate::mermaid::is_lang(&self.lang).then(|| format!("{}{:016x}", crate::mermaid::SRC, self.key(text)))
    }

    /// Innehållets signatur (språk och kod) för cachenycklar.
    pub fn key(&self, text: &str) -> u64 {
        *self.key.get_or_init(|| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.lang.hash(&mut h);
            text.get(self.content.clone()).hash(&mut h);
            h.finish()
        })
    }

    /// De färgade bitarna (absoluta byteintervall); tomt tills syntaxuppsättningen är inläst och för
    /// block utan känt språk. Resultatet sparas när färgningen är klar.
    pub fn segs(&self, text: &str) -> Arc<Vec<Seg>> {
        if let Some(s) = self.segs.get() {
            return s.clone();
        }
        let code = text.get(self.content.clone()).unwrap_or("");
        let hl = codehl::tokens(&self.lang, code);
        if matches!(hl, Hl::Pending) {
            return Arc::default();
        }
        let base = self.content.start;
        let v: Vec<Seg> = match &hl {
            Hl::Tokens(_) => codehl::segments(&hl, code)
                .into_iter()
                .map(|(r, class, bold, italic)| Seg { range: base + r.start..base + r.end, class, bold, italic })
                .collect(),
            _ => Vec::new(),
        };
        let v = Arc::new(v);
        let _ = self.segs.set(v.clone());
        v
    }
}

/// Cachenyckel för blockets utseende i fönstret `w`: kod, språk och om färgningen finns.
pub fn sig(cb: &[Block], text: &str, w: &Range<usize>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for b in in_range(cb, w) {
        b.key(text).hash(&mut h);
        b.segs(text).len().min(1).hash(&mut h);
    }
    h.finish()
}

/// Blocken som överlappar `w`.
pub fn in_range<'a>(cb: &'a [Block], w: &Range<usize>) -> impl Iterator<Item = &'a Block> {
    let i = cb.partition_point(|b| b.whole().end <= w.start);
    cb[i..].iter().take_while(move |b| b.open.start < w.end)
}

/// Blocket vars hela intervall innehåller `byte`.
pub fn find(cb: &[Block], byte: usize) -> Option<&Block> {
    let i = cb.partition_point(|b| b.whole().end <= byte);
    cb.get(i).filter(|b| b.open.start <= byte)
}

/// Blocket vars kod (innehåll) innehåller `byte`.
pub fn find_content(cb: &[Block], byte: usize) -> Option<&Block> {
    find(cb, byte).filter(|b| b.content.start <= byte && byte < b.content.end.max(b.content.start + 1))
}

/// Inhägnadens tecken, antal och infosträng om `line` är en inhägnadsrad (``` eller ~~~).
fn fence(line: &str) -> Option<(char, usize, &str)> {
    let l = line.trim_start_matches([' ', '\t', '>']);
    let c = l.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = l.chars().take_while(|x| *x == c).count();
    (n >= 3).then(|| (c, n, l[n..].trim()))
}

/// Alla inhägnade kodblock i ordning.
pub fn blocks(text: &str, a: &Analysis) -> Vec<Block> {
    struct Open {
        open: Range<usize>,
        info: String,
        ch: char,
        len: usize,
        start: usize,
        last: usize,
    }
    let mut out = Vec::new();
    let mut cur: Option<Open> = None;
    let finish = |o: Open, close: Option<Range<usize>>| {
        let end = close.as_ref().map_or(o.last, |c| c.start);
        let start = o.start.min(end);
        Block {
            lang: codehl::lang_of(&o.info),
            open: o.open,
            content: start..end,
            close,
            key: OnceLock::new(),
            segs: OnceLock::new(),
        }
    };
    for pc in &a.pieces {
        if pc.block != Blk::Code {
            if let Some(o) = cur.take() {
                out.push(finish(o, None));
            }
            continue;
        }
        match cur.take() {
            None => {
                if pc.markup {
                    if let Some((ch, len, info)) = text.get(pc.range.clone()).and_then(fence) {
                        cur = Some(Open { open: pc.range.clone(), info: info.to_owned(), ch, len, start: (pc.range.end + 1).min(text.len()), last: (pc.range.end + 1).min(text.len()) });
                    }
                }
            }
            Some(mut o) => {
                let closes = pc.markup
                    && text.get(pc.range.clone()).and_then(fence).is_some_and(|(ch, len, rest)| ch == o.ch && len >= o.len && rest.is_empty());
                if closes {
                    out.push(finish(o, Some(pc.range.clone())));
                } else {
                    o.last = o.last.max(pc.range.end);
                    cur = Some(o);
                }
            }
        }
    }
    if let Some(o) = cur {
        out.push(finish(o, None));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::markup;

    fn doc() -> String {
        "Före\n\n```rust\nfn main() {\n    let x = 1;\n}\n```\n\nMellan\n\n```\nbara text\n```\n\n~~~python\nprint(1)\n~~~\n\nEfter\n".to_owned()
    }

    #[test]
    fn finds_fenced_blocks_with_language() {
        let t = doc();
        let a = markup::analyze(&t);
        let cb = blocks(&t, &a);
        assert_eq!(cb.len(), 3);
        assert_eq!(cb[0].lang, "rust");
        assert_eq!(cb[0].code(&t), "fn main() {\n    let x = 1;\n}");
        assert_eq!(&t[cb[0].open.clone()], "```rust");
        assert_eq!(&t[cb[0].close.clone().unwrap()], "```");
        assert_eq!(cb[1].lang, "");
        assert_eq!(cb[1].code(&t), "bara text");
        assert_eq!(cb[2].lang, "python");
        assert_eq!(cb[2].code(&t), "print(1)");
        // Blocken är sorterade och överlappar inte.
        assert!(cb.windows(2).all(|w| w[0].whole().end <= w[1].whole().start));
        assert!(find(&cb, t.find("let x").unwrap()).is_some());
        assert!(find(&cb, t.find("Mellan").unwrap()).is_none());
    }

    #[test]
    fn unterminated_fence_runs_to_the_end() {
        let t = "```json\n{\"a\": 1}\n";
        let a = markup::analyze(t);
        let cb = blocks(t, &a);
        assert_eq!(cb.len(), 1);
        assert!(cb[0].close.is_none());
        assert_eq!(cb[0].code(t), "{\"a\": 1}");
    }

    #[test]
    fn indented_code_is_not_a_fenced_block() {
        let t = "text\n\n    indragen kod\n\nmer\n";
        let a = markup::analyze(t);
        assert!(blocks(t, &a).is_empty());
    }

    #[test]
    fn coloring_does_not_change_text_or_positions() {
        crate::highlight::syntaxes_wait();
        let t = doc();
        let a = markup::analyze(&t);
        let cb = blocks(&t, &a);
        let segs = cb[0].segs(&t);
        assert!(!segs.is_empty());
        // Bitarna täcker koden utan glapp, i ordning, och byter inte ut en byte av texten.
        let joined: String = segs.iter().map(|s| &t[s.range.clone()]).collect();
        assert_eq!(joined, &t[cb[0].content.clone()]);
        assert_eq!(segs.first().unwrap().range.start, cb[0].content.start);
        assert_eq!(segs.last().unwrap().range.end, cb[0].content.end);
        let kw = segs.iter().find(|s| &t[s.range.clone()] == "fn").unwrap();
        assert_eq!(kw.class, Class::Keyword);
        // Block utan språk färgas inte.
        assert!(cb[1].segs(&t).is_empty());
    }

    #[test]
    fn typing_elsewhere_does_not_recolor_a_block() {
        crate::highlight::syntaxes_wait();
        // Egen kod: färgningens cache delas med andra tester.
        let t = doc().replace("fn main()", "fn typing_test()").replace("print(1)", "print(31337)");
        let a = markup::analyze(&t);
        let cb = blocks(&t, &a);
        let _ = cb[0].segs(&t);
        let _ = cb[2].segs(&t);
        let before = codehl::runs();
        // Skrivning i brödtexten: nya block byggs men koden är densamma.
        let t2 = t.replace("Mellan", "Mellan och mer text");
        let a2 = markup::analyze(&t2);
        let cb2 = blocks(&t2, &a2);
        for b in &cb2 {
            let _ = b.segs(&t2);
        }
        assert_eq!(codehl::runs(), before, "oförändrade block ska inte färgas om");
        // Ändras koden färgas just det blocket om.
        let t3 = t.replace("let x = 1;", "let x = 2;");
        let a3 = markup::analyze(&t3);
        let cb3 = blocks(&t3, &a3);
        for b in &cb3 {
            let _ = b.segs(&t3);
        }
        assert_eq!(codehl::runs(), before + 1);
    }

    #[test]
    fn signature_changes_with_code_only() {
        crate::highlight::syntaxes_wait();
        let t = doc();
        let a = markup::analyze(&t);
        let cb = blocks(&t, &a);
        let w = cb[0].whole();
        let s1 = sig(&cb, &t, &w);
        let t2 = t.replace("let x = 1;", "let x = 2;");
        let a2 = markup::analyze(&t2);
        let cb2 = blocks(&t2, &a2);
        assert_ne!(s1, sig(&cb2, &t2, &cb2[0].whole()));
        assert_eq!(s1, sig(&cb, &t, &w));
    }
}
