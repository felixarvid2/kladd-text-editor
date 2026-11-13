//! Prov för mallar: varje inbyggd mall tolkas, läggs ut och skrivs som docx/odt/pdf; platshållare; egna mallar.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use egui::{Color32, FontId};

use super::*;
use crate::word::layout::{Cache, Params};
use crate::word::pages::Geom;
use crate::word::{markup, sections};

fn tmp_dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("kladd-mallar-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn all() -> Vec<(Lang, Template)> {
    [Lang::Sv, Lang::En].into_iter().flat_map(|l| builtins(l).into_iter().map(move |t| (l, t))).collect()
}

fn fam(_: Option<&str>, bold: bool, italic: bool) -> egui::FontFamily {
    crate::theme::doc_font(bold, italic)
}

/// Lägger ut texten som dokumentläget gör; ger (galley, antal sidor).
fn lay(text: &str) -> (Arc<egui::epaint::text::Galley>, usize) {
    let pal = crate::theme::palette(false);
    let img = |_: &str| None;
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let a = markup::analyze(text);
    let geom = Geom::for_sections(1.0, &a.defaults, &a.sections);
    let mut cache = Cache::default();
    let mut out = None;
    for _ in 0..2 {
        let r = ctx.run_ui(Default::default(), |_| {
            let measure =
                |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
            let p = Params {
                palette: &pal,
                zoom: 1.0,
                reveal: 0..0,
                matches: &[],
                image_size: &img,
                measure: &measure,
                font_family: &fam,
                fonts_gen: 0,
            };
            let g = cache.galley_paged(&ctx, 1, text, &p, geom.edit_w(), Some(&geom));
            let pages = cache.page_boxes(1, text).len();
            out = Some((g, pages));
        });
        std::mem::forget(r);
    }
    out.unwrap()
}

/// Mallens text med alla platshållare ifyllda med ett ord.
fn filled(t: &Template) -> String {
    let values: Vec<(String, String)> = placeholders(&t.body).into_iter().map(|k| (k, "Ord".to_owned())).collect();
    create(t, &values)
}

#[test]
fn every_builtin_parses_in_both_languages() {
    for (l, t) in all() {
        let who = format!("{l:?} {}", t.id);
        assert!(!t.name.is_empty() && t.name != t.id, "{who}: namn saknas");
        assert!(!t.description.is_empty(), "{who}: beskrivning saknas");
        assert!(t.category != Category::Mine, "{who}: kategori saknas");
        assert!(!t.user);
        assert!(!t.body.starts_with("<!-- mall"), "{who}: beskrivningsraden ska inte följa med");
    }
    // Samma id:n och lägen på båda språken, och alla kategorier finns.
    let (sv, en) = (builtins(Lang::Sv), builtins(Lang::En));
    assert_eq!(sv.len(), en.len());
    for (a, b) in sv.iter().zip(&en) {
        assert_eq!((&a.id, a.doc, a.category), (&b.id, b.doc, b.category));
    }
    for c in Category::BUILTIN {
        assert!(sv.iter().any(|t| t.category == c), "{c:?} saknar mall");
    }
}

#[test]
fn modes_follow_the_kind_of_template() {
    let sv = builtins(Lang::Sv);
    let get = |id: &str| sv.iter().find(|t| t.id == id).unwrap();
    assert!(!get("anteckningar").doc && !get("readme").doc);
    for id in ["brev-formellt", "rapport", "faktura", "uppsats", "cv-modernt", "tom-a4", "tom-letter"] {
        assert!(get(id).doc, "{id}");
    }
    // Två varianter av CV och tom i både A4 och Letter.
    assert_eq!(sv.iter().filter(|t| t.category == Category::Cv).count(), 2);
    assert_eq!(sv.iter().filter(|t| t.category == Category::Blank).count(), 2);
}

#[test]
fn every_builtin_uses_valid_kladd_dialect() {
    for (l, t) in all().into_iter().filter(|(_, t)| t.doc) {
        let text = filled(&t);
        let a = markup::analyze(&text);
        let who = format!("{l:?} {}", t.id);
        // Grundformatsraden ska läsas (sidformat) och inte lämna okända deklarationer efter sig.
        if t.id == "tom-letter" || t.id == "tom-a4" {
            continue;
        }
        let cmt = text.lines().find(|l| l.starts_with("<!-- kladd:") && !sections::is_section_keyword(&l["<!-- kladd:".len()..].trim_end_matches("-->"))).unwrap_or_else(|| panic!("{who}: grundformat saknas"));
        let d = crate::style::DocDefaults::from_comment(cmt).unwrap();
        assert!(d.page_size.is_some() && d.page_margin.is_some(), "{who}: sida eller marginal saknas");
        // Raden skrivs om till samma innehåll: inga deklarationer förlorades vid tolkningen.
        let again = crate::style::DocDefaults::from_comment(&d.to_comment()).unwrap();
        assert_eq!((d.margins().map(f32::round), d.page(), &d.footer, &d.header, &d.styles), (again.margins().map(f32::round), again.page(), &again.footer, &again.header, &again.styles), "{who}");
        assert_eq!(a.defaults.page_size, d.page_size, "{who}");
    }
}

#[test]
fn report_has_toc_headings_and_page_numbers() {
    let sv = builtins(Lang::Sv);
    let r = sv.iter().find(|t| t.id == "rapport").unwrap();
    assert!(r.body.contains("<!-- kladd: toc -->"));
    assert!(r.body.contains("\n# 1. ") && r.body.contains("\n## 2.1 "));
    let d = markup::analyze(&r.body).defaults;
    assert!(d.footer.as_deref().is_some_and(|f| f.contains("{sida}") && f.contains("{sidor}")));
    assert!(d.header.is_some());
}

#[test]
fn essay_title_page_is_its_own_section() {
    for l in [Lang::Sv, Lang::En] {
        let t = builtins(l).into_iter().find(|t| t.id == "uppsats").unwrap();
        let a = markup::analyze(&filled(&t));
        assert_eq!(a.sections.len(), 2, "titelsida och brödtext");
        // Första sidan har ingen sidnumrering.
        assert_eq!(a.defaults.footer_first.as_deref(), Some(""));
        let (g, pages) = lay(&filled(&t));
        assert!(pages >= 2 && g.rows.len() > 10);
    }
}

#[test]
fn every_builtin_lays_out_and_paginates() {
    for (l, t) in all().into_iter().filter(|(_, t)| t.doc) {
        let text = filled(&t);
        let (g, pages) = lay(&text);
        assert!(pages >= 1, "{l:?} {}", t.id);
        if !t.body.trim().is_empty() && !t.body.lines().all(|x| x.starts_with("<!--")) {
            assert!(!g.rows.is_empty(), "{l:?} {}", t.id);
        }
    }
}

#[test]
fn every_builtin_writes_docx_odt_and_pdf() {
    let none_d = |_: &str| None;
    for (l, t) in all() {
        let who = format!("{l:?} {}", t.id);
        let text = filled(&t);
        let title = "Mall";
        let w = crate::docx::write(&text, title, &none_d, None).unwrap_or_else(|e| panic!("{who} docx: {e}"));
        assert!(w.bytes.starts_with(b"PK"), "{who}");
        let back = crate::docx::read(&w.bytes).unwrap_or_else(|e| panic!("{who} läsa docx: {e}"));
        assert!(back.dropped.is_empty(), "{who}: {:?}", back.dropped);
        let o = crate::odt::write(&text, title, &none_d).unwrap_or_else(|e| panic!("{who} odt: {e}"));
        crate::odt::read(&o.bytes).unwrap_or_else(|e| panic!("{who} läsa odt: {e}"));
        let p = crate::pdf_write::write(&text, title, &|_: &str| None).unwrap_or_else(|e| panic!("{who} pdf: {e}"));
        assert!(p.bytes.starts_with(b"%PDF"), "{who}");
    }
}

#[test]
fn docx_keeps_the_essay_sections_and_the_report_toc() {
    let sv = builtins(Lang::Sv);
    let essay = sv.iter().find(|t| t.id == "uppsats").unwrap();
    let w = crate::docx::write(&filled(essay), "x", &|_| None, None).unwrap();
    let back = crate::docx::read(&w.bytes).unwrap().markdown;
    assert_eq!(back.lines().filter(|l| l.starts_with("<!-- kladd: avsnitt")).count(), 1, "{back}");
    let report = sv.iter().find(|t| t.id == "rapport").unwrap();
    let w = crate::docx::write(&filled(report), "x", &|_| None, None).unwrap();
    let back = crate::docx::read(&w.bytes).unwrap().markdown;
    assert!(back.contains("kladd: toc"), "{back}");
    assert!(back.contains("{sida}"), "{back}");
}

// ---------------------------------------------------------------- platshållare

fn vals(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter().map(|(k, x)| (k.to_string(), x.to_string())).collect()
}

#[test]
fn placeholders_are_found_once_in_order() {
    let t = "Hej {{namn}}, {{datum}} och {{namn}} igen. {{ }} {{a b}} {{ärende}} {x} {{oavslutad";
    assert_eq!(placeholders(t), vec!["namn", "datum", "ärende"]);
    assert_eq!(placeholders("{{{namn}}}"), vec!["namn"]);
    assert!(placeholders("ingen här").is_empty());
}

#[test]
fn substitution_fills_values() {
    let out = fill("Hej {{namn}}!\n\n{{datum}}", &vals(&[("namn", "Åsa"), ("datum", "2026-11-13")]), true);
    assert_eq!(out, "Hej Åsa!\n\n2026-11-13");
    // Värdet trimmas och radbrytningar blir mellanslag.
    assert_eq!(fill("{{a}}", &vals(&[("a", "  två\nrader ")]), true), "två rader");
}

#[test]
fn empty_placeholders_stay_highlighted() {
    let out = fill("Hej {{namn}}", &vals(&[("namn", "  ")]), true);
    assert_eq!(out, "Hej <span style=\"background-color: #FFFF00\">{{namn}}</span>");
    // Utan värde alls likadant, och en markerad platshållare går att hitta igen.
    assert_eq!(fill("{{x}}", &[], true), "<span style=\"background-color: #FFFF00\">{{x}}</span>");
    assert_eq!(placeholders(&out), vec!["namn"]);
    // Textläge (Markdown) och kommentarsrader får ingen markering.
    assert_eq!(fill("Hej {{namn}}", &[], false), "Hej {{namn}}");
    let cmt = "<!-- kladd: footer: \"{{namn}}\" -->";
    assert_eq!(fill(cmt, &[], true), cmt);
    assert_eq!(fill(cmt, &vals(&[("namn", "Åsa")]), true), "<!-- kladd: footer: \"Åsa\" -->");
}

#[test]
fn pipes_in_table_values_are_escaped() {
    assert_eq!(fill("| {{a}} | b |", &vals(&[("a", "x|y")]), true), "| x\\|y | b |");
    assert_eq!(fill("{{a}}", &vals(&[("a", "x|y")]), true), "x|y");
}

#[test]
fn highlighted_placeholder_renders_without_error() {
    let sv = builtins(Lang::Sv);
    let t = sv.iter().find(|t| t.id == "brev-formellt").unwrap();
    let text = create(t, &vals(&[("namn", "Åsa Berg")]));
    assert!(text.contains("background-color"));
    let (g, pages) = lay(&text);
    assert!(pages >= 1 && !g.rows.is_empty());
    crate::docx::write(&text, "x", &|_| None, None).unwrap();
}

#[test]
fn prefill_uses_date_and_author() {
    let keys: Vec<String> = ["datum", "namn", "mottagare", "date", "Name"].map(String::from).to_vec();
    let v = prefill(&keys, "  Åsa Berg ");
    assert_eq!(v[0], today_text());
    assert_eq!(v[1], "Åsa Berg");
    assert_eq!(v[2], "");
    assert_eq!(v[3], today_text());
    assert_eq!(v[4], "Åsa Berg");
    assert!(!today_text().is_empty());
    assert_eq!(label("mottagarens-adress"), "Mottagarens adress");
    assert_eq!(label("ärende"), "Ärende");
    assert_eq!(label("e-post"), "E-post");
}

// ---------------------------------------------------------------- egna mallar

#[test]
fn user_template_save_load_delete() {
    let dir = tmp_dir();
    assert!(list_user(&dir).is_empty());
    let text = "<!-- kladd: page-size: A5 -->\n\n# Hej {{namn}}\n";
    let id = save_user(&dir, "Min mall", text, true, false).unwrap();
    assert_eq!(id, "Min mall");
    assert!(dir.join("Min mall.md").exists());
    let src = std::fs::read_to_string(dir.join("Min mall.md")).unwrap();
    assert!(src.starts_with("<!-- mall; namn: Min mall; läge: dokument; skapad: "), "{src}");
    let t = load_user(&dir, &id).unwrap();
    assert!(t.user && t.doc && t.category == Category::Mine);
    assert_eq!(t.body, text, "brödtexten är orörd");
    assert_eq!(placeholders(&t.body), vec!["namn"]);
    // Textläge sparas och läses tillbaka.
    save_user(&dir, "Anteckning", "# x\n", false, false).unwrap();
    let names: Vec<(String, bool)> = list_user(&dir).into_iter().map(|t| (t.name, t.doc)).collect();
    assert_eq!(names, vec![("Anteckning".to_owned(), false), ("Min mall".to_owned(), true)]);
    // Samma namn kräver bekräftelse; då ersätts mallen utan att huvudet dubbleras.
    assert!(save_user(&dir, "Min mall", "ny", true, false).is_err());
    save_user(&dir, "Min mall", &src, true, true).unwrap();
    assert_eq!(load_user(&dir, "Min mall").unwrap().body, text);
    delete_user(&dir, "Min mall").unwrap();
    assert!(load_user(&dir, "Min mall").is_err());
    assert_eq!(list_user(&dir).len(), 1);
    assert!(delete_user(&dir, "Min mall").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn user_template_rename() {
    let dir = tmp_dir();
    save_user(&dir, "Ett", "# a\n", true, false).unwrap();
    save_user(&dir, "Två", "# b\n", false, false).unwrap();
    assert_eq!(rename_user(&dir, "Ett", "Tre").unwrap(), "Tre");
    assert!(!dir.join("Ett.md").exists());
    let t = load_user(&dir, "Tre").unwrap();
    assert!(t.doc);
    assert_eq!(t.body, "# a\n");
    assert!(rename_user(&dir, "Tre", "Två").is_err(), "finns redan");
    assert!(rename_user(&dir, "Tre", "  ").is_err());
    assert_eq!(rename_user(&dir, "Tre", "Tre").unwrap(), "Tre");
    assert!(!load_user(&dir, "Två").unwrap().doc, "läget behålls");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn names_become_safe_file_names() {
    assert_eq!(sanitize_name("  Brev/till: mamma?  ").as_deref(), Some("Brev-till- mamma-"));
    assert_eq!(sanitize_name("..dolt").as_deref(), Some("dolt"));
    assert_eq!(sanitize_name(" \t "), None);
    assert_eq!(sanitize_name("…").as_deref(), Some("…"));
    assert_eq!(sanitize_name(&"å".repeat(200)).unwrap().chars().count(), 80);
    let dir = tmp_dir();
    let id = save_user(&dir, "../ut", "x", true, false).unwrap();
    assert!(dir.join(format!("{id}.md")).exists(), "stannar i mallkatalogen");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn metadata_header_roundtrips_and_is_optional() {
    let (m, body) = split_meta("<!-- mall; namn: A; läge: text -->\ninnehåll\n");
    assert_eq!(m, vec![("namn".into(), "A".into()), ("läge".into(), "text".into())]);
    assert_eq!(body, "innehåll\n");
    let (m, body) = split_meta("# Rubrik\n");
    assert!(m.is_empty() && body == "# Rubrik\n");
    // Ett namn med semikolon ändrar inte strukturen.
    let line = meta_line("a; läge: text", true);
    let t = parse("x", &format!("{line}\nb"), true);
    assert!(t.doc, "{line}");
    // Mallar utan huvud är dokumentmallar.
    assert!(parse("x", "# a", true).doc);
}

#[test]
fn imports_strip_images_and_markers() {
    let md = "# Rubrik\n\n![Sol](bild:image1.png) text <!--k:3--> mer\n\n<img src=\"bild:image2.png\" alt=\"x\" width=\"240\">\n\n![ok](https://x.se/a.png)\n";
    let s = strip_unportable(md);
    assert!(!s.contains("bild:") && !s.contains("<!--k:"), "{s}");
    assert!(s.contains("text  mer") && s.contains("![ok](https://x.se/a.png)"));
}

#[test]
fn docx_and_odt_import_as_templates() {
    let md = "<!-- kladd: page-size: A5 -->\n\n# Titel {{namn}}\n\nBrödtext.\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n";
    let docx = crate::docx::write(md, "x", &|_| None, None).unwrap().bytes;
    let odt = crate::odt::write(md, "x", &|_| None).unwrap().bytes;
    for (bytes, ext) in [(docx, "docx"), (odt, "odt")] {
        let (text, images) = import_bytes(&bytes, ext).unwrap();
        assert_eq!(images, 0);
        assert!(text.contains("# Titel") && text.contains("{{namn}}") && text.contains("| A | B |"), "{ext}: {text}");
        let dir = tmp_dir();
        let id = save_user(&dir, "Importerad", &text, true, false).unwrap();
        assert_eq!(placeholders(&load_user(&dir, &id).unwrap().body), vec!["namn"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(import_bytes(b"x", "pdf").is_err());
    assert!(import_bytes(b"inte en zip", "docx").is_err());
    assert_eq!(name_from_path(std::path::Path::new("/a/b/Brev.docx")), "Brev");
}

#[test]
fn sketch_describes_the_page() {
    let body = "<!-- kladd: page-size: A4 -->\n\n# Rubrik\n\ntext\n\n- punkt\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n";
    let s = sketch(body, 20);
    assert!(matches!(s[0], Sketch::Heading(1, _)));
    assert!(s.contains(&Sketch::Row(2)));
    assert!(s.iter().any(|x| matches!(x, Sketch::Item(_))));
    assert!(!matches!(s.last(), Some(Sketch::Gap)));
    assert!(sketch(body, 2).len() <= 2);
    assert!(page_aspect("<!-- kladd: page-size: Letter -->\n") > page_aspect("<!-- kladd: page-size: A4 -->\n"));
    assert!(page_aspect("<!-- kladd: page-size: A4 landscape -->\n") > 1.0);
    for (_, t) in all() {
        let _ = sketch(&t.body, 30);
    }
}
