//! Sidändringar i en PDF-flik: kör en operation från `organize` på flikens bytes, läser om visaren
//! direkt, för Ångra/Gör om och märker fliken som ändrad (`PdfTab::pages_changed`).
//!
//! Ändringarna görs i minnet; filen på disk skrivs först vid Spara (Ctrl+S). Krypterade PDF:er
//! avkrypteras vid första ändringen (som `print::decrypted_pdf`), så en sparad fil blir okrypterad.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::chrome::PdfTab;
use super::engine::{OpenError, PdfEngine};
use super::meta::read_meta;
use super::organize::{self, History};

/// Det sidpanelen, menyn och tangenterna ber om.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrgOp {
    Rotate { pages: Vec<usize>, clockwise: bool },
    Delete(Vec<usize>),
    /// Spara de valda sidorna som en ny PDF (fråga efter filnamn).
    Extract(Vec<usize>),
    /// Flytta sidorna så att de hamnar före sidan `before` (antal sidor = sist).
    Move { pages: BTreeSet<usize>, before: usize },
    /// Infoga en vald PDF på plats `at` (antal sidor = sist; "Slå ihop").
    InsertAt(usize),
    Undo,
    Redo,
}

/// Flikens tillstånd för sidändringar.
#[derive(Default)]
pub struct OrgState {
    pub history: History,
    /// Valda miniatyrer (sidnummer, 0-baserade).
    pub sel: BTreeSet<usize>,
    pub anchor: Option<usize>,
    /// Ett drag av miniatyrer pågår; `drop` är platsen (före vilken sida) som markeras.
    pub drag_active: bool,
    pub drop: Option<usize>,
}

impl OrgState {
    /// Nollställer ångra-historiken och urvalet (filen lästes om från disk).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Urvalet efter ett klick: (nytt urval, nytt ankare). Skift = intervall från ankaret, Ctrl = växla.
pub fn click_selection(sel: &BTreeSet<usize>, anchor: Option<usize>, page: usize, shift: bool, ctrl: bool, fallback_anchor: usize) -> (BTreeSet<usize>, Option<usize>) {
    if shift {
        let a = anchor.unwrap_or(fallback_anchor);
        let (lo, hi) = (a.min(page), a.max(page));
        (if ctrl { sel.iter().copied().chain(lo..=hi).collect() } else { (lo..=hi).collect() }, Some(a))
    } else if ctrl {
        let mut s = sel.clone();
        if !s.remove(&page) {
            s.insert(page);
        }
        (s, Some(page))
    } else {
        (BTreeSet::from([page]), Some(page))
    }
}

fn open_error(e: OpenError) -> String {
    match e {
        OpenError::Invalid(m) => m,
        _ => "lösenordet stämmer inte".to_owned(),
    }
}

/// Flikens bytes utan kryptering (lösenordet används om det finns).
fn plain(tab: &PdfTab) -> Result<Vec<u8>, String> {
    // Osparade anteckningar och formulärvärden bakas in först, så att de följer med sina sidor
    // (commit tömmer listan och läser om fälten).
    let edited;
    let bytes: &[u8] = if tab.view.annot.has_pending() || tab.view.forms.dirty() {
        edited = tab.edited_bytes()?;
        &edited
    } else {
        tab.view.bytes()
    };
    match tab.password.as_deref() {
        Some(pw) => crate::print::decrypted_pdf(bytes, pw).ok_or_else(|| "PDF:en kunde inte avkrypteras".to_owned()),
        None => Ok(bytes.to_vec()),
    }
}

/// Visar `bytes` i fliken (behåller sida och zoom). Ändrar inget annat.
fn install(ctx: &egui::Context, tab: &mut PdfTab, bytes: Arc<Vec<u8>>) -> Result<(), String> {
    let engine = PdfEngine::open(bytes.clone(), tab.password.as_deref(), ctx.clone()).map_err(open_error)?;
    let meta = read_meta(&bytes, tab.password.as_deref());
    tab.view.reload(engine, meta, bytes);
    Ok(())
}

/// Gör `new` till flikens innehåll: tidigare version sparas för Ångra och fliken märks som ändrad.
fn commit(ctx: &egui::Context, tab: &mut PdfTab, new: Vec<u8>, sel: BTreeSet<usize>) -> Result<(), String> {
    let prev = tab.view.bytes().clone();
    install(ctx, tab, Arc::new(new))?;
    tab.org.history.push(prev);
    tab.pages_changed = true;
    tab.view.annot.clear();
    tab.reload_forms();
    if let Some(&first) = sel.iter().next() {
        tab.view.go_to_page(first);
    }
    tab.org.sel = sel;
    tab.org.anchor = None;
    Ok(())
}

/// Utför en ändring som inte behöver någon fil. Ger en kort rapport till notisen.
pub fn run(ctx: &egui::Context, tab: &mut PdfTab, op: &OrgOp) -> Result<String, String> {
    let count = tab.view.page_count();
    match op {
        OrgOp::Rotate { pages, clockwise } => {
            let out = organize::rotate(&plain(tab)?, pages, if *clockwise { 1 } else { -1 })?;
            let sel = tab.org.sel.clone();
            commit(ctx, tab, out, sel)?;
            Ok(String::new())
        }
        OrgOp::Delete(pages) => {
            let first = pages.iter().copied().min().unwrap_or(0);
            let out = organize::delete(&plain(tab)?, pages)?;
            let after = organize::page_count(&out)?;
            commit(ctx, tab, out, organize::selection_after_delete(after, first))?;
            let n = pages.iter().collect::<BTreeSet<_>>().len();
            Ok(if n == 1 { "Tog bort 1 sida".to_owned() } else { format!("Tog bort {n} sidor") })
        }
        OrgOp::Move { pages, before } => {
            let (order, sel) = organize::move_order(count, pages, *before);
            if order.iter().enumerate().all(|(i, &o)| i == o) {
                return Ok(String::new());
            }
            let out = organize::reorder(&plain(tab)?, &order)?;
            commit(ctx, tab, out, sel)?;
            Ok(String::new())
        }
        OrgOp::Undo | OrgOp::Redo => {
            // Äldre versioner saknar de osparade anteckningarna och fältvärdena, som annars skulle försvinna.
            if tab.view.annot.has_pending() || tab.view.forms.dirty() {
                return Err("Spara först: det finns osparade anteckningar eller formulärvärden.".to_owned());
            }
            let cur = tab.view.bytes().clone();
            let (next, what) = if *op == OrgOp::Undo { (tab.org.history.undo(cur), "Ångrade") } else { (tab.org.history.redo(cur), "Gjorde om") };
            let Some(bytes) = next else { return Ok(String::new()) };
            install(ctx, tab, bytes)?;
            tab.pages_changed = true;
            tab.reload_forms();
            tab.org.sel.clear();
            Ok(format!("{what} sidändringen"))
        }
        OrgOp::Extract(_) | OrgOp::InsertAt(_) => Err("Åtgärden behöver en fil".to_owned()),
    }
}

/// En ny PDF med de valda sidorna, att skriva till disk.
pub fn extract_bytes(tab: &PdfTab, pages: &[usize]) -> Result<Vec<u8>, String> {
    organize::extract(&plain(tab)?, pages)
}

/// Infogar sidorna i `other` (filens bytes) på plats `at`; de nya sidorna blir markerade.
pub fn insert_bytes(ctx: &egui::Context, tab: &mut PdfTab, other: &[u8], at: usize) -> Result<String, String> {
    let before = tab.view.page_count();
    let at = at.min(before);
    let out = organize::insert(&plain(tab)?, other, None, at)?;
    let added = organize::page_count(&out)?.saturating_sub(before);
    commit(ctx, tab, out, (at..at + added).collect())?;
    Ok(if added == 1 { "Infogade 1 sida".to_owned() } else { format!("Infogade {added} sidor") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[usize]) -> BTreeSet<usize> {
        v.iter().copied().collect()
    }

    #[test]
    fn click_selection_modes() {
        let none = BTreeSet::new();
        assert_eq!(click_selection(&none, None, 3, false, false, 0), (set(&[3]), Some(3)));
        // Ctrl växlar och flyttar ankaret.
        let (s, a) = click_selection(&set(&[3]), Some(3), 5, false, true, 0);
        assert_eq!((s.clone(), a), (set(&[3, 5]), Some(5)));
        assert_eq!(click_selection(&s, a, 3, false, true, 0).0, set(&[5]));
        // Skift tar intervallet från ankaret, åt båda håll; utan ankare från aktuell sida.
        assert_eq!(click_selection(&set(&[2]), Some(2), 5, true, false, 0), (set(&[2, 3, 4, 5]), Some(2)));
        assert_eq!(click_selection(&set(&[4]), Some(4), 1, true, false, 0).0, set(&[1, 2, 3, 4]));
        assert_eq!(click_selection(&none, None, 3, true, false, 1).0, set(&[1, 2, 3]));
        // Ctrl+Skift lägger intervallet till urvalet.
        assert_eq!(click_selection(&set(&[0, 2]), Some(2), 4, true, true, 0).0, set(&[0, 2, 3, 4]));
    }

    #[test]
    fn edits_reload_the_tab_and_undo() {
        use crate::pdfview::view::PdfView;
        let ctx = egui::Context::default();
        let bytes = Arc::new(sample(4));
        let engine = PdfEngine::open(bytes.clone(), None, ctx.clone()).ok().expect("öppnas");
        let meta = read_meta(&bytes, None);
        let mut tab = PdfTab::new(PdfView::new(engine, meta, bytes.clone(), None), None, None);
        assert!(!tab.pages_changed);
        run(&ctx, &mut tab, &OrgOp::Delete(vec![0, 1])).unwrap();
        assert!(tab.pages_changed);
        assert_eq!(tab.view.page_count(), 2);
        assert_eq!(tab.org.sel, set(&[0]));
        run(&ctx, &mut tab, &OrgOp::Move { pages: set(&[0]), before: 2 }).unwrap();
        assert_eq!(tab.org.sel, set(&[1]));
        run(&ctx, &mut tab, &OrgOp::Rotate { pages: vec![1], clockwise: true }).unwrap();
        // En PDF-fil med sidan flyttad sist har gått att öppna igen och sidantalet stämmer.
        let msg = insert_bytes(&ctx, &mut tab, &sample(3), 1).unwrap();
        assert_eq!(msg, "Infogade 3 sidor");
        assert_eq!(tab.view.page_count(), 5);
        assert_eq!(tab.org.sel, set(&[1, 2, 3]));
        for _ in 0..4 {
            run(&ctx, &mut tab, &OrgOp::Undo).unwrap();
        }
        assert_eq!(tab.view.page_count(), 4);
        assert_eq!(**tab.view.bytes(), *bytes);
        assert!(!tab.org.history.can_undo());
        run(&ctx, &mut tab, &OrgOp::Redo).unwrap();
        assert_eq!(tab.view.page_count(), 2);
        assert!(extract_bytes(&tab, &[1]).is_ok());
        assert!(run(&ctx, &mut tab, &OrgOp::Delete(vec![0, 1])).is_err());
    }

    /// Minsta möjliga PDF med `n` sidor.
    fn sample(n: usize) -> Vec<u8> {
        use lopdf::{Dictionary, Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let root = doc.new_object_id();
        let mut kids = Vec::new();
        for i in 1..=n {
            let cid = doc.add_object(Stream::new(Dictionary::new(), format!("0 0 10 10 re f % {i}").into_bytes()));
            let page = doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => root, "Contents" => cid,
                "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            });
            kids.push(Object::Reference(page));
        }
        doc.objects.insert(root, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }));
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
        doc.trailer.set("Root", cat);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }
}
