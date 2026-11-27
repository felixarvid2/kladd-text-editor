//! Redigeringen av en presentation: transaktioner på paketets delar, ångra/gör om och sparande.
//!
//! Sanningen är paketets delar (byte). En ändring görs i en `Txn`: delarna som ändras tolkas förlustfritt
//! (`edit_xml.rs`), ändras och skrivs ut igen, och presentationen byggs om ur de nya delarna
//! (`Deck::from_package`, med typsnitt och bildcachar från den förra). Ritmodellen tolkas alltså alltid
//! från samma byte som sparas. Ångra/gör om är ögonblicksbilder av de delar ett steg ändrade.
//! Sparandet (`save.rs`) kopierar alla orörda zip-poster oförändrade.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::edit_xml::{self, XDoc, XElem};
use super::model::Deck;
use super::package::{self, Package, Rel};

pub const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
pub const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const NS_PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";

/// Innehållstyperna för delar som redigeringen skapar.
pub mod ct {
    pub const SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
    pub const NOTES_SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
    pub const NOTES_MASTER: &str = "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";
    pub const THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
}

/// Huvuddelens innehållstyp för en filändelse (`None` = inte en presentation).
pub fn main_content_type(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
        "ppsx" => "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml",
        "potx" => "application/vnd.openxmlformats-officedocument.presentationml.template.main+xml",
        "pptm" => "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml",
        "ppsm" => "application/vnd.ms-powerpoint.slideshow.macroEnabled.main+xml",
        "potm" => "application/vnd.ms-powerpoint.template.macroEnabled.main+xml",
        _ => return None,
    })
}

/// En relation i en transaktion.
pub type RelInfo = Rel;

/// En pågående ändring: tolkade och ändrade delar, nya och borttagna delar.
pub struct Txn<'a> {
    pub deck: &'a Deck,
    docs: BTreeMap<String, XDoc>,
    raw: BTreeMap<String, Option<Arc<Vec<u8>>>>,
    used: &'a HashSet<String>,
    created: HashSet<String>,
}

impl<'a> Txn<'a> {
    fn new(deck: &'a Deck, used: &'a HashSet<String>) -> Txn<'a> {
        Txn { deck, docs: BTreeMap::new(), raw: BTreeMap::new(), used, created: HashSet::new() }
    }

    /// Delens nuvarande byte (med ändringar i transaktionen; tolkade delar skrivs ut).
    pub fn read(&self, part: &str) -> Option<Vec<u8>> {
        if let Some(d) = self.docs.get(part) {
            return Some(d.to_bytes());
        }
        match self.raw.get(part) {
            Some(Some(b)) => Some(b.to_vec()),
            Some(None) => None,
            None => self.deck.pkg.parts.get(part).map(|b| b.to_vec()),
        }
    }

    pub fn exists(&self, part: &str) -> bool {
        self.docs.contains_key(part) || matches!(self.raw.get(part), Some(Some(_))) || (!self.raw.contains_key(part) && self.deck.pkg.parts.contains_key(part))
    }

    /// Delen tolkad för ändring.
    pub fn doc(&mut self, part: &str) -> Result<&mut XDoc, String> {
        if !self.docs.contains_key(part) {
            let bytes = self.read(part).ok_or_else(|| format!("{part} saknas"))?;
            let d = edit_xml::parse(&bytes).ok_or_else(|| crate::i18n::t("En del av presentationen kan inte ändras (XML som inte är UTF-8 eller är trasig).").to_owned())?;
            self.raw.remove(part);
            self.docs.insert(part.to_owned(), d);
        }
        Ok(self.docs.get_mut(part).expect("tolkad ovan"))
    }

    /// Lägger in en ny del som XML-text.
    pub fn put_xml(&mut self, part: &str, xml: &str) -> Result<(), String> {
        let d = edit_xml::parse(xml.as_bytes()).ok_or_else(|| format!("ogiltig XML för {part}"))?;
        self.raw.remove(part);
        self.docs.insert(part.to_owned(), d);
        self.created.insert(part.to_lowercase());
        Ok(())
    }

    pub fn put_raw(&mut self, part: &str, bytes: Vec<u8>) {
        self.docs.remove(part);
        self.raw.insert(part.to_owned(), Some(Arc::new(bytes)));
        self.created.insert(part.to_lowercase());
    }

    /// Tar bort en del, dess relationsdel och dess innehållstyp.
    pub fn remove_part(&mut self, part: &str) -> Result<(), String> {
        self.docs.remove(part);
        self.raw.insert(part.to_owned(), None);
        let rels = rels_name(part);
        if self.exists(&rels) {
            self.docs.remove(&rels);
            self.raw.insert(rels.clone(), None);
            // Vissa verktyg listar relationsdelarna med `Override`.
            self.ct_remove_override(&rels)?;
        }
        self.ct_remove_override(part)
    }

    /// Ett delnamn som inte finns och inte har funnits under redigeringen (`ppt/slides/slide`, `.xml`).
    pub fn unique_part(&mut self, prefix: &str, ext: &str) -> String {
        let mut n = 1;
        loop {
            let name = format!("{prefix}{n}{ext}");
            let low = name.to_lowercase();
            if !self.used.contains(&low) && !self.created.contains(&low) && !self.exists(&name) && self.deck.pkg.read(&name).is_none() {
                self.created.insert(low);
                return name;
            }
            n += 1;
        }
    }

    // ---------------------------------------------------------------- relationer

    /// Relationerna från en del (med ändringar i transaktionen).
    pub fn rels(&self, owner: &str) -> Vec<RelInfo> {
        let name = rels_name(owner);
        let Some(bytes) = self.read(&name) else { return Vec::new() };
        let Some(d) = edit_xml::parse(&bytes) else { return Vec::new() };
        let dir = owner.rfind('/').map_or("", |i| &owner[..i]);
        d.root
            .elems()
            .filter(|e| e.local() == "Relationship")
            .filter_map(|e| {
                let id = e.attr("Id")?;
                let ty = e.attr("Type").unwrap_or_default();
                let kind = ty.rsplit('/').next().unwrap_or("").to_owned();
                let raw = e.attr("Target")?;
                let external = e.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
                let target = if external { raw } else { package::resolve(dir, &raw) };
                Some(Rel { id, kind, target, external })
            })
            .collect()
    }

    fn rels_doc(&mut self, owner: &str) -> Result<&mut XDoc, String> {
        let name = rels_name(owner);
        if !self.exists(&name) {
            self.put_xml(&name, &format!("{XML_DECL}<Relationships xmlns=\"{NS_PKG_REL}\"></Relationships>"))?;
            // Utan `Default` för .rels (vissa verktyg listar varje relationsdel för sig) får den en `Override`.
            if self.content_type(&name).is_none() {
                self.ct_override(&name, "application/vnd.openxmlformats-package.relationships+xml")?;
            }
        }
        self.doc(&name)
    }

    /// Lägger till en relation (`kind` = typens sista led, t.ex. `slide`) och ger dess id.
    pub fn add_rel(&mut self, owner: &str, kind: &str, target_part: &str) -> Result<String, String> {
        let dir = owner.rfind('/').map_or("", |i| &owner[..i]).to_owned();
        let target = relative(&dir, target_part);
        let ty = rel_type(kind);
        let d = self.rels_doc(owner)?;
        let id = next_rel_id(&d.root);
        let el = d.fragment(&format!("<Relationship Id=\"{id}\" Type=\"{}\" Target=\"{}\"/>", edit_xml::escape(&ty), edit_xml::escape(&target)));
        let el = el.into_iter().next().ok_or("relation")?;
        d.root.children.push(edit_xml::XNode::Elem(el));
        Ok(id)
    }

    /// Lägger till en relation med ett givet id och en given typ (kopiering av en del med relationer).
    pub fn add_rel_raw(&mut self, owner: &str, id: &str, ty: &str, target: &str, external: bool) -> Result<(), String> {
        let d = self.rels_doc(owner)?;
        let mode = if external { " TargetMode=\"External\"" } else { "" };
        let el = d.fragment(&format!("<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{mode}/>", edit_xml::escape(id), edit_xml::escape(ty), edit_xml::escape(target)));
        let el = el.into_iter().next().ok_or("relation")?;
        d.root.children.push(edit_xml::XNode::Elem(el));
        Ok(())
    }

    pub fn remove_rel(&mut self, owner: &str, id: &str) -> Result<(), String> {
        if !self.exists(&rels_name(owner)) {
            return Ok(());
        }
        let d = self.rels_doc(owner)?;
        d.root.children.retain(|c| !matches!(c, edit_xml::XNode::Elem(e) if e.local() == "Relationship" && e.attr("Id").as_deref() == Some(id)));
        Ok(())
    }

    /// Relationerna som de står (id, typ, mål som det står, extern).
    pub fn raw_rels(&self, owner: &str) -> Vec<(String, String, String, bool)> {
        let Some(bytes) = self.read(&rels_name(owner)) else { return Vec::new() };
        let Some(d) = edit_xml::parse(&bytes) else { return Vec::new() };
        d.root
            .elems()
            .filter(|e| e.local() == "Relationship")
            .filter_map(|e| Some((e.attr("Id")?, e.attr("Type").unwrap_or_default(), e.attr("Target")?, e.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External")))))
            .collect()
    }

    // ---------------------------------------------------------------- innehållstyper

    pub fn ct_override(&mut self, part: &str, ty: &str) -> Result<(), String> {
        let d = self.doc("[Content_Types].xml")?;
        let pn = format!("/{part}");
        if let Some(e) = d.root.elems_mut().find(|e| e.local() == "Override" && e.attr("PartName").is_some_and(|p| p.eq_ignore_ascii_case(&pn))) {
            e.set_attr("ContentType", ty);
            return Ok(());
        }
        let el = d.fragment(&format!("<Override PartName=\"{}\" ContentType=\"{}\"/>", edit_xml::escape(&pn), edit_xml::escape(ty)));
        if let Some(el) = el.into_iter().next() {
            d.root.children.push(edit_xml::XNode::Elem(el));
        }
        Ok(())
    }

    pub fn ct_remove_override(&mut self, part: &str) -> Result<(), String> {
        let pn = format!("/{part}");
        let has = self.read("[Content_Types].xml").is_some_and(|b| String::from_utf8_lossy(&b).to_lowercase().contains(&format!("\"{}\"", pn.to_lowercase())));
        if !has {
            return Ok(());
        }
        let d = self.doc("[Content_Types].xml")?;
        d.root.children.retain(|c| !matches!(c, edit_xml::XNode::Elem(e) if e.local() == "Override" && e.attr("PartName").is_some_and(|p| p.eq_ignore_ascii_case(&pn))));
        Ok(())
    }

    /// Lägger till `Default` för en filändelse om den saknas.
    pub fn ct_default(&mut self, ext: &str, ty: &str) -> Result<(), String> {
        let d = self.doc("[Content_Types].xml")?;
        if d.root.elems().any(|e| e.local() == "Default" && e.attr("Extension").is_some_and(|x| x.eq_ignore_ascii_case(ext))) {
            return Ok(());
        }
        let el = d.fragment(&format!("<Default Extension=\"{}\" ContentType=\"{}\"/>", edit_xml::escape(ext), edit_xml::escape(ty)));
        if let Some(el) = el.into_iter().next() {
            // `Default` före `Override` (vanlig ordning, inte ett krav).
            d.root.insert_before(el, &["ct:Override"]);
        }
        Ok(())
    }

    /// Delens innehållstyp (med ändringar i transaktionen).
    pub fn content_type(&self, part: &str) -> Option<String> {
        let d = edit_xml::parse(&self.read("[Content_Types].xml")?)?;
        let pn = format!("/{part}");
        if let Some(e) = d.root.elems().find(|e| e.local() == "Override" && e.attr("PartName").is_some_and(|p| p.eq_ignore_ascii_case(&pn))) {
            return e.attr("ContentType");
        }
        let ext = part.rsplit('.').next()?.to_lowercase();
        d.root.elems().find(|e| e.local() == "Default" && e.attr("Extension").is_some_and(|x| x.to_lowercase() == ext)).and_then(|e| e.attr("ContentType"))
    }

    /// Delen för presentationen (`ppt/presentation.xml`).
    pub fn pres_part(&self) -> String {
        self.rels("").iter().find(|r| r.kind == "officeDocument").map(|r| r.target.clone()).unwrap_or_else(|| "ppt/presentation.xml".to_owned())
    }

    /// Har transaktionen ändrat något?
    fn changed(&self) -> bool {
        self.raw.iter().any(|(k, v)| v.as_deref() != self.deck.pkg.parts.get(k).map(|b| &**b))
            || self.docs.iter().any(|(k, d)| self.deck.pkg.parts.get(k).is_none_or(|old| **old != d.to_bytes()))
    }

    /// Ändringarna i transaktionen som delnamn → nytt innehåll (`None` = borttagen), utan delar som
    /// blev lika som förut.
    fn finish(self) -> BTreeMap<String, Option<Arc<Vec<u8>>>> {
        let mut out = self.raw;
        for (k, d) in self.docs {
            out.insert(k, Some(Arc::new(d.to_bytes())));
        }
        out.retain(|k, v| match (v.as_ref(), self.deck.pkg.parts.get(k)) {
            (Some(new), Some(old)) => new != old,
            (None, None) => false,
            _ => true,
        });
        out
    }
}

/// `ppt/slides/slide1.xml` → `ppt/slides/_rels/slide1.xml.rels`.
pub fn rels_name(part: &str) -> String {
    match part.rfind('/') {
        Some(i) => format!("{}/_rels/{}.rels", &part[..i], &part[i + 1..]),
        None if part.is_empty() => "_rels/.rels".to_owned(),
        None => format!("_rels/{part}.rels"),
    }
}

/// Relativt mål från katalogen `dir` till delen (`ppt/slides` → `ppt/media/a.png` = `../media/a.png`).
pub fn relative(dir: &str, target: &str) -> String {
    let from: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), from.len() - common).collect();
    parts.extend(to[common..].iter().map(|s| (*s).to_owned()));
    parts.join("/")
}

/// Relationstypens URI för en kort typ.
pub fn rel_type(kind: &str) -> String {
    format!("{NS_R}/{kind}")
}

fn next_rel_id(root: &XElem) -> String {
    let ids: HashSet<String> = root.elems().filter_map(|e| e.attr("Id")).collect();
    let mut n = ids.iter().filter_map(|i| i.strip_prefix("rId").and_then(|s| s.parse::<u64>().ok())).max().unwrap_or(0) + 1;
    while ids.contains(&format!("rId{n}")) {
        n += 1;
    }
    format!("rId{n}")
}

/// Ett steg i historiken.
struct Step {
    /// Delarnas innehåll före och efter (`None` = fanns inte).
    before: Vec<(String, Option<Arc<Vec<u8>>>)>,
    after: Vec<(String, Option<Arc<Vec<u8>>>)>,
    rev_before: u64,
    rev_after: u64,
    /// Skrivning i samma textruta slås ihop till ett steg.
    merge: Option<String>,
    at: Instant,
}

/// Slår ihop skrivning inom så här lång tid till ett ångra-steg.
const MERGE_WINDOW: Duration = Duration::from_millis(1500);
/// Högst så många ångra-steg.
const MAX_UNDO: usize = 300;

/// Redigeringstillståndet för en presentation.
pub struct Session {
    /// Zip-filen som öppnades eller senast sparades.
    base: Arc<Vec<u8>>,
    /// Delar som kan skilja sig från `base`.
    touched: HashSet<String>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    rev: u64,
    next_rev: u64,
    saved_rev: u64,
    /// Alla delnamn som funnits under redigeringen (gemener): ett namn återanvänds aldrig, så att
    /// bildcachen (efter delnamn) aldrig visar en gammal bild.
    used: HashSet<String>,
    /// Digitala signaturer togs bort vid första ändringen (notis till användaren).
    pub signature_removed: bool,
}

/// Resultatet av en ändring: den nya presentationen.
pub struct Applied {
    pub deck: Deck,
    pub signature_removed: bool,
}

impl Session {
    pub fn new(base: Vec<u8>, deck: &Deck) -> Session {
        let used = deck.pkg.parts.keys().map(|k| k.to_lowercase()).collect();
        Session { base: Arc::new(base), touched: HashSet::new(), undo: Vec::new(), redo: Vec::new(), rev: 0, next_rev: 1, saved_rev: 0, used, signature_removed: false }
    }

    /// Finns osparade ändringar?
    pub fn dirty(&self) -> bool {
        self.rev != self.saved_rev
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Avslutar sammanslagningen av skrivning (nästa ändring blir ett eget steg).
    pub fn break_merge(&mut self) {
        if let Some(s) = self.undo.last_mut() {
            s.merge = None;
        }
    }

    /// Gör en ändring. `Ok(None)` = ingenting ändrades. `merge`: skrivning i samma textruta inom kort
    /// tid blir ett ångra-steg.
    pub fn apply(&mut self, deck: &Deck, merge: Option<String>, f: impl FnOnce(&mut Txn) -> Result<(), String>) -> Result<Option<Applied>, String> {
        let mut txn = Txn::new(deck, &self.used);
        f(&mut txn)?;
        let sig = if txn.changed() { strip_signatures(&mut txn)? } else { false };
        let changes = txn.finish();
        if changes.is_empty() {
            return Ok(None);
        }
        let new = rebuild(deck, &changes)?;
        let before: Vec<_> = changes.keys().map(|k| (k.clone(), deck.pkg.parts.get(k).cloned())).collect();
        let after: Vec<_> = changes.into_iter().collect();
        for (k, _) in &after {
            self.touched.insert(k.clone());
            self.used.insert(k.to_lowercase());
        }
        let rev = self.next_rev;
        self.next_rev += 1;
        let now = Instant::now();
        let merged = match (self.undo.last_mut(), &merge) {
            (Some(top), Some(m)) if top.merge.as_ref() == Some(m) && top.rev_after == self.rev && now.duration_since(top.at) < MERGE_WINDOW => {
                for (k, v) in &before {
                    if !top.before.iter().any(|(n, _)| n == k) {
                        top.before.push((k.clone(), v.clone()));
                    }
                }
                for (k, v) in &after {
                    match top.after.iter_mut().find(|(n, _)| n == k) {
                        Some(e) => e.1 = v.clone(),
                        None => top.after.push((k.clone(), v.clone())),
                    }
                }
                top.rev_after = rev;
                top.at = now;
                true
            }
            _ => false,
        };
        if !merged {
            self.undo.push(Step { before, after, rev_before: self.rev, rev_after: rev, merge, at: now });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.rev = rev;
        if sig {
            self.signature_removed = true;
        }
        Ok(Some(Applied { deck: new, signature_removed: sig }))
    }

    pub fn undo(&mut self, deck: &Deck) -> Result<Option<Deck>, String> {
        let Some(step) = self.undo.pop() else { return Ok(None) };
        let changes: BTreeMap<_, _> = step.before.iter().cloned().collect();
        match rebuild(deck, &changes) {
            Ok(d) => {
                self.touched.extend(changes.keys().cloned());
                self.rev = step.rev_before;
                self.redo.push(step);
                Ok(Some(d))
            }
            Err(e) => {
                self.undo.push(step);
                Err(e)
            }
        }
    }

    pub fn redo(&mut self, deck: &Deck) -> Result<Option<Deck>, String> {
        let Some(mut step) = self.redo.pop() else { return Ok(None) };
        let changes: BTreeMap<_, _> = step.after.iter().cloned().collect();
        match rebuild(deck, &changes) {
            Ok(d) => {
                self.touched.extend(changes.keys().cloned());
                self.rev = step.rev_after;
                step.merge = None;
                self.undo.push(step);
                Ok(Some(d))
            }
            Err(e) => {
                self.redo.push(step);
                Err(e)
            }
        }
    }

    /// Filens innehåll för att spara som `ext` (`pptx`, `ppsx` …). `from_ext` är filändelsen som filen
    /// hade: bara när den byts ändras huvuddelens innehållstyp (och makron tas bort för ett format utan
    /// makron); presentationen byggs då om och returneras också.
    pub fn save_bytes(&mut self, deck: &Deck, ext: &str, from_ext: &str) -> Result<(Vec<u8>, Option<Deck>), String> {
        let want = main_content_type(ext).ok_or_else(|| crate::i18n::t("En presentation kan bara sparas som .pptx, .ppsx eller .potx.").to_owned())?;
        let mut txn = Txn::new(deck, &self.used);
        let pres = txn.pres_part();
        if ext != from_ext && txn.content_type(&pres).as_deref() != Some(want) {
            txn.ct_override(&pres, want)?;
            // Makron följer inte med till ett format utan makron (filen öppnas annars inte).
            if !want.contains("macroEnabled") {
                for r in txn.rels(&pres).into_iter().filter(|r| r.kind == "vbaProject") {
                    txn.remove_rel(&pres, &r.id)?;
                    txn.remove_part(&r.target)?;
                }
            }
        }
        let changes = txn.finish();
        let rebuilt = if changes.is_empty() {
            None
        } else {
            let d = rebuild(deck, &changes)?;
            for k in changes.keys() {
                self.touched.insert(k.clone());
                self.used.insert(k.to_lowercase());
            }
            Some(d)
        };
        let bytes = super::save::write_zip(&self.base, &rebuilt.as_ref().unwrap_or(deck).pkg, &self.touched)?;
        Ok((bytes, rebuilt))
    }

    /// Efter en lyckad sparning: det sparade blir utgångsläget.
    pub fn mark_saved(&mut self, bytes: Vec<u8>) {
        self.base = Arc::new(bytes);
        self.touched.clear();
        self.saved_rev = self.rev;
    }
}

/// Bygger om presentationen med ändrade delar.
fn rebuild(deck: &Deck, changes: &BTreeMap<String, Option<Arc<Vec<u8>>>>) -> Result<Deck, String> {
    let mut parts: HashMap<String, Arc<Vec<u8>>> = deck.pkg.parts.clone();
    for (k, v) in changes {
        match v {
            Some(b) => {
                parts.insert(k.clone(), b.clone());
            }
            None => {
                parts.remove(k);
            }
        }
    }
    Deck::from_package(Package::from_parts(parts), Some(deck))
}

/// Tar bort digitala signaturer (de blir ogiltiga när filen ändras). Sant om någon fanns.
fn strip_signatures(txn: &mut Txn) -> Result<bool, String> {
    let origin: Vec<RelInfo> = txn.rels("").into_iter().filter(|r| r.kind == "origin").collect();
    if origin.is_empty() {
        return Ok(false);
    }
    for r in origin {
        for s in txn.rels(&r.target) {
            if !s.external {
                txn.remove_part(&s.target)?;
            }
        }
        txn.remove_part(&r.target)?;
        txn.remove_rel("", &r.id)?;
    }
    // Övriga delar under _xmlsignatures/.
    let rest: Vec<String> = txn.deck.pkg.parts.keys().filter(|k| k.starts_with("_xmlsignatures/")).cloned().collect();
    for k in rest {
        if txn.exists(&k) {
            txn.remove_part(&k)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relativa_mal_och_relationsnamn() {
        assert_eq!(relative("ppt/slides", "ppt/media/a.png"), "../media/a.png");
        assert_eq!(relative("ppt", "ppt/slides/slide2.xml"), "slides/slide2.xml");
        assert_eq!(relative("", "ppt/presentation.xml"), "ppt/presentation.xml");
        assert_eq!(relative("ppt/notesSlides", "ppt/slides/slide1.xml"), "../slides/slide1.xml");
        assert_eq!(rels_name("ppt/slides/slide1.xml"), "ppt/slides/_rels/slide1.xml.rels");
        assert_eq!(rels_name(""), "_rels/.rels");
        assert_eq!(main_content_type("ppsx").map(|c| c.contains("slideshow")), Some(true));
        assert!(main_content_type("docx").is_none());
    }
}
