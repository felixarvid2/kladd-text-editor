//! EPUB-läsläget: en bok öppnas bara för läsning (aldrig ändrad eller sparad). Inläsningen bygger en
//! `Book` av paketet: metadata, kapitel som Markdown (delade i delar som går att rita snabbt),
//! innehållsförteckning, länkmål och noter. Ritningen och tangenterna finns i `view.rs`, designen i
//! `docs/dev/epub.md`.

pub mod anchor;
pub mod css;
pub mod html;
pub mod read;
pub mod selection;
pub mod store;
pub mod view;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

pub use read::Meta;
use read::{Archive, RawToc};

use crate::paste::{El, parse};

/// Så stor (byte Markdown) en del högst är när ett kapitel delas. Hela delen ritas varje bildruta,
/// så stora kapitel delas vid styckegränser för att rulla flytande.
pub const SECTION_LIMIT: usize = 24_000;
/// Antal extra filer (utanför läsordningen) som tas med som länkmål, som mest.
const MAX_EXTRA: usize = 64;
/// Största stilmall (byte) som läses; större hoppas över.
const MAX_CSS: usize = 2_000_000;

/// Ett ställe i boken: del och byteposition i delens Markdown.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Dest {
    pub section: usize,
    pub byte: usize,
}

/// Ett kapitel (en fil i läsordningen).
#[derive(Clone, Debug)]
pub struct Chapter {
    pub path: String,
    pub title: String,
    /// Ingår i den löpande läsordningen (`linear="no"` och extra filer gör det inte).
    pub linear: bool,
    pub first_section: usize,
    pub section_count: usize,
}

/// En del av ett kapitel som ritas som en enhet.
#[derive(Clone, Debug)]
pub struct Section {
    pub chapter: usize,
    pub md: String,
    pub words: usize,
    /// Bildfiler i delen.
    pub images: Vec<String>,
    /// Länkadresser (`kl:K:N`) i delen: varje sådan fångas som klick i stället för att öppnas som webbadress.
    pub link_dests: Vec<String>,
    /// Omslagssida: bara en bild, som ritas så att den får plats i fönstret.
    pub cover: Option<String>,
    /// Ord i böckerna före delen (för framsteg).
    pub words_before: usize,
    /// Byteposition i kapitlets Markdown där delen börjar (förankring av bokmärken och understrykningar).
    pub offset: usize,
    /// Förlagets blockstil: byteposition i delens Markdown och utseende.
    pub blocks: Vec<(usize, css::BlockLook)>,
}

#[derive(Clone, Debug)]
pub struct TocEntry {
    pub title: String,
    pub depth: usize,
    pub dest: Option<Dest>,
}

pub struct Book {
    /// Dokumentets id (ingår i bildadresserna).
    pub uid: u64,
    pub meta: Meta,
    pub chapters: Vec<Chapter>,
    pub sections: Vec<Section>,
    pub toc: Vec<TocEntry>,
    /// Omslagsbildens sökväg i arkivet.
    pub cover: Option<String>,
    pub total_words: usize,
    anchors: HashMap<String, Dest>,
    notes: HashMap<String, String>,
    /// Länkarna per konverterat kapitel (`kl:K:N` är `links[K][N]`).
    links: Vec<Vec<html::LinkRef>>,
    archive: Mutex<Archive>,
}

impl std::fmt::Debug for Book {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Book({:?}, {} kapitel, {} delar)", self.meta.title, self.chapters.len(), self.sections.len())
    }
}

fn is_html(path: &str, media: &str) -> bool {
    let l = path.to_ascii_lowercase();
    media.contains("html") || media == "text/xml" || [".xhtml", ".html", ".htm", ".xml"].iter().any(|e| l.ends_with(e))
}

fn is_image_path(path: &str) -> bool {
    let l = path.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".bmp"].iter().any(|e| l.ends_with(e))
}

/// Delar Markdown vid styckegränser (tomma rader utanför kodblock) i delar om högst `limit` byte, där det går.
pub fn split_ranges(md: &str, limit: usize) -> Vec<Range<usize>> {
    if md.len() <= limit {
        return vec![0..md.len()];
    }
    let mut cands = Vec::new();
    let mut fence = false;
    let mut pos = 0;
    let mut prev_blank = false;
    for line in md.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        if !fence && prev_blank && !line.trim().is_empty() {
            cands.push(pos);
        }
        prev_blank = !fence && line.trim().is_empty();
        pos += line.len();
    }
    let mut cuts = Vec::new();
    let mut start = 0;
    let mut last: Option<usize> = None;
    for &c in &cands {
        if c - start > limit {
            let cut = match last {
                Some(l) if l > start => l,
                _ => c,
            };
            cuts.push(cut);
            start = cut;
            if c - start > limit {
                cuts.push(c);
                start = c;
            }
        }
        last = Some(c);
    }
    let mut out = Vec::new();
    let mut from = 0;
    for c in cuts {
        out.push(from..c);
        from = c;
    }
    out.push(from..md.len());
    out
}

/// Ersätter `[text](kl:K:N)` med `text` (länkar i en not eller i en kopia går inte att följa).
pub fn unlink(md: &str) -> String {
    const KEY: &str = "](kl:";
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(p) = rest.find(KEY) {
        let after = &rest[p + KEY.len()..];
        let close = after.find(')').filter(|&c| after[..c].bytes().all(|b| b.is_ascii_digit() || b == b':'));
        out.push_str(&rest[..p]);
        let Some(c) = close else {
            out.push_str(KEY);
            rest = after;
            continue;
        };
        // Länktextens inledande '[': leta bakåt med hänsyn till nästlade hakparenteser.
        let bytes = out.as_bytes();
        let (mut depth, mut i, mut open) = (0usize, bytes.len(), None);
        while i > 0 {
            i -= 1;
            let escaped = i > 0 && bytes[i - 1] == b'\\';
            match bytes[i] {
                b']' if !escaped => depth += 1,
                b'[' if !escaped => {
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        match open {
            Some(o) => {
                out.remove(o);
            }
            None => out.push_str(&rest[p..p + KEY.len() + c + 1]),
        }
        rest = &after[c + 1..];
    }
    out.push_str(rest);
    out
}

/// Ren text ur en delens Markdown, ett stycke per rad (för sökning).
pub fn plain_lines(md: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut lines = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, lines: &mut Vec<String>| {
        let t = cur.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            lines.push(t);
        }
        cur.clear();
    };
    for ev in Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        match ev {
            Event::Text(t) | Event::Code(t) => cur.push_str(&t),
            Event::SoftBreak | Event::HardBreak => cur.push(' '),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock | TagEnd::TableCell | TagEnd::TableRow) => {
                flush(&mut cur, &mut lines)
            }
            Event::Start(Tag::TableCell) => cur.push(' '),
            _ => {}
        }
    }
    flush(&mut cur, &mut lines);
    lines
}

/// Sidan består bara av en bild?
fn cover_only(md: &str, images: &[String]) -> Option<String> {
    let t = md.trim();
    if images.len() == 1 && t.starts_with("![") && t.ends_with(')') && !t[2..].contains("![") && !t.contains("\n\n") {
        // Bara bilden: ingenting efter den avslutande parentesen.
        let close = t.find("](")?;
        let rest = &t[close + 2..];
        if rest.find(')') == Some(rest.len() - 1) {
            return images.first().cloned();
        }
    }
    None
}

struct Conv {
    path: String,
    linear: bool,
    out: html::Converted,
}

impl Book {
    /// Läser en bok ur filens bytes. `uid` är dokumentets id.
    pub fn load(uid: u64, bytes: Vec<u8>) -> Result<Book, String> {
        let arc: Arc<[u8]> = bytes.into();
        let mut ar = Archive::open(arc)?;
        let pkg = read::read_package(&mut ar)?;
        let names: HashSet<String> = ar.names().iter().cloned().collect();
        let lower: HashMap<String, String> = names.iter().map(|n| (n.to_lowercase(), n.clone())).collect();
        let canon = move |p: &str| -> Option<String> {
            if names.contains(p) { Some(p.to_owned()) } else { lower.get(&p.to_lowercase()).cloned() }
        };

        // Läsordningen: bara filer som finns och går att läsa som sidor (eller bilder).
        let mut order: Vec<(String, bool)> = Vec::new();
        for s in &pkg.spine {
            if let Some(c) = canon(&s.path) {
                if !order.iter().any(|(p, _)| *p == c) {
                    order.push((c, s.linear));
                }
            }
        }
        if order.is_empty() {
            return Err(crate::i18n::t("Boken har ingen text att läsa (tom läsordning).").to_owned());
        }
        // Står alla filer som `linear="no"` läser man dem ändå i ordning.
        if order.iter().all(|(_, linear)| !linear) {
            for o in &mut order {
                o.1 = true;
            }
        }

        // Träd för alla kapitel; ingång 1 samlar vilka mål fotnotsreferenser pekar på.
        let mut roots: Vec<(String, (El, css::Sheet))> = Vec::new();
        let mut note_targets = HashSet::new();
        let mut css_cache: HashMap<String, css::Sheet> = HashMap::new();
        for (p, _) in &order {
            let (root, sheet) = load_page(&mut ar, p, &mut css_cache);
            html::scan_note_targets(&root, p, &canon, &mut note_targets);
            roots.push((p.clone(), (root, sheet)));
        }

        // Ingång 2: kapitlen till Markdown. Filer som länkas men inte ligger i läsordningen (noter) tas med efteråt.
        let mut convs: Vec<Conv> = Vec::new();
        let mut done: HashSet<String> = HashSet::new();
        let mut queue: Vec<(String, bool)> = order.clone();
        let mut qi = 0;
        let mut extra = 0;
        let mut root_map: HashMap<String, (El, css::Sheet)> = roots.into_iter().collect();
        while qi < queue.len() {
            let (path, linear) = queue[qi].clone();
            qi += 1;
            if !done.insert(path.clone()) {
                continue;
            }
            let (root, sheet) = match root_map.remove(&path) {
                Some(r) => r,
                None => {
                    let r = load_page(&mut ar, &path, &mut css_cache);
                    html::scan_note_targets(&r.0, &path, &canon, &mut note_targets);
                    r
                }
            };
            let env = html::Env { doc_id: uid, chapter: convs.len(), file: &path, canon: &canon, note_targets: &note_targets, sheet: &sheet };
            let out = html::convert(&root, &env);
            for l in &out.links {
                if extra < MAX_EXTRA && !done.contains(&l.path) && !queue.iter().any(|(p, _)| *p == l.path) {
                    let media = pkg.items.values().find(|i| i.path == l.path).map_or("", |i| i.media.as_str());
                    if is_html(&l.path, media) {
                        queue.push((l.path.clone(), false));
                        extra += 1;
                    }
                }
            }
            convs.push(Conv { path, linear, out });
        }

        // Omslaget: en egen sida först om boken inte själv börjar med omslagsbilden.
        let cover = pkg.cover.clone();
        let first_has_cover = convs.first().is_some_and(|c| match (&cover, cover_only(&c.out.md, &c.out.images)) {
            // En sida med bara en bild är omslaget om det är omslagsbilden (annars en titelsida med illustration).
            (Some(cv), Some(img)) => img == *cv,
            (None, Some(_)) => true,
            (Some(cv), None) => c.out.images.contains(cv) && c.out.md.split_whitespace().count() <= 12,
            (None, None) => false,
        });
        let synthetic_cover = cover.clone().filter(|_| !first_has_cover);

        // Delar, länkmål och noter.
        let mut chapters: Vec<Chapter> = Vec::new();
        let mut sections: Vec<Section> = Vec::new();
        let mut anchors: HashMap<String, Dest> = HashMap::new();
        let mut notes: HashMap<String, String> = HashMap::new();
        let mut links: Vec<Vec<html::LinkRef>> = Vec::new();
        let mut headings: Vec<Option<String>> = Vec::new();
        if let Some(cv) = &synthetic_cover {
            chapters.push(Chapter { path: String::new(), title: crate::i18n::t("Omslag").to_owned(), linear: true, first_section: 0, section_count: 1 });
            sections.push(Section {
                chapter: 0,
                md: format!("![{}]({})\n", crate::i18n::t("Omslag"), html::image_uri(uid, cv)),
                words: 0,
                images: vec![cv.clone()],
                link_dests: Vec::new(),
                cover: Some(cv.clone()),
                words_before: 0,
                offset: 0,
                blocks: Vec::new(),
            });
            headings.push(None);
        }
        for c in convs {
            let ch = chapters.len();
            let first = sections.len();
            let ranges = split_ranges(&c.out.md, SECTION_LIMIT);
            for r in &ranges {
                let md = c.out.md[r.clone()].to_owned();
                let images: Vec<String> = c.out.images.iter().filter(|p| md.contains(&html::image_uri(uid, p))).cloned().collect();
                let link_dests = link_dests_of(&md);
                let cover_page = cover_only(&md, &images);
                let blocks = c.out.styles.iter().filter(|(b, _)| *b >= r.start && *b < r.end).map(|(b, l)| (b - r.start, l.clone())).collect();
                sections.push(Section {
                    chapter: ch,
                    words: md.split_whitespace().count(),
                    md,
                    images,
                    link_dests,
                    cover: cover_page,
                    words_before: 0,
                    offset: r.start,
                    blocks,
                });
            }
            for (id, byte) in &c.out.anchors {
                let si = ranges.iter().position(|r| *byte >= r.start && *byte < r.end).unwrap_or(ranges.len() - 1);
                let d = Dest { section: first + si, byte: byte.saturating_sub(ranges[si].start) };
                anchors.entry(format!("{}#{id}", c.path)).or_insert(d);
            }
            anchors.entry(c.path.clone()).or_insert(Dest { section: first, byte: 0 });
            for (id, md) in &c.out.notes {
                notes.entry(format!("{}#{id}", c.path)).or_insert_with(|| unlink(md));
            }
            chapters.push(Chapter { path: c.path.clone(), title: String::new(), linear: c.linear, first_section: first, section_count: ranges.len() });
            headings.push(c.out.heading.clone());
            links.push(c.out.links);
        }

        let mut words = 0;
        for s in &mut sections {
            s.words_before = words;
            words += s.words;
        }

        // Innehållsförteckning.
        let target_of = |t: &Option<(String, Option<String>)>| -> Option<Dest> {
            let (p, f) = t.as_ref()?;
            let p = canon(p).unwrap_or_else(|| p.clone());
            f.as_ref()
                .and_then(|f| anchors.get(&format!("{p}#{f}")).copied())
                .or_else(|| anchors.get(&p).copied())
        };
        let mut toc: Vec<TocEntry> = pkg
            .toc
            .iter()
            .map(|r: &RawToc| TocEntry { title: r.title.trim().to_owned(), depth: r.depth, dest: target_of(&r.target) })
            .filter(|t| !t.title.is_empty())
            .collect();
        if toc.iter().all(|t| t.dest.is_none()) {
            toc.clear();
        }
        // Kapitelrubrikerna ur boken, om den saknar förteckning.
        if toc.is_empty() {
            for (i, ch) in chapters.iter().enumerate().filter(|(_, c)| c.linear) {
                let title = match headings.get(i).cloned().flatten().filter(|h| !h.trim().is_empty()) {
                    Some(h) => h,
                    None if synthetic_cover.is_some() && i == 0 => crate::i18n::t("Omslag").to_owned(),
                    None => crate::tf!("Del {n}", n = i + 1),
                };
                toc.push(TocEntry { title, depth: 0, dest: Some(Dest { section: ch.first_section, byte: 0 }) });
            }
        }
        // Kapitlens titlar: förteckningens post som pekar in i kapitlet, annars första rubriken.
        for (i, ch) in chapters.iter_mut().enumerate() {
            if !ch.title.is_empty() {
                continue;
            }
            let range = ch.first_section..ch.first_section + ch.section_count;
            let from_toc = toc.iter().find(|t| t.dest.is_some_and(|d| range.contains(&d.section))).map(|t| t.title.clone());
            ch.title = from_toc
                .or_else(|| headings.get(i).cloned().flatten().map(|h| h.trim().to_owned()).filter(|h| !h.is_empty()))
                .unwrap_or_else(|| crate::tf!("Del {n}", n = i + 1));
        }

        let meta = {
            let mut m = pkg.meta.clone();
            if m.title.trim().is_empty() {
                m.title = String::new();
            }
            m
        };
        let book = Book {
            uid,
            meta,
            chapters,
            sections,
            toc,
            cover,
            total_words: words,
            anchors,
            notes,
            links,
            archive: Mutex::new(ar),
        };
        Ok(book)
    }

    /// Länken `kl:K:N` i en del: dit den pekar, och om den är en notreferens.
    pub fn link(&self, dest: &str) -> Option<(Dest, bool)> {
        let rest = dest.strip_prefix("kl:")?;
        let (k, n) = rest.split_once(':')?;
        let l = self.links.get(k.parse::<usize>().ok()?)?.get(n.parse::<usize>().ok()?)?;
        Some((self.lookup(&l.path, l.frag.as_deref())?, l.noteref))
    }

    /// Noten som en länk pekar på (Markdown), om målet är en not.
    pub fn link_note(&self, dest: &str) -> Option<String> {
        let rest = dest.strip_prefix("kl:")?;
        let (k, n) = rest.split_once(':')?;
        let l = self.links.get(k.parse::<usize>().ok()?)?.get(n.parse::<usize>().ok()?)?;
        let frag = l.frag.as_ref()?;
        self.notes.get(&format!("{}#{frag}", l.path)).cloned()
    }

    fn lookup(&self, path: &str, frag: Option<&str>) -> Option<Dest> {
        frag.and_then(|f| self.anchors.get(&format!("{path}#{f}")).copied()).or_else(|| self.anchors.get(path).copied())
    }

    /// Bildens bytes, redo att visa (SVG ritas om till PNG).
    pub fn image(&self, path: &str) -> Option<Vec<u8>> {
        let bytes = self.archive.lock().ok()?.read(path)?;
        if crate::imgfmt::is_svg(&bytes) {
            return crate::imgfmt::svg_to_png(&bytes);
        }
        crate::imgfmt::to_supported(bytes)
    }

    /// Omslagets bytes.
    pub fn cover_image(&self) -> Option<Vec<u8>> {
        self.image(self.cover.as_deref()?)
    }

    pub fn title(&self) -> String {
        let t = self.meta.title.trim();
        if !t.is_empty() {
            return t.to_owned();
        }
        self.chapters.iter().find(|c| c.linear && !c.path.is_empty()).map(|c| c.title.clone()).unwrap_or_default()
    }

    /// Kapitlets hela Markdown (delarna hopsatta). Bokmärken och understrykningar pekar in i den.
    pub fn chapter_md(&self, ch: usize) -> String {
        let Some(c) = self.chapters.get(ch) else { return String::new() };
        self.sections[c.first_section..c.first_section + c.section_count].iter().map(|s| s.md.as_str()).collect()
    }

    /// Stället (del och byte i delen) för en byteposition i kapitel `ch`s Markdown.
    pub fn dest_in_chapter(&self, ch: usize, byte: usize) -> Option<Dest> {
        let c = self.chapters.get(ch)?;
        let secs = &self.sections[c.first_section..c.first_section + c.section_count];
        let i = secs.iter().rposition(|s| s.offset <= byte).unwrap_or(0);
        Some(Dest { section: c.first_section + i, byte: byte.saturating_sub(secs[i].offset).min(secs[i].md.len()) })
    }

    /// Kapitlens (index, titel) i läsordning.
    pub fn chapter_titles(&self) -> Vec<(usize, String)> {
        self.linear_chapters().into_iter().map(|c| (c, self.chapters[c].title.clone())).collect()
    }

    /// Kapitlen i den löpande läsordningen (index i `chapters`).
    pub fn linear_chapters(&self) -> Vec<usize> {
        self.chapters.iter().enumerate().filter(|(_, c)| c.linear).map(|(i, _)| i).collect()
    }

    /// Kapitlet som följer (`step` = 1) eller föregår (-1) kapitel `ch` i läsordningen.
    pub fn step_chapter(&self, ch: usize, step: isize) -> Option<usize> {
        let lin = self.linear_chapters();
        let pos = match lin.iter().position(|c| *c == ch) {
            Some(p) => p as isize,
            // Ett extra kapitel (en not): läsordningen fortsätter från närmast föregående.
            None => lin.iter().rposition(|c| *c < ch).map_or(-1, |p| p as isize) + isize::from(step < 0),
        };
        let at = pos + step;
        (at >= 0).then(|| lin.get(at as usize).copied()).flatten()
    }

    /// Förteckningsposten som gäller vid `at` (den sista som börjar före eller vid positionen).
    pub fn toc_current(&self, at: Dest) -> Option<usize> {
        let mut best: Option<(usize, Dest)> = None;
        for (i, t) in self.toc.iter().enumerate() {
            if let Some(d) = t.dest {
                if d <= at && best.is_none_or(|(_, b)| d >= b) {
                    best = Some((i, d));
                }
            }
        }
        best.map(|b| b.0)
    }

    /// Hela bokens text som Markdown för "Redigera en kopia": läsordningens kapitel, bilderna som
    /// `bild:`-namn och de bilder som hör till dem.
    pub fn to_markdown(&self) -> (String, Vec<(String, crate::docx::Image)>) {
        let mut images: Vec<(String, crate::docx::Image)> = Vec::new();
        let mut names: HashMap<String, String> = HashMap::new();
        let mut out = String::new();
        let title = self.title();
        if !title.is_empty() {
            out.push_str(&format!("# {}\n\n", title.replace('\n', " ")));
        }
        if !self.meta.authors.is_empty() {
            out.push_str(&format!("*{}*\n\n", self.meta.authors.join(", ")));
        }
        for ch in self.chapters.iter().filter(|c| c.linear && !c.path.is_empty()) {
            for s in &self.sections[ch.first_section..ch.first_section + ch.section_count] {
                let mut md = unlink(&s.md);
                for img in &s.images {
                    let uri = html::image_uri(self.uid, img);
                    if !md.contains(&uri) {
                        continue;
                    }
                    let name = match names.get(img) {
                        Some(n) => n.clone(),
                        None => {
                            let Some(bytes) = self.image(img) else {
                                md = md.replace(&format!("]({uri})"), "]()");
                                continue;
                            };
                            let Some(image) = crate::imgfmt::prepare(bytes) else { continue };
                            let base = img.rsplit('/').next().unwrap_or(img).trim_end_matches(".svg").to_owned();
                            let base = if crate::imgfmt::is_svg(&image.bytes) || base.is_empty() { format!("bild{}", images.len() + 1) } else { base };
                            let mut name = base.clone();
                            let mut n = 2;
                            while images.iter().any(|(k, _)| *k == name) {
                                name = format!("{n}-{base}");
                                n += 1;
                            }
                            let name = if name.contains('.') { name } else { format!("{name}.png") };
                            images.push((name.clone(), image));
                            names.insert(img.clone(), name.clone());
                            name
                        }
                    };
                    md = md.replace(&format!("]({uri})"), &format!("]({}{})", crate::docx::IMAGE_SCHEME, name.replace(' ', "%20")));
                }
                out.push_str(md.trim_end());
                out.push_str("\n\n");
            }
        }
        (out.trim_end().to_owned() + "\n", images)
    }
}

/// Adresserna `kl:K:N` i en text.
fn link_dests_of(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = md;
    while let Some(p) = rest.find("](kl:") {
        let after = &rest[p + 2..];
        let end = after.find(')').unwrap_or(after.len());
        out.push(after[..end].to_owned());
        rest = &after[end..];
    }
    out.sort();
    out.dedup();
    out
}

/// Läser en sida (eller en bild i läsordningen) som träd och stilmallen som gäller den.
fn load_page(ar: &mut Archive, path: &str, cache: &mut HashMap<String, css::Sheet>) -> (El, css::Sheet) {
    if is_image_path(path) {
        let name = path.rsplit('/').next().unwrap_or(path);
        return (parse(&format!("<body><p><img src=\"{}\"/></p></body>", name.replace('"', ""))), css::Sheet::default());
    }
    let raw = ar.read_text(path).unwrap_or_default();
    let root = parse(&raw);
    let mut sheet = css::Sheet::default();
    // Länkade stilmallar (i ordning) och sedan `<style>` i sidan.
    let mut hrefs: Vec<String> = Vec::new();
    read::each(&root, &mut |e| {
        if read::local(&e.tag) == "link" && e.attr("rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet")) {
            if let Some(h) = e.attr("href").filter(|h| !read::has_scheme(h)) {
                hrefs.push(read::resolve(read::dir_of(path), h).0);
            }
        }
    });
    for h in hrefs.into_iter().take(16) {
        let key = h.to_lowercase();
        if !cache.contains_key(&key) {
            let text = ar.read_text(&h).filter(|t| t.len() <= MAX_CSS).unwrap_or_default();
            cache.insert(key.clone(), css::Sheet::parse(&text));
        }
        sheet.extend(&cache[&key]);
    }
    for st in css::inline_styles(&raw).into_iter().take(16) {
        sheet.add(&st);
    }
    (root, sheet)
}

#[cfg(test)]
pub(crate) mod tests;
