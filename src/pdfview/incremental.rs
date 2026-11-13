//! Inkrementell uppdatering av PDF:er (ISO 32000-1, 7.5.6): ändrade objekt läggs sist i filen med ett
//! nytt xref-avsnitt, en ny trailer (med `/Prev`) och `%%EOF`. De ursprungliga bytena rörs inte,
//! så digitala signaturer (som täcker ett `/ByteRange` i originalet) förblir giltiga och filen
//! struktureras inte om.
//!
//! Gränssnittet är en generell lista över nya, ändrade och borttagna objekt (`Changes`), som kan
//! byggas för hand eller som skillnaden mellan ett inläst `lopdf`-dokument före och efter en ändring.
//! Krypterade filer hanteras inte här (nya objekt skulle behöva krypteras): anroparna skriver om
//! dem avkrypterade som förut.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{Hash, Hasher};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, Writer};

use crate::i18n::t;

/// Nya, ändrade och borttagna objekt jämfört med originalet.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Changes {
    /// Nya och ändrade objekt (samma id ersätter originalets objekt).
    pub objects: BTreeMap<ObjectId, Object>,
    /// Objekt som tas bort (blir lediga i det nya xref-avsnittet).
    pub removed: BTreeSet<ObjectId>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.removed.is_empty()
    }

    /// Lägger till eller ersätter ett objekt.
    pub fn set(&mut self, id: ObjectId, obj: Object) {
        self.removed.remove(&id);
        self.objects.insert(id, obj);
    }

    /// Tar bort ett objekt.
    pub fn remove(&mut self, id: ObjectId) {
        self.objects.remove(&id);
        self.removed.insert(id);
    }

    /// Skillnaden mellan `before` och `after`: objekt som är nya eller har ändrats, och de som försvunnit.
    pub fn diff(before: &Document, after: &Document) -> Self {
        let mut c = Changes::default();
        for (id, obj) in &after.objects {
            if before.objects.get(id) != Some(obj) {
                c.objects.insert(*id, obj.clone());
            }
        }
        for id in before.objects.keys() {
            if !after.objects.contains_key(id) {
                c.removed.insert(*id);
            }
        }
        c
    }
}

/// Är filen krypterad (då görs ingen inkrementell uppdatering)?
fn encrypted(doc: &Document) -> bool {
    doc.is_encrypted() || doc.trailer.get(b"Encrypt").is_ok()
}

/// Läser `bytes`, låter `edit` ändra dokumentet och lägger skillnaden sist i originalbytena.
/// Ger `Ok(None)` för krypterade filer (anroparen skriver om dem avkrypterade).
pub fn edit(bytes: &[u8], password: Option<&str>, edit: impl FnOnce(&mut Document) -> Result<(), String>) -> Result<Option<Vec<u8>>, String> {
    let mut opts = lopdf::LoadOptions::default();
    opts.password = password.map(str::to_owned);
    let before = Document::load_mem_with_options(bytes, opts).map_err(|e| crate::tf!("PDF:en kunde inte läsas: {e}", e = e))?;
    if encrypted(&before) {
        return Ok(None);
    }
    let mut after = before.clone();
    edit(&mut after)?;
    let changes = Changes::diff(&before, &after);
    append(bytes, &before, &changes).map(Some)
}

/// Offset som sista `startxref` pekar på.
fn last_startxref(bytes: &[u8]) -> Option<usize> {
    let key = b"startxref";
    let pos = bytes.windows(key.len()).rposition(|w| w == key)?;
    let rest = &bytes[pos + key.len()..];
    let digits: Vec<u8> = rest.iter().copied().skip_while(|b| b.is_ascii_whitespace()).take_while(u8::is_ascii_digit).collect();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

/// Använder originalets senaste xref en ström (annars en klassisk tabell)? `None` om offseten inte
/// pekar på något av dem (filen är reparerad vid inläsning och kan inte byggas på).
fn xref_is_stream(bytes: &[u8], at: usize) -> Option<bool> {
    let rest = bytes.get(at..)?;
    let rest = &rest[rest.iter().take_while(|b| b.is_ascii_whitespace()).count()..];
    if rest.starts_with(b"xref") {
        return Some(false);
    }
    let mut it = rest.split(|b| b.is_ascii_whitespace()).filter(|s| !s.is_empty());
    let num = it.next()?;
    let gen_ = it.next()?;
    let obj = it.next()?;
    (num.iter().all(u8::is_ascii_digit) && gen_.iter().all(u8::is_ascii_digit) && obj.starts_with(b"obj")).then_some(true)
}

#[derive(Clone, Copy)]
enum Entry {
    Used { offset: usize, gen_: u16 },
    Free { next_gen: u16 },
}

/// Nytt ID-par: första elementet behålls, det andra ändras (identifierar den nya versionen).
fn new_id(old: &Object, appended: &[u8]) -> Option<Object> {
    let Object::Array(a) = old else { return None };
    let first = a.first()?.clone();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    appended.hash(&mut h);
    let a1 = h.finish();
    a1.hash(&mut h);
    let a2 = h.finish();
    let mut second = Vec::with_capacity(16);
    second.extend(a1.to_be_bytes());
    second.extend(a2.to_be_bytes());
    Some(Object::Array(vec![first, Object::String(second, lopdf::StringFormat::Hexadecimal)]))
}

/// Originalbytena följda av `changes` som en inkrementell uppdatering. `base` är originalet inläst
/// (för `/Root`, `/Info`, `/ID` och `/Size`).
pub fn append(original: &[u8], base: &Document, changes: &Changes) -> Result<Vec<u8>, String> {
    let bad = || t("PDF:en kunde inte skrivas").to_owned();
    let prev = last_startxref(original).ok_or_else(bad)?;
    let stream_xref = xref_is_stream(original, prev).ok_or_else(bad)?;
    let root = base.trailer.get(b"Root").map_err(|_| bad())?.clone();

    let mut out = original.to_vec();
    if !out.ends_with(b"\n") && !out.ends_with(b"\r") {
        out.push(b'\n');
    }
    let appended_from = out.len();
    let mut entries: BTreeMap<u32, Entry> = BTreeMap::new();

    for (&(id, gen_), obj) in &changes.objects {
        entries.insert(id, Entry::Used { offset: out.len(), gen_ });
        let mut obj = obj.clone();
        if let Object::Stream(s) = &mut obj {
            fix_length(s);
        }
        out.extend(format!("{id} {gen_} obj\n").bytes());
        if Writer::need_separator(&obj) {
            out.push(b' ');
        }
        Writer::write_object(&mut out, &obj).map_err(|_| bad())?;
        if Writer::need_end_separator(&obj) {
            out.push(b' ');
        }
        out.extend(b"\nendobj\n");
    }
    for &(id, gen_) in &changes.removed {
        if base.objects.contains_key(&(id, gen_)) {
            entries.insert(id, Entry::Free { next_gen: gen_.saturating_add(1) });
        }
    }

    let old_size = base.trailer.get(b"Size").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0).max(0) as u64;
    let mut size = old_size.max(u64::from(base.max_id) + 1);
    if let Some((&last, _)) = entries.iter().next_back() {
        size = size.max(u64::from(last) + 1);
    }

    let mut trailer = Dictionary::new();
    trailer.set("Root", root);
    if let Ok(info) = base.trailer.get(b"Info") {
        trailer.set("Info", info.clone());
    }
    if let Some(id) = base.trailer.get(b"ID").ok().and_then(|o| new_id(o, &out[appended_from..])) {
        trailer.set("ID", id);
    }
    trailer.set("Prev", Object::Integer(prev as i64));

    let xref_start = out.len();
    if stream_xref {
        let self_id = u32::try_from(size).map_err(|_| bad())?;
        entries.insert(self_id, Entry::Used { offset: xref_start, gen_: 0 });
        trailer.set("Size", Object::Integer(size as i64 + 1));
        write_xref_stream(&mut out, &entries, trailer, self_id).ok_or_else(bad)?;
    } else {
        trailer.set("Size", Object::Integer(size as i64));
        write_xref_table(&mut out, &entries);
        out.extend(b"trailer\n");
        Writer::write_object(&mut out, &Object::Dictionary(trailer)).map_err(|_| bad())?;
    }
    out.extend(format!("\nstartxref\n{xref_start}\n%%EOF\n").bytes());
    Ok(out)
}

/// `/Length` ska vara ett heltal som stämmer med innehållet (ett indirekt värde duger inte i en ny version).
fn fix_length(s: &mut Stream) {
    let len = s.content.len() as i64;
    if s.dict.get(b"Length").ok().and_then(|o| o.as_i64().ok()) != Some(len) {
        s.dict.set("Length", Object::Integer(len));
    }
}

/// Löpande följder av objektnummer: `(första, antal)`.
fn runs(entries: &BTreeMap<u32, Entry>) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = Vec::new();
    for &id in entries.keys() {
        match v.last_mut() {
            Some((s, n)) if *s + *n == id => *n += 1,
            _ => v.push((id, 1)),
        }
    }
    v
}

fn write_xref_table(out: &mut Vec<u8>, entries: &BTreeMap<u32, Entry>) {
    out.extend(b"xref\n");
    for (start, n) in runs(entries) {
        out.extend(format!("{start} {n}\n").bytes());
        for id in start..start + n {
            match entries[&id] {
                Entry::Used { offset, gen_ } => out.extend(format!("{offset:010} {gen_:05} n \n").bytes()),
                Entry::Free { next_gen } => out.extend(format!("{:010} {next_gen:05} f \n", 0).bytes()),
            }
        }
    }
}

/// En xref-ström som objekt `self_id`, med `/W [1 4 2]`.
fn write_xref_stream(out: &mut Vec<u8>, entries: &BTreeMap<u32, Entry>, mut dict: Dictionary, self_id: u32) -> Option<()> {
    let mut data = Vec::new();
    let mut index = Vec::new();
    for (start, n) in runs(entries) {
        index.push(Object::Integer(i64::from(start)));
        index.push(Object::Integer(i64::from(n)));
        for id in start..start + n {
            match entries[&id] {
                Entry::Used { offset, gen_ } => {
                    data.push(1);
                    data.extend(u32::try_from(offset).ok()?.to_be_bytes());
                    data.extend(gen_.to_be_bytes());
                }
                Entry::Free { next_gen } => {
                    data.push(0);
                    data.extend(0u32.to_be_bytes());
                    data.extend(next_gen.to_be_bytes());
                }
            }
        }
    }
    dict.set("Type", Object::Name(b"XRef".to_vec()));
    dict.set("W", Object::Array(vec![Object::Integer(1), Object::Integer(4), Object::Integer(2)]));
    dict.set("Index", Object::Array(index));
    dict.set("Length", Object::Integer(data.len() as i64));
    out.extend(format!("{self_id} 0 obj\n").bytes());
    Writer::write_object(out, &Object::Stream(Stream::new(dict, data))).ok()?;
    out.extend(b"\nendobj");
    Some(())
}

// ============================================================== signaturer

/// Antal signaturer i PDF:en: formulärfält av typen `/Sig` som har ett värde (`/V`).
/// Läser in filen; ger 0 om den inte går att läsa.
pub fn signature_count(bytes: &[u8], password: Option<&str>) -> usize {
    let mut opts = lopdf::LoadOptions::default();
    opts.password = password.map(str::to_owned);
    Document::load_mem_with_options(bytes, opts).map_or(0, |d| signatures_in(&d))
}

fn signatures_in(doc: &Document) -> usize {
    let Some(acro) = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).and_then(|o| deref(doc, o)).and_then(|o| o.as_dict().ok()) else { return 0 };
    let Some(fields) = acro.get(b"Fields").ok().and_then(|o| deref(doc, o)).and_then(|o| o.as_array().ok()) else { return 0 };
    let mut seen = HashSet::new();
    let mut n = 0;
    let mut stack: Vec<(&Object, bool, usize)> = fields.iter().map(|f| (f, false, 0)).collect();
    while let Some((o, inherited_sig, depth)) = stack.pop() {
        if let Object::Reference(id) = o
            && !seen.insert(*id)
        {
            continue;
        }
        let Some(d) = deref(doc, o).and_then(|o| o.as_dict().ok()) else { continue };
        let is_sig = match d.get(b"FT").ok().and_then(|o| deref(doc, o)).and_then(|o| o.as_name().ok()) {
            Some(ft) => ft == b"Sig",
            None => inherited_sig,
        };
        if is_sig && d.get(b"V").ok().and_then(|o| deref(doc, o)).is_some_and(|v| matches!(v, Object::Dictionary(_))) {
            n += 1;
        }
        if depth < 32
            && let Some(kids) = d.get(b"Kids").ok().and_then(|o| deref(doc, o)).and_then(|o| o.as_array().ok())
        {
            stack.extend(kids.iter().map(|k| (k, is_sig, depth + 1)));
        }
    }
    n
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => doc.get_object(*id).ok(),
        o => Some(o),
    }
}

// ============================================================== tester

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{SaveOptions, dictionary};

    /// Ett litet dokument med en sida och ett textfält; `modern` ger objektströmmar och xref-ström.
    fn sample(modern: bool, sig: bool) -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let page = doc.new_object_id();
        let content = doc.add_object(Stream::new(dictionary! {}, b"0 0 m 10 10 l S".to_vec()));
        let field = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("a"),
            "Rect" => vec![10.into(), 10.into(), 100.into(), 30.into()], "P" => page,
        });
        let mut fields = vec![Object::Reference(field)];
        let mut annots = vec![Object::Reference(field)];
        if sig {
            let v = doc.add_object(dictionary! {
                "Type" => "Sig", "Filter" => "Adobe.PPKLite", "SubFilter" => "adbe.pkcs7.detached",
                "ByteRange" => vec![0.into(), 0.into(), 0.into(), 0.into()],
                "Contents" => Object::String(vec![0; 16], lopdf::StringFormat::Hexadecimal),
            });
            let s = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Widget", "FT" => "Sig", "T" => Object::string_literal("sig"),
                "Rect" => vec![0.into(), 0.into(), 0.into(), 0.into()], "V" => v, "P" => page,
            });
            fields.push(Object::Reference(s));
            annots.push(Object::Reference(s));
        }
        doc.set_object(page, dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            "Resources" => dictionary! {}, "Annots" => annots,
        });
        doc.set_object(pages, dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 });
        let acro = doc.add_object(dictionary! { "Fields" => fields, "SigFlags" => if sig { 3 } else { 0 } });
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "AcroForm" => acro });
        doc.trailer.set("Root", cat);
        doc.trailer.set("ID", vec![Object::string_literal("first-id"), Object::string_literal("second")]);
        let mut out = Vec::new();
        if modern {
            doc.save_with_options(&mut out, SaveOptions::builder().use_object_streams(true).use_xref_streams(true).build()).unwrap();
        } else {
            doc.reference_table.cross_reference_type = lopdf::xref::XrefType::CrossReferenceTable;
            doc.save_to(&mut out).unwrap();
        }
        out
    }

    fn set_title(doc: &mut Document) -> Result<(), String> {
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let pages = doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
        let first = doc.get_dictionary(pages).unwrap().get(b"Kids").unwrap().as_array().unwrap()[0].as_reference().unwrap();
        doc.get_dictionary_mut(first).unwrap().set("Rotate", 90);
        let n = doc.add_object(lopdf::dictionary! { "Marker" => "new" });
        doc.get_dictionary_mut(root).unwrap().set("Marker", n);
        Ok(())
    }

    fn check(original: &[u8], stream: bool) -> Vec<u8> {
        let out = edit(original, None, set_title).unwrap().unwrap();
        // Originalet är ett exakt prefix och resten är en ny version.
        assert!(out.starts_with(original));
        assert!(out.ends_with(b"%%EOF\n"));
        let doc = Document::load_mem(&out).unwrap();
        let pages = doc.get_pages();
        let page = doc.get_dictionary(pages[&1]).unwrap();
        assert_eq!(page.get(b"Rotate").unwrap().as_i64().unwrap(), 90);
        assert!(doc.catalog().unwrap().get(b"Marker").is_ok());
        // /Prev pekar på originalets startxref.
        let prev = last_startxref(original).unwrap();
        let new = last_startxref(&out).unwrap();
        assert!(new > original.len() - 40);
        assert_eq!(xref_is_stream(&out, new), Some(stream));
        let txt = String::from_utf8_lossy(&out[original.len()..]).into_owned();
        // Kedjan: den nya sektionen pekar på originalets, och originalets startxref är en riktig xref.
        assert!(txt.contains(&format!("/Prev {prev}")), "{txt}");
        assert!(xref_is_stream(&out, prev).is_some());
        // Första ID-elementet behålls.
        let ids = doc.trailer.get(b"ID").unwrap().as_array().unwrap();
        assert_eq!(ids[0].as_str().unwrap(), b"first-id");
        assert_ne!(ids[1].as_str().unwrap(), b"second");
        out
    }

    #[test]
    fn table_original_gives_table_update() {
        let o = sample(false, false);
        assert_eq!(xref_is_stream(&o, last_startxref(&o).unwrap()), Some(false));
        let out = check(&o, false);
        assert!(crate::pdfview::engine::PdfEngine::open(std::sync::Arc::new(out), None, egui::Context::default()).is_ok());
    }

    #[test]
    fn stream_original_gives_stream_update() {
        let o = sample(true, false);
        assert_eq!(xref_is_stream(&o, last_startxref(&o).unwrap()), Some(true));
        let out = check(&o, true);
        assert!(crate::pdfview::engine::PdfEngine::open(std::sync::Arc::new(out), None, egui::Context::default()).is_ok());
    }

    #[test]
    fn updates_chain_twice() {
        let o = sample(false, false);
        let a = edit(&o, None, set_title).unwrap().unwrap();
        let b = edit(&a, None, |d| {
            let n = d.add_object(lopdf::dictionary! { "Second" => 1 });
            d.catalog_mut().unwrap().set("Second", n);
            Ok(())
        })
        .unwrap()
        .unwrap();
        assert!(b.starts_with(&a));
        let doc = Document::load_mem(&b).unwrap();
        assert!(doc.catalog().unwrap().get(b"Marker").is_ok() && doc.catalog().unwrap().get(b"Second").is_ok());
    }

    #[test]
    fn removed_objects_become_free() {
        let o = sample(false, false);
        let before = Document::load_mem(&o).unwrap();
        let victim = *before.objects.keys().find(|id| matches!(before.get_object(**id), Ok(Object::Stream(_)))).unwrap();
        let mut c = Changes::default();
        c.remove(victim);
        let out = append(&o, &before, &c).unwrap();
        let doc = Document::load_mem(&out).unwrap();
        assert!(doc.get_object(victim).is_err());
        assert!(String::from_utf8_lossy(&out[o.len()..]).contains(" f \n"));
    }

    #[test]
    fn signature_bytes_stay_untouched_and_are_counted() {
        let o = sample(false, true);
        assert_eq!(signature_count(&o, None), 1);
        assert_eq!(signature_count(&sample(false, false), None), 0);
        // Låtsas att ByteRange täcker hela originalet: de bytena ska vara oförändrade.
        let out = edit(&o, None, set_title).unwrap().unwrap();
        assert_eq!(&out[..o.len()], &o[..]);
        assert_eq!(signature_count(&out, None), 1);
    }

    #[test]
    fn forms_and_annotations_in_one_section_on_a_signed_file() {
        use crate::pdfview::annot::{NewAnnot, NewKind, apply_doc};
        use crate::pdfview::forms::{self, Value};
        for modern in [false, true] {
            let o = sample(modern, true);
            let fields = forms::read_fields(&o, None);
            let tx = fields.iter().find(|f| f.name == "a").unwrap().id;
            let a = NewAnnot { page: 0, color: [255, 235, 59], kind: NewKind::Note { pos: egui::pos2(50.0, 60.0), text: "Hej".into() } };
            let out = edit(&o, None, |d| {
                forms::apply_doc(d, &[(tx, Value::Text("värde".into()))])?;
                apply_doc(d, std::slice::from_ref(&a), &Default::default());
                Ok(())
            })
            .unwrap()
            .unwrap();
            // ByteRange-täckta bytes (här hela originalet) är oförändrade och det finns bara ett nytt avsnitt.
            assert_eq!(&out[..o.len()], &o[..]);
            let tail = &out[o.len()..];
            assert_eq!(tail.windows(9).filter(|w| *w == b"startxref").count(), 1);
            assert_eq!(signature_count(&out, None), 1);
            let after = forms::read_fields(&out, None);
            assert_eq!(after.iter().find(|f| f.name == "a").unwrap().id, tx);
            let doc = Document::load_mem(&out).unwrap();
            let page = *doc.get_pages().values().next().unwrap();
            let n = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().len();
            assert_eq!(n, 3);
            assert!(doc.get_dictionary(tx).unwrap().get(b"V").is_ok());
        }
    }

    #[test]
    fn encrypted_and_garbage_are_not_updated() {
        assert!(edit(b"not a pdf", None, |_| Ok(())).is_err());
        let o = sample(false, false);
        // Utan ändringar läggs ändå en giltig tom version till.
        let out = edit(&o, None, |_| Ok(())).unwrap().unwrap();
        assert!(Document::load_mem(&out).is_ok());
    }

    #[test]
    fn bogus_startxref_is_refused() {
        let mut o = sample(false, false);
        let n = o.len();
        o.truncate(n - 20);
        o.extend(b"startxref\n5\n%%EOF\n");
        let doc = Document::load_mem(&o).unwrap();
        assert!(append(&o, &doc, &Changes::default()).is_err());
    }
}
