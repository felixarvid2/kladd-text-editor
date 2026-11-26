//! Styckeutseende (`pbox`) i layouten: avstånd, indrag, rutornas geometri.
use super::*;
use crate::word::pbox::{self, Border, Borders, LineStyle};
use crate::word::pboxui;

fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
    theme::doc_font(bold, italic)
}

fn measure(s: &str, _: FontId) -> f32 {
    s.chars().count() as f32 * 7.0
}

fn tight() -> String {
    "<!--k:pp sp=\"b=0 a=0\"-->".to_owned()
}

/// Kör `f` med paragrafer och ett utlagt galleri (bredd `wrap`).
fn with_galley<R>(t: &str, wrap: f32, f: impl FnOnce(&[Para], &Galley, &Params) -> R) -> R {
    let pal = theme::palette(false);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let img = |_: &str| None;
    let p = Params {
        palette: &pal,
        zoom: 1.0,
        reveal: 0..0,
        image_size: &img,
        measure: &measure,
        font_family: &fam,
        fonts_gen: 0,
    };
    let a = markup::analyze(t);
    let paras = paragraphs(t, &a, &p);
    let job = layout_job(t, &a, &p);
    let mut f = Some(f);
    let mut res = None;
    for frame in 0..2 {
        let out = ctx.run_ui(Default::default(), |ui| {
            let g = ui.ctx().fonts_mut(|fo| layout_galley(fo, job.clone(), &a, &p, wrap, 1.0));
            if frame == 1 {
                res = Some((f.take().unwrap())(&paras, &g, &p));
            }
        });
        std::mem::forget(out);
    }
    res.unwrap()
}

/// Gapet (px) på raden efter stycket som börjar med `after`.
fn gap_of(paras: &[Para], t: &str, after: &str) -> Option<f32> {
    let i = paras.iter().position(|q| t[q.range.clone()].starts_with(after)).unwrap();
    paras[i + 1].gap
}

#[test]
fn spacing_marker_sets_the_gap_between_paragraphs() {
    let t = format!("A{}\n\nB{}\n\nC\n", tight(), tight());
    with_galley(&t, 400.0, |paras, _, _| {
        assert_eq!(gap_of(paras, &t, "A"), Some(0.0));
        // B har markör: gapet till C beräknas (B:s efter 0, C:s före 0).
        assert_eq!(gap_of(paras, &t, "B"), Some(0.0));
        // C har ingen markör och inget par med markör efter sig.
        assert_eq!(gap_of(paras, &t, "C"), None);
    });
    // Största av efter och före, i punkter.
    let t = "A<!--k:pp sp=\"b=0 a=6\"-->\n\nB<!--k:pp sp=\"b=10 a=0\"-->\n";
    with_galley(t, 400.0, |paras, _, _| {
        assert!((gap_of(paras, t, "A").unwrap() - px(1.0, 10.0)).abs() < 0.01);
    });
    // Utan markör: som förut (ingen överskrivning).
    let t = "A\n\nB\n";
    with_galley(t, 400.0, |paras, _, _| assert!(paras.iter().all(|q| q.gap.is_none() && q.look.is_none())));
}

#[test]
fn tight_paragraphs_make_a_shorter_document() {
    let loose = "Ett\n\nTvå\n\nTre\n";
    let t = format!("Ett{0}\n\nTvå{0}\n\nTre{0}\n", tight());
    let h_loose = with_galley(loose, 400.0, |_, g, _| g.rect.height());
    let h_tight = with_galley(&t, 400.0, |_, g, _| g.rect.height());
    // Två tomma rader på 8 pt försvinner (markörerna är dolda och tar ingen höjd).
    assert!(h_loose - h_tight > 2.0 * px(1.0, 8.0) - 1.0, "{h_loose} {h_tight}");
}

#[test]
fn indent_narrows_and_shifts_the_paragraph() {
    let t = "Ett ganska långt stycke som får radbrytas flera gånger inuti det smala utrymmet.<!--k:pp ind=\"l=36 r=18 f=-18\"-->\n";
    let xs = with_galley(t, 300.0, |_, g, _| g.rows.iter().map(|r| r.pos.x).collect::<Vec<_>>());
    assert!(xs.len() >= 2, "{xs:?}");
    // Hängande: första raden 18 pt närmare kanten än resten.
    assert!((xs[1] - px(1.0, 36.0)).abs() < 0.6, "{xs:?}");
    assert!((xs[0] - px(1.0, 18.0)).abs() < 0.6, "{xs:?}");
    with_galley(t, 300.0, |_, g, _| {
        assert!(g.rows.iter().all(|r| r.pos.x + r.rect().width() <= 300.0 - px(1.0, 18.0) + 1.0));
    });
}

#[test]
fn exact_line_spacing_sets_row_height() {
    let t = "Rad ett som är så lång att den bryts över flera rader i ett smalt galleri.<!--k:pp sp=\"b=0 a=0 l=exact:30\"-->\n";
    with_galley(t, 200.0, |_, g, _| {
        assert!(g.rows.len() >= 3);
        // Sista raden är den tomma efter slutradbrytningen.
        for r in &g.rows[..g.rows.len() - 1] {
            assert!((r.rect().height() - px(1.0, 30.0)).abs() < 0.6, "{}", r.rect().height());
        }
    });
}

fn bd() -> Border {
    Border { style: LineStyle::Single, sz: 8, space: 4, color: None }
}

#[test]
fn joined_paragraphs_form_one_box_with_padding() {
    let b = pbox::PPr {
        borders: Borders { top: Some(bd()), left: Some(bd()), bottom: Some(bd()), right: Some(bd()), between: None },
        shd: Some([0xED, 0xE7, 0xF6]),
        before: Some(0.0),
        after: Some(0.0),
        ..Default::default()
    };
    let m = b.marker();
    let t = format!("Före\n\nett{m}\n\ntvå{m}\n\nEfter\n");
    with_galley(&t, 400.0, |paras, g, p| {
        // Mellan raderna i rutan: inget avstånd.
        assert_eq!(gap_of(paras, &t, "ett"), Some(0.0));
        let specs = pbox::scan(&t);
        let runs = pbox::box_runs(&t, &specs);
        assert_eq!(runs.len(), 1);
        let env = pboxui::Env {
            galley: g,
            gpos: Vec2::ZERO,
            text: &t,
            boxes: &[],
            col_at: &|_, _| (0.0, 400.0),
            zoom: 1.0,
            palette: p.palette,
            clip: Rect::from_min_max(egui::pos2(-100.0, -100.0), egui::pos2(900.0, 900.0)),
            vis: 0..t.len(),
        };
        let shapes = pboxui::shapes_rects(&env, &specs, &runs).0;
        let rects: Vec<Rect> = shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) => Some(r.rect),
                _ => None,
            })
            .collect();
        // Skuggningen är en enda rektangel över båda styckena, utanför texten med ramens luft.
        let fill = rects[0];
        let pad = bd().pad_pt() * px(1.0, 1.0);
        assert!((fill.left() + pad).abs() < 0.01 && (fill.right() - 400.0 - pad).abs() < 0.01, "{fill:?}");
        let i = t.find("ett").unwrap();
        let first = g.pos_from_cursor(egui::text::CCursor::new(t[..i].chars().count()));
        assert!((fill.top() - (first.top() - pad)).abs() < 0.6, "{fill:?} {first:?}");
        let j = t.find("två").unwrap();
        let second = g.pos_from_cursor(egui::text::CCursor::new(t[..j].chars().count()));
        assert!((fill.bottom() - (second.bottom() + pad)).abs() < 1.0, "{fill:?} {second:?}");
        // Fyra kantlinjer ovanpå fyllningen.
        assert_eq!(rects.len(), 5, "{rects:?}");
    });
}
