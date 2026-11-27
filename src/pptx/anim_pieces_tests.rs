//! Prov på utlösare, bokstav för bokstav och mediekommandon genom hela vägen: egna minimala .pptx
//! (zip i minnet) med text och tidsträd; lagerdelning, delarnas tillstånd och träffprov.

use std::io::Write;

use resvg::tiny_skia as sk;

use super::anim::{self, Geom, MediaCmd, MediaKind, State, Target, Timeline, UnitKind};
use super::anim_layers::{self, LayerKind};
use super::scene::{Item, NO_CHAR, Scene};
use super::{raster, render, Deck};

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const RELS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Prov"><a:themeElements>
<a:clrScheme name="Prov"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="Prov"><a:majorFont><a:latin typeface="Calibri Light"/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/></a:minorFont></a:fontScheme>
<a:fmtScheme name="Prov"><a:fillStyleLst/><a:lnStyleLst/><a:effectStyleLst/><a:bgFillStyleLst/></a:fmtScheme></a:themeElements></a:theme>"#;

/// En presentation med en bild: `shapes` i formträdet och `timing` (`<p:timing>…`, eller tomt).
pub(crate) fn deck(shapes: &str, timing: &str) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opt).unwrap();
        z.write_all(data).unwrap();
    };
    put("[Content_Types].xml", br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/></Types>"#);
    put("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#).as_bytes());
    put("ppt/_rels/presentation.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{RELS}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rIdS1" Type="{RELS}/slide" Target="slides/slide1.xml"/></Relationships>"#).as_bytes());
    put("ppt/presentation.xml", format!(r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rIdS1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#).as_bytes());
    let tree = |s: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{s}</p:spTree></p:cSld>"#);
    put("ppt/slideMasters/slideMaster1.xml", format!(r#"<p:sldMaster {NS}>{}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:sldMaster>"#, tree("")).as_bytes());
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{RELS}/theme" Target="../theme/theme1.xml"/></Relationships>"#).as_bytes());
    put("ppt/slideLayouts/slideLayout1.xml", format!(r#"<p:sldLayout {NS}>{}</p:sldLayout>"#, tree("")).as_bytes());
    put("ppt/slideLayouts/_rels/slideLayout1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#).as_bytes());
    put("ppt/theme/theme1.xml", THEME.as_bytes());
    put("ppt/slides/slide1.xml", format!(r#"<p:sld {NS}>{}{timing}</p:sld>"#, tree(shapes)).as_bytes());
    put("ppt/slides/_rels/slide1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#).as_bytes());
    z.finish().unwrap().into_inner()
}

/// En rektangel utan text (`id`) på (x, y) med storlek (w, h) i punkter.
pub(crate) fn rect(id: u32, x: f32, y: f32, w: f32, h: f32) -> String {
    let e = |v: f32| (v * 12700.0) as i64;
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Form {id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="4472C4"/></a:solidFill></p:spPr></p:sp>"#,
        e(x),
        e(y),
        e(w),
        e(h)
    )
}

/// Textformen 4 med styckena `paras` (stycke 0 med punkt om `bullet`) och skugga på texten.
fn text_shape(paras: &[&str], bullet: bool) -> String {
    let mut body = String::new();
    for (i, t) in paras.iter().enumerate() {
        let ppr = if bullet && i == 0 { r#"<a:pPr marL="342900" indent="-342900"><a:buChar char="•"/></a:pPr>"# } else { "" };
        body.push_str(&format!(
            r#"<a:p>{ppr}<a:r><a:rPr lang="sv-SE" sz="3200"><a:solidFill><a:srgbClr val="202020"/></a:solidFill><a:effectLst><a:outerShdw blurRad="38100" dist="38100" dir="2700000"><a:srgbClr val="000000"><a:alpha val="40000"/></a:srgbClr></a:outerShdw></a:effectLst></a:rPr><a:t>{t}</a:t></a:r></a:p>"#
        ));
    }
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="4" name="Text"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="914400" y="914400"/><a:ext cx="7000000" cy="2400000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr wrap="square"/><a:lstStyle/>{body}</p:txBody></p:sp>"#
    )
}

fn timing(main: &str, seqs: &str) -> String {
    format!(
        r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>{main}</p:childTnLst></p:cTn></p:seq>{seqs}</p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"#
    )
}

/// Tona in `tgt` på `ms` med `iterate`-elementet `it`.
fn fade(tgt: &str, it: &str, ms: u32) -> String {
    format!(
        r#"<p:par><p:cTn presetID="10" presetClass="entr" presetSubtype="0" fill="hold" nodeType="clickEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst>{it}<p:childTnLst><p:set><p:cBhvr><p:cTn dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn>{tgt}<p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set><p:animEffect transition="in" filter="fade"><p:cBhvr><p:cTn dur="{ms}"/>{tgt}</p:cBhvr></p:animEffect></p:childTnLst></p:cTn></p:par>"#
    )
}

fn click(effects: &str) -> String {
    format!(r#"<p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst>{effects}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par>"#)
}

fn trigger(shape: u32, effects: &str) -> String {
    format!(
        r#"<p:seq concurrent="1" nextAc="seek"><p:cTn restart="whenNotActive" fill="hold" evtFilter="cancelBubble" nodeType="interactiveSeq"><p:stCondLst><p:cond evt="onClick" delay="0"><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl></p:cond></p:stCondLst><p:childTnLst><p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst>{effects}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:seq>"#
    )
}

const WHOLE: &str = r#"<p:tgtEl><p:spTgt spid="4"/></p:tgtEl>"#;

fn geom() -> Geom {
    Geom { cx: 0.5, cy: 0.5, w: 0.2, h: 0.1 }
}

/// Bilden delad i lager för tidsträdet, med lagrens tillstånd `t` sekunder in i klicksteg 1
/// (`t < 0`: före klicket).
struct Fixture {
    scene: Scene,
    tl: Timeline,
    layers: Vec<anim_layers::LayerItems>,
}

impl Fixture {
    fn new(shapes: &str, timing: &str) -> Fixture {
        let d = Deck::open(&deck(shapes, timing)).unwrap();
        let scene = render::slide_scene(&d, 0);
        let tl = Timeline::of_slide(&d.slides[0].root);
        let layers = anim_layers::split_units(&scene, &tl.targets(), &tl.unit_scopes());
        Fixture { scene, tl, layers }
    }

    fn kinds(&self) -> Vec<LayerKind> {
        self.layers.iter().map(|l| l.kind).collect()
    }

    fn state_of(&self, i: usize, t: f32) -> State {
        let chain = if t < 0.0 { 0..0 } else { self.tl.chain(1) };
        let t = t.max(0.0);
        match self.layers[i].kind {
            LayerKind::Static => State::SHOWN,
            LayerKind::Target(tg) => self.tl.state_unit(tg, None, geom(), chain, t),
            LayerKind::Piece { scope, unit } => self.tl.state_unit(scope, Some(unit), geom(), chain, t),
            LayerKind::Rest(scope) => self.tl.state_unit(scope, Some(0), geom(), chain, t),
        }
    }

    fn plan(&self, t: f32) -> Vec<Option<State>> {
        anim_layers::plan(&self.kinds(), &|i| self.state_of(i, t))
    }

    fn pieces(&self) -> Vec<usize> {
        (0..self.layers.len()).filter(|&i| matches!(self.layers[i].kind, LayerKind::Piece { .. })).collect()
    }

    /// Lagren som ritas i `t`, ihoplagda (opacitet och förskjutning ignoreras: bara de som ritas).
    fn composite(&self, t: f32, w: u32, h: u32) -> sk::Pixmap {
        let rendered = anim_layers::render(&self.scene, &self.layers, w, h);
        let mut acc = sk::Pixmap::new(w, h).unwrap();
        for (r, st) in rendered.iter().zip(self.plan(t)) {
            if !st.is_some_and(|s| s.drawn()) {
                continue;
            }
            let pm = sk::Pixmap::from_vec(r.rgba.clone(), sk::IntSize::from_wh(r.px.2, r.px.3).unwrap()).unwrap();
            acc.draw_pixmap(r.px.0 as i32, r.px.1 as i32, pm.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), None);
        }
        acc
    }
}

fn glyph_chars(items: &[Item]) -> Vec<u32> {
    items.iter().filter_map(|i| if let Item::Glyphs(g) = i { Some(g.glyphs.iter().map(|p| p.ch)) } else { None }).flatten().filter(|c| *c != NO_CHAR).collect()
}

const LT: &str = r#"<p:iterate type="lt"><p:tmPct val="10000"/></p:iterate>"#;
const WD: &str = r#"<p:iterate type="wd"><p:tmAbs val="300"/></p:iterate>"#;

#[test]
fn bokstavsdelningen_ger_samma_bild_som_helheten() {
    let f = Fixture::new(&text_shape(&["Hej du glada vänner", "Andra stycket"], false), &timing(&click(&fade(WHOLE, LT, 500)), ""));
    let pieces = f.pieces();
    assert!(pieces.len() >= 28, "en del per tecken: {}", pieces.len());
    // Inga tecken försvinner eller dubbleras: delarna har lika många glyfer som helhetslagret.
    let whole = f.layers.iter().find(|l| matches!(l.kind, LayerKind::Target(_))).unwrap();
    let mut got: Vec<u32> = pieces.iter().flat_map(|&i| glyph_chars(&f.layers[i].items)).collect();
    got.sort_unstable();
    let mut want = glyph_chars(&whole.items);
    want.sort_unstable();
    assert_eq!(got, want);
    // Före klicket är allt dolt: helheten ritas (dold), inga delar.
    let p0 = f.plan(-1.0);
    for (i, l) in f.layers.iter().enumerate() {
        match l.kind {
            LayerKind::Target(_) => assert!(p0[i].is_some_and(|s| !s.drawn())),
            LayerKind::Piece { .. } | LayerKind::Rest(_) => assert!(p0[i].is_none()),
            LayerKind::Static => {}
        }
    }
    // Mitt i: delarna ritas var för sig, helheten inte, och först och sist skiljer sig åt.
    let mid = f.plan(0.3);
    let whole_i = f.layers.iter().position(|l| matches!(l.kind, LayerKind::Target(_))).unwrap();
    assert!(mid[whole_i].is_none());
    let first = mid[pieces[0]].unwrap();
    let last = mid[*pieces.last().unwrap()].unwrap();
    assert!(first.drawn() && !last.drawn(), "{first:?} {last:?}");
    // Klart: helheten ritas som vanligt och bilden är samma som den statiska (±2).
    let (w, h) = (400u32, 300u32);
    let done = f.composite(f32::INFINITY, w, h);
    let still = raster::render(&f.scene, w, h).unwrap();
    let diff = still.data().iter().zip(done.data()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(diff <= 2, "slutläget skiljer {diff}");
    // Och efter den sista delens slut (31 · 50 ms + 500 ms).
    let late = f.composite(2.1, w, h);
    assert_eq!(late.data(), done.data());
}

#[test]
fn orddelning_och_stycke_med_punkt() {
    let f = Fixture::new(&text_shape(&["Hej du glada vänner", "Andra stycket"], true), &timing(&click(&fade(WHOLE, WD, 400)), ""));
    assert_eq!(f.tl.unit_scopes()[0].kind, UnitKind::Words);
    // Fem ord i hela formens text (4 + 2 = 6 ord): del 0..=5.
    let units: Vec<u32> = f.pieces().iter().map(|&i| if let LayerKind::Piece { unit, .. } = f.layers[i].kind { unit } else { 0 }).collect();
    assert_eq!(units, vec![0, 1, 2, 3, 4, 5]);
    // Punkten hör inte till något ord: den ligger i resten.
    let rest = f.layers.iter().find(|l| matches!(l.kind, LayerKind::Rest(_))).expect("rest");
    assert!(rest.items.iter().any(|i| matches!(i, Item::Glyphs(g) if g.glyphs.iter().all(|p| p.ch == NO_CHAR))));
    // Ord 1 börjar 0,3 s efter ord 0.
    let t = f.tl.state_unit(Target { shape: 4, para: None }, Some(1), geom(), f.tl.chain(1), 0.2);
    assert!(!t.visible);
    let t = f.tl.state_unit(Target { shape: 4, para: None }, Some(1), geom(), f.tl.chain(1), 0.5);
    assert!(t.visible && t.opacity > 0.4 && t.opacity < 0.6, "{t:?}");
    let (w, h) = (400u32, 300u32);
    let still = raster::render(&f.scene, w, h).unwrap();
    let done = f.composite(f32::INFINITY, w, h);
    let diff = still.data().iter().zip(done.data()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(diff <= 2, "{diff}");
}

#[test]
fn stycke_för_stycke_med_bokstäver_delar_bara_stycket() {
    let tgt = r#"<p:tgtEl><p:spTgt spid="4"><p:txEl><p:pRg st="1" end="1"/></p:txEl></p:spTgt></p:tgtEl>"#;
    let f = Fixture::new(&text_shape(&["Första", "Andra"], false), &timing(&click(&fade(tgt, LT, 400)), ""));
    let sc = f.tl.unit_scopes();
    assert_eq!((sc[0].shape, sc[0].para), (4, Some(1)));
    let n = f.pieces().len();
    assert_eq!(n, 5, "bara 'Andra' delas: {n}");
    // Delarna har styckets egna teckennummer 0..5.
    let mut chars: Vec<u32> = f.pieces().iter().flat_map(|&i| glyph_chars(&f.layers[i].items)).collect();
    chars.sort_unstable();
    assert_eq!(chars, vec![0, 1, 2, 3, 4]);
    // Stycke 0 ligger kvar i det fasta lagret.
    let still = raster::render(&f.scene, 400, 300).unwrap();
    let done = f.composite(f32::INFINITY, 400, 300);
    let diff = still.data().iter().zip(done.data()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(diff <= 2, "{diff}");
    // Före klicket syns stycke 0 men inte stycke 1.
    let before = f.composite(-1.0, 400, 300);
    assert_ne!(before.data(), done.data());
}

#[test]
fn teckenintervall_ritar_resten_och_delarna_var_för_sig() {
    let tgt = r#"<p:tgtEl><p:spTgt spid="4"><p:txEl><p:charRg st="4" end="5"/></p:txEl></p:spTgt></p:tgtEl>"#;
    let f = Fixture::new(&text_shape(&["Hej du glada"], false), &timing(&click(&fade(tgt, "", 400)), ""));
    // Före klicket skiljer sig delarna (tecknen 4 och 5 dolda): delarna ritas, helheten inte.
    let p0 = f.plan(-1.0);
    let whole_i = f.layers.iter().position(|l| matches!(l.kind, LayerKind::Target(_))).unwrap();
    assert!(p0[whole_i].is_none());
    let hidden: Vec<bool> = f.pieces().iter().map(|&i| !p0[i].unwrap().drawn()).collect();
    assert_eq!(hidden.iter().filter(|h| **h).count(), 2, "{hidden:?}");
    // Efteråt: som helhet, samma bild som den statiska.
    let still = raster::render(&f.scene, 400, 300).unwrap();
    let done = f.composite(f32::INFINITY, 400, 300);
    let diff = still.data().iter().zip(done.data()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(diff <= 2, "{diff}");
    let before = f.composite(-1.0, 400, 300);
    assert_ne!(before.data(), done.data(), "två tecken saknas före klicket");
}

#[test]
fn utlösarens_mål_är_dolda_tills_klicket() {
    let shapes = text_shape(&["Hej"], false) + &rect(7, 72.0, 400.0, 144.0, 72.0);
    let f = Fixture::new(&shapes, &timing("", &trigger(7, &fade(WHOLE, "", 1000))));
    assert_eq!(f.tl.triggers.len(), 1);
    let i = f.layers.iter().position(|l| l.kind == LayerKind::Target(Target { shape: 4, para: None })).expect("lager för målet");
    let tr = &f.tl.triggers[0];
    let g = f.layers[i].geom(720.0, 540.0);
    assert!(!tr.state(Target { shape: 4, para: None }, None, g, -1.0).drawn());
    assert!(tr.state(Target { shape: 4, para: None }, None, g, f32::INFINITY).drawn());
    assert!(f.layers.iter().all(|l| l.kind != LayerKind::Target(Target { shape: 7, para: None })), "utlösarformen är inget mål");
}

#[test]
fn träffprov_och_oträffbara_utlösare() {
    use egui::{pos2, vec2, Rect};
    use std::sync::Arc;
    // Formen 7 (72..216 × 400..472 pt) är utlösare; 99 finns inte; 20 ligger i en grupp.
    let group = r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="30" name="Grupp"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/><a:chOff x="0" y="0"/><a:chExt cx="914400" cy="914400"/></a:xfrm></p:grpSpPr>"#.to_owned() + &rect(20, 0.0, 0.0, 50.0, 50.0) + "</p:grpSp>";
    let shapes = text_shape(&["Hej"], false) + &rect(7, 72.0, 400.0, 144.0, 72.0) + &group;
    let t = |trig: &str| timing("", trig);
    let build = |timing: &str| {
        let d = Arc::new(Deck::open(&deck(&shapes, timing)).unwrap());
        super::presenter::ShowView::new(d.clone(), super::slideshow::Show::from_beginning(&[false]).unwrap(), true)
    };
    let mut v = build(&t(&trigger(7, &fade(WHOLE, "", 1000))));
    let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(720.0, 540.0));
    assert_eq!(v.trigger_at(full, pos2(100.0, 420.0)), Some(7));
    assert_eq!(v.trigger_at(full, pos2(100.0, 300.0)), None);
    assert_eq!(v.trigger_at(full, pos2(300.0, 420.0)), None);
    // Halv storlek (360 pt bred) och förskjuten: samma träff på motsvarande plats.
    let half = Rect::from_min_size(pos2(100.0, 50.0), vec2(360.0, 270.0));
    assert_eq!(v.trigger_at(half, pos2(100.0 + 50.0, 50.0 + 210.0)), Some(7));
    assert_eq!(v.trigger_at(half, pos2(90.0, 50.0 + 210.0)), None, "utanför bilden");
    // Klick på utlösaren spelar den i stället för att gå vidare.
    assert!(v.click_slide(full, Some(pos2(100.0, 420.0)), 1.0));
    assert!(matches!(v.slide_screen(), super::slideshow::Screen::Slide(0)));
    // Klick bredvid går till nästa steg (här: slutet).
    assert!(v.click_slide(full, Some(pos2(600.0, 100.0)), 2.0));
    assert!(matches!(v.slide_screen(), super::slideshow::Screen::End));
    // Formen saknas, ligger i en grupp: utlösaren tas bort och målet visas som vanligt.
    for id in [99, 20] {
        let v = build(&t(&trigger(id, &fade(WHOLE, "", 1000))));
        let tl = &v.timelines()[0];
        assert!(tl.triggers.is_empty(), "utlösare {id}");
        assert!(tl.targets().is_empty());
    }
}

#[test]
fn mediakommandon_och_mediaformer_ur_en_bild() {
    let pic = r#"<p:pic><p:nvPicPr><p:cNvPr id="8" name="Video"/><p:cNvPicPr/><p:nvPr><a:videoFile r:link="rId9"/></p:nvPr></p:nvPicPr><p:blipFill/><p:spPr><a:xfrm><a:off x="914400" y="3657600"/><a:ext cx="2743200" cy="1828800"/></a:xfrm></p:spPr></p:pic>"#;
    let cmd = |c: &str| {
        format!(r#"<p:par><p:cTn presetID="2" presetClass="mediacall" fill="hold" nodeType="clickEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:cmd type="call" cmd="{c}"><p:cBhvr><p:cTn dur="30503" fill="hold"/><p:tgtEl><p:spTgt spid="8"/></p:tgtEl></p:cBhvr></p:cmd></p:childTnLst></p:cTn></p:par>"#)
    };
    let d = Deck::open(&deck(&(text_shape(&["Hej"], false) + pic), &timing(&click(&cmd("playFrom(0.0)")), &trigger(8, &cmd("togglePause"))))).unwrap();
    let tl = Timeline::of_slide(&d.slides[0].root);
    assert_eq!(tl.media, vec![anim::MediaShape { id: 8, video: true }]);
    assert_eq!(tl.chain_cmds(1), vec![(0.0, MediaCmd { shape_id: 8, kind: MediaKind::Play { from_s: 0.0 } })]);
    assert_eq!(tl.triggers[0].cmds(), vec![(0.0, MediaCmd { shape_id: 8, kind: MediaKind::TogglePause })]);
    // Bilden (videons stillbild) är ingen animerad form: inga lager för den.
    assert!(tl.targets().is_empty());
}
