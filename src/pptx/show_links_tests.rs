//! Prov för länkar och åtgärdsknappar i bildspelet och för bildnummer med siffror + Enter.

use std::io::Write;
use std::sync::Arc;

use egui::{Event, Key, Modifiers, RawInput, Rect, pos2};

use super::presenter::ShowView;
use super::show_links::{self, Action};
use super::slideshow::{Screen, Show};
use super::Deck;

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn sp(id: u32, name: &str, nv_extra: &str, rect: (i64, i64, i64, i64), body: &str) -> String {
    let (x, y, w, h) = rect;
    // Former utan egen text får namnet som etikett (syns i skärmbilderna).
    let label;
    let body = if body.is_empty() {
        label = format!(r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="sv-SE" sz="1100"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:rPr><a:t>{name}</a:t></a:r></a:p></p:txBody>"#);
        label.as_str()
    } else {
        body
    };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}">{nv_extra}</p:cNvPr><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="DDDDDD"/></a:solidFill></p:spPr>{body}</p:sp>"#
    )
}

/// Bild 1: åtgärdsknappar (nästa, bild 3, avsluta vid hover, hlinkfile som inte utförs) och en text med en
/// webblänk. Bild 2 och 4: tomma. Bild 3: tom (mål för länken).
pub fn links_deck_bytes() -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opt).unwrap();
        z.write_all(data).unwrap();
    };
    put("[Content_Types].xml", br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#);
    put("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#).as_bytes());
    let tree = |inner: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#);
    let click = |a: &str, id: &str| format!(r#"<a:hlinkClick r:id="{id}" action="{a}"/>"#);
    let text = r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE" sz="4000"/><a:t>Besök </a:t></a:r><a:r><a:rPr lang="sv-SE" sz="4000"><a:hlinkClick r:id="rIdWeb"/></a:rPr><a:t>Kladd</a:t></a:r><a:r><a:rPr lang="sv-SE" sz="4000"/><a:t> nu</a:t></a:r></a:p></p:txBody>"#;
    let s1 = format!(
        r#"<p:sld {NS}>{}</p:sld>"#,
        tree(
            &(sp(2, "Nästa", &click("ppaction://hlinkshowjump?jump=nextslide", ""), (0, 0, 914400, 914400), "")
                + &sp(3, "Till bild 3", &click("ppaction://hlinksldjump", "rIdS3"), (1000000, 0, 914400, 914400), "")
                + &sp(4, "Avsluta", r#"<a:hlinkHover r:id="" action="ppaction://hlinkshowjump?jump=endshow"/>"#, (2000000, 0, 914400, 914400), "")
                + &sp(5, "Fil", &click("ppaction://hlinkfile", "rIdWeb"), (3000000, 0, 914400, 914400), "")
                + &sp(6, "Text", "", (0, 3000000, 8000000, 1000000), text)
                + &sp(7, "Fil som skadlig url", &click("", "rIdBad"), (4000000, 0, 914400, 914400), ""))
        )
    );
    let titled = |n: usize| format!(r#"<p:sld {NS}>{}</p:sld>"#, tree(&sp(2, "Rubrik", "", (1000000, 2500000, 7000000, 1500000), &format!(r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE" sz="6000"/><a:t>Bild {n}</a:t></a:r></a:p></p:txBody>"#))));
    let mut ids = String::new();
    let mut rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#);
    for n in 1..=4 {
        ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rIdS{n}"/>"#, 255 + n));
        rels.push_str(&format!(r#"<Relationship Id="rIdS{n}" Type="{REL}/slide" Target="slides/slide{n}.xml"/>"#));
        put(&format!("ppt/slides/slide{n}.xml"), &if n == 1 { s1.as_bytes().to_vec() } else { titled(n).into_bytes() });
        let mut sr = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
        if n == 1 {
            sr.push_str(&format!(r#"<Relationship Id="rIdS3" Type="{REL}/slide" Target="slide3.xml"/><Relationship Id="rIdWeb" Type="{REL}/hyperlink" Target="https://example.org/kladd" TargetMode="External"/><Relationship Id="rIdBad" Type="{REL}/hyperlink" Target="file:///etc/passwd" TargetMode="External"/>"#));
        }
        sr.push_str("</Relationships>");
        put(&format!("ppt/slides/_rels/slide{n}.xml.rels"), sr.as_bytes());
    }
    rels.push_str("</Relationships>");
    put("ppt/_rels/presentation.xml.rels", rels.as_bytes());
    put("ppt/presentation.xml", format!(r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#).as_bytes());
    put("ppt/slideMasters/slideMaster1.xml", format!(r#"<p:sldMaster {NS}>{}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:sldMaster>"#, tree("")).as_bytes());
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#).as_bytes());
    put("ppt/slideLayouts/slideLayout1.xml", format!(r#"<p:sldLayout {NS}>{}</p:sldLayout>"#, tree("")).as_bytes());
    put("ppt/slideLayouts/_rels/slideLayout1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#).as_bytes());
    z.finish().unwrap().into_inner()
}

fn deck() -> Arc<Deck> {
    Arc::new(Deck::open(&links_deck_bytes()).expect("presentationen öppnas"))
}

/// Punkter på bild 1 (720 × 540 pt): mitt i en form av 72 × 72 pt som börjar vid EMU `x`.
fn at_emu(x: i64) -> (f32, f32) {
    (x as f32 / 12700.0 + 36.0, 36.0)
}

fn all_visible(_: u32) -> bool {
    true
}

#[test]
fn åtgärder_läses_ur_formlänkar() {
    let d = deck();
    let l = show_links::of_slide(&d, 0);
    let act = |x: i64| {
        let (px, py) = at_emu(x);
        l.at(px, py, &all_visible).map(|(_, a)| a.link.clone())
    };
    assert_eq!(act(0).unwrap().click, Some(Action::Next));
    assert_eq!(act(1_000_000).unwrap().click, Some(Action::Slide(2)));
    let hover = act(2_000_000).unwrap();
    assert_eq!((hover.click, hover.hover), (None, Some(Action::End)));
    // `hlinkfile` och adresser som inte är webb eller e-post utförs inte.
    assert!(act(3_000_000).is_none());
    assert!(act(4_000_000).is_none());
    // En form som animeringen döljer går inte att klicka på.
    assert!(l.at(36.0, 36.0, &|_| false).is_none());
    // Tomt utanför formerna.
    assert!(l.at(600.0, 500.0, &all_visible).is_none());
}

#[test]
fn textlänken_träffas_bara_på_de_länkade_orden() {
    let d = deck();
    let l = show_links::of_slide(&d, 0);
    // Texten står i formen vid y = 3 000 000 EMU (236 pt) och är 40 pt hög; "Besök " först, så "Kladd" börjar
    // en bit in. Sök över raden efter träffar.
    let y = 3_000_000.0 / 12700.0 + 25.0;
    let hits: Vec<f32> = (0..600).map(|x| x as f32).filter(|&x| l.at(x, y, &all_visible).is_some_and(|(_, a)| a.link.click == Some(Action::Url("https://example.org/kladd".into())))).collect();
    assert!(!hits.is_empty(), "ingen träff på textlänken");
    let (first, last) = (hits[0], *hits.last().unwrap());
    // Orden före och efter länken ska inte träffa: bredden på "Kladd" vid 40 pt är ungefär 5 tecken.
    assert!(first > 60.0, "länken börjar för tidigt: {first}");
    assert!(last - first < 200.0, "länken är för bred: {first}..{last}");
    assert!(l.at(last + 150.0, y, &all_visible).is_none_or(|(_, a)| a.link.click != Some(Action::Url("https://example.org/kladd".into()))));
}

fn view() -> ShowView {
    let d = deck();
    ShowView::new(d, Show::from_beginning(&[false; 4]).unwrap(), true)
}

fn full() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(720.0, 540.0))
}

#[test]
fn klick_på_åtgärdsknappar_byter_bild_eller_avslutar() {
    let mut v = view();
    // Nästa-knappen.
    let (x, y) = at_emu(0);
    assert!(v.click_slide(full(), Some(pos2(x, y)), 1.0));
    assert_eq!(v.slide_screen(), Screen::Slide(1));
    // Tillbaka till bild 1, sedan knappen till bild 3.
    let mut v = view();
    let (x, y) = at_emu(1_000_000);
    assert!(v.click_slide(full(), Some(pos2(x, y)), 1.0));
    assert_eq!(v.slide_screen(), Screen::Slide(2));
    // Klick utanför knapparna går till nästa steg (bild 2).
    let mut v = view();
    assert!(v.click_slide(full(), Some(pos2(600.0, 500.0)), 1.0));
    assert_eq!(v.slide_screen(), Screen::Slide(1));
}

#[test]
fn senast_visade_bilden_minns_hoppet() {
    let mut v = view();
    let (x, y) = at_emu(1_000_000);
    assert!(v.click_slide(full(), Some(pos2(x, y)), 1.0));
    assert_eq!(v.slide_screen(), Screen::Slide(2));
    assert!(v.run_action_for_test(Action::LastViewed, 2.0));
    assert_eq!(v.slide_screen(), Screen::Slide(0));
    assert!(!v.run_action_for_test(Action::End, 3.0));
}

fn press(ctx: &egui::Context, v: &mut ShowView, k: Key) -> bool {
    let ev = |pressed| Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers: Modifiers::NONE };
    let mut keep = true;
    ctx.run_ui(RawInput { events: vec![ev(true), ev(false)], ..Default::default() }, |ui| keep = v.keys(ui.ctx())).drop_without_applying_deltas();
    keep
}

#[test]
fn siffror_och_enter_hoppar_till_bildnumret() {
    let mut v = view();
    let ctx = egui::Context::default();
    assert!(press(&ctx, &mut v, Key::Num3));
    // Enter hoppar när siffror skrivits, och går inte vidare som "nästa".
    assert!(press(&ctx, &mut v, Key::Enter));
    assert_eq!(v.slide_screen(), Screen::Slide(2));
    // Utanför intervallet och 0 ignoreras.
    assert!(press(&ctx, &mut v, Key::Num9));
    assert!(press(&ctx, &mut v, Key::Enter));
    assert_eq!(v.slide_screen(), Screen::Slide(2));
    assert!(press(&ctx, &mut v, Key::Num0));
    assert!(press(&ctx, &mut v, Key::Enter));
    assert_eq!(v.slide_screen(), Screen::Slide(2));
    // Flera siffror: 1 och sedan Backspace tar bort den; Enter är då vanligt "nästa".
    assert!(press(&ctx, &mut v, Key::Num1));
    assert!(press(&ctx, &mut v, Key::Backspace));
    assert!(press(&ctx, &mut v, Key::Enter));
    assert_eq!(v.slide_screen(), Screen::Slide(3));
}

/// Skriver provpresentationen till `$KLADD_PLINK_OUT` (för att titta på bildspelet för hand).
#[test]
#[ignore]
fn skriv_provfil() {
    let Ok(p) = std::env::var("KLADD_PLINK_OUT") else { return };
    std::fs::write(p, links_deck_bytes()).unwrap();
}
