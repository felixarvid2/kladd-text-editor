//! Presentationsdelen av säkerhetskopieringen (`recovery.rs`): en fullständig .pptx med de osparade ändringarna.

use super::view::PptxTab;

impl PptxTab {
    /// Ändringsnumret om presentationen har osparade ändringar (säkerhetskopian skrivs bara när det ändrats).
    pub fn recovery_rev(&self) -> Option<u64> {
        let s = &self.edit.session;
        s.dirty().then(|| s.rev())
    }

    /// Filens innehåll just nu, för säkerhetskopian.
    pub fn recovery_bytes(&self) -> Result<Vec<u8>, String> {
        self.edit.session.recovery_bytes(&self.deck)
    }

    /// Räknar fliken som ändrad (efter en återställning).
    pub fn mark_recovered(&mut self) {
        self.edit.session.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use crate::pptx::{edit_ops, Deck};
    use crate::recovery::{find_orphans, Recovery};

    fn tab() -> PptxTab {
        let bytes = super::super::media_tests::media_deck_bytes();
        PptxTab::with_source(Deck::open(&bytes).unwrap(), bytes)
    }

    #[test]
    fn säkerhetskopia_tur_och_retur_med_originalets_sökväg() {
        let dir = std::env::temp_dir().join(format!("kladd-prec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut t = tab();
        assert!(t.recovery_rev().is_none(), "ren presentation: ingen kopia");
        assert!(t.apply(None, |x| edit_ops::set_notes(x, 0, "Återställ mig åäö")));
        assert!(t.recovery_rev().is_some());
        let orig = std::path::PathBuf::from("/mapp/talet.pptx");
        let mut d = Document::pptx_tab(1, &orig, t);
        d.dirty = true;
        let mut r = Recovery::with_dir(dir.clone());
        let docs = vec![d];
        assert!(r.sync(&docs, true));
        r.shutdown(&docs);
        // Kraschen: ingen städning. En halv skrivning (tmp) får inte räknas som kopia.
        std::fs::write(dir.join("999-9.json.tmp"), b"halv").unwrap();
        std::fs::write(dir.join("999-9.pptx.tmp"), b"halv").unwrap();
        let found = find_orphans(&dir, &|_| false);
        assert_eq!(found.len(), 1);
        assert!(found[0].snap.pptx && found[0].snap.text.is_empty());
        let restored = found[0].into_document(2, 1);
        assert_eq!(restored.path.as_deref(), Some(orig.as_path()));
        assert!(restored.dirty);
        let rt = restored.pptx.as_ref().expect("öppnas som presentation");
        assert!(rt.unsaved(), "återställd = ändrad, Spara skriver till originalet");
        let want = docs[0].pptx.as_ref().unwrap().deck.pkg.parts.clone();
        let got = &rt.deck.pkg.parts;
        assert_eq!(want.len(), got.len());
        for (k, v) in &want {
            assert_eq!(got.get(k), Some(v), "delen {k} skiljer sig");
        }
        // Sparningen av kopian är en giltig zip.
        let bytes = rt.recovery_bytes().unwrap();
        assert!(Deck::open(&bytes).is_ok());
        found[0].discard();
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(left.iter().all(|n| n.starts_with("999-9")), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
