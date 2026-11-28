//! Prov för redigering av SmartArt-text (`edit_smartart.rs`): text i nod med och utan PowerPoints
//! ritdel, nya, flyttade och borttagna noder, tur och retur och `validate`.

use super::edit::Session;
use super::edit_smartart::{self, Op, Target};
use super::edit_tests::validate;
use super::edit_text::Pos;
use super::smartart_tests::{frame, layout, pptx, DGM, LIN_T};
use super::Deck;

/// Datamodellen: två noder "Ett" (a) och "Två" (b) med övergångar, presentationspunkter Pa/Pb med
/// `presOf`, och (om `drawing`) `dsp:dataModelExt` som pekar på ritdelen.
fn data(drawing: bool) -> String {
    let pt = |id: &str, t: &str| format!(r#"<dgm:pt modelId="{id}"><dgm:prSet phldrT="[Text]"/><dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE" b="1"/><a:t>{t}</a:t></a:r></a:p></dgm:t></dgm:pt>"#);
    let tr = |id: &str, kind: &str, c: &str| format!(r#"<dgm:pt modelId="{id}" type="{kind}" cxnId="{c}"><dgm:prSet/><dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="sv-SE"/></a:p></dgm:t></dgm:pt>"#);
    let pres = |id: &str, of: &str| format!(r#"<dgm:pt modelId="{id}" type="pres"><dgm:prSet presAssocID="{of}" presName="node" presStyleLbl="node1" presStyleIdx="0" presStyleCnt="2"/><dgm:spPr/></dgm:pt>"#);
    let ext = if drawing {
        r#"<dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rIdW" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst>"#
    } else {
        ""
    };
    format!(
        r#"<dgm:dataModel {DGM}><dgm:ptLst><dgm:pt modelId="doc" type="doc"><dgm:prSet csTypeId="urn:prov/colors"/></dgm:pt>{}{}{}{}{}{}{}{}</dgm:ptLst><dgm:cxnLst><dgm:cxn modelId="ca" srcId="doc" destId="a" srcOrd="0" destOrd="0" parTransId="a-p" sibTransId="a-s"/><dgm:cxn modelId="cb" srcId="doc" destId="b" srcOrd="1" destOrd="0" parTransId="b-p" sibTransId="b-s"/><dgm:cxn modelId="xa" type="presOf" srcId="a" destId="Pa" srcOrd="0" destOrd="0"/><dgm:cxn modelId="xb" type="presOf" srcId="b" destId="Pb" srcOrd="0" destOrd="0"/></dgm:cxnLst>{ext}</dgm:dataModel>"#,
        pt("a", "Ett"),
        tr("a-p", "parTrans", "ca"),
        tr("a-s", "sibTrans", "ca"),
        pt("b", "Två"),
        tr("b-p", "parTrans", "cb"),
        tr("b-s", "sibTrans", "cb"),
        pres("Pa", "a"),
        pres("Pb", "b")
    )
}

/// PowerPoints ritdel: Pa överst och Pb under (300 × 80 pt), med text i 20 pt.
fn drawing() -> String {
    let sp = |id: &str, y: i64, t: &str| {
        let e = |v: i64| v * 12_700;
        format!(
            r#"<dsp:sp modelId="{id}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="0" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="ED7D31"/></a:solidFill></dsp:spPr><dsp:style><a:lnRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef><a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dsp:style><dsp:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE" sz="2000" b="1"/><a:t>{t}</a:t></a:r></a:p></dsp:txBody><dsp:txXfrm><a:off x="0" y="{}"/><a:ext cx="{}" cy="{}"/></dsp:txXfrm></dsp:sp>"#,
            e(y),
            e(300),
            e(80),
            e(y),
            e(300),
            e(80)
        )
    };
    format!(
        r#"<dsp:drawing xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>{}{}</dsp:spTree></dsp:drawing>"#,
        sp("Pa", 0, "Ett"),
        sp("Pb", 100, "Två")
    )
}

struct Ed {
    deck: Deck,
    s: Session,
}

impl Ed {
    fn new(bytes: Vec<u8>) -> Ed {
        let deck = Deck::open(&bytes).unwrap();
        let s = Session::new(bytes, &deck);
        Ed { deck, s }
    }

    fn apply<R>(&mut self, f: impl FnOnce(&mut super::edit::Txn) -> Result<R, String>) -> R {
        let mut out = None;
        let a = self
            .s
            .apply(&self.deck, None, |t| {
                out = Some(f(t)?);
                Ok(())
            })
            .unwrap()
            .expect("ändring");
        self.deck = a.deck;
        out.unwrap()
    }

    fn save(&mut self) -> Vec<u8> {
        let (b, d) = self.s.save_bytes(&self.deck, "pptx", "pptx").unwrap();
        if let Some(d) = d {
            self.deck = d;
        }
        let e = validate(&b);
        assert!(e.is_empty(), "ogiltig fil: {e:#?}");
        b
    }

    fn text(&self) -> String {
        super::render::slide_scene(&self.deck, 0).text()
    }
}

fn part(bytes: &[u8], name: &str) -> Option<String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    std::io::Read::read_to_string(&mut f, &mut s).ok()?;
    Some(s)
}

/// Texten i datamodellens punkt `id`.
fn pt_text(data: &str, id: &str) -> String {
    let start = data.find(&format!(r#"<dgm:pt modelId="{id}""#)).unwrap_or_else(|| panic!("{id} saknas"));
    let end = start + data[start..].find("</dgm:pt>").unwrap();
    let mut out = String::new();
    for piece in data[start..end].split("<a:t>").skip(1) {
        out.push_str(piece.split("</a:t>").next().unwrap_or(""));
    }
    out
}

#[test]
fn text_i_nod_skrivs_till_datamodellen_och_ritdelen() {
    let orig = pptx(&frame(100.0, 100.0, 300.0, 200.0), &data(true), &layout(LIN_T), Some(&drawing()));
    let mut ed = Ed::new(orig.clone());
    // Ramen vid (100, 100): Pb ligger på 200–280 pt.
    let t = edit_smartart::target_at(&ed.deck, 0, 250.0, 240.0).expect("noden under punkten");
    assert_eq!((t.pt.as_str(), t.shape_id.as_str(), t.drawing_part.as_deref()), ("b", "Pb", Some("ppt/diagrams/drawing1.xml")));
    let mut m = edit_smartart::model(&ed.deck, &t).unwrap();
    assert_eq!(m.text(), "Två");
    assert!(edit_smartart::carets(&ed.deck, &t, &m).is_some(), "markörlägen");
    let end = m.end();
    m.insert(end, " och tre");
    ed.apply(|txn| edit_smartart::set_text(txn, &t, &m));
    assert!(ed.text().contains("Två och tre"), "{}", ed.text());
    let out = ed.save();
    // Datamodellen och ritdelen har samma text; datamodellens formatering (fet, utan storlek) behålls,
    // ritdelens storlek likaså.
    let d = part(&out, "ppt/diagrams/data1.xml").unwrap();
    let w = part(&out, "ppt/diagrams/drawing1.xml").unwrap();
    assert_eq!(pt_text(&d, "b"), "Två och tre");
    assert!(d.contains(r#"<a:rPr lang="sv-SE" b="1"/><a:t>Två och tre</a:t>"#), "{d}");
    assert!(w.contains(r#"<a:rPr lang="sv-SE" sz="2000" b="1"/><a:t>Två och tre</a:t>"#), "{w}");
    assert_eq!(pt_text(&d, "a"), "Ett");
    // Övriga delar orörda.
    for n in ["ppt/slides/slide1.xml", "ppt/diagrams/layout1.xml", "ppt/diagrams/colors1.xml"] {
        assert_eq!(part(&out, n), part(&orig, n), "{n}");
    }
    let again = Deck::open(&out).unwrap();
    assert!(super::render::slide_scene(&again, 0).text().contains("Två och tre"));
}

#[test]
fn ny_nod_tar_bort_ritdelen_och_laggs_ut_av_motorn() {
    let orig = pptx(&frame(100.0, 100.0, 300.0, 200.0), &data(true), &layout(LIN_T), Some(&drawing()));
    let mut ed = Ed::new(orig);
    let t = edit_smartart::target_at(&ed.deck, 0, 250.0, 140.0).unwrap();
    assert_eq!(t.pt, "a");
    let new = ed.apply(|txn| edit_smartart::restructure(txn, &t, Op::After)).expect("ny nod");
    // Ritdelen, relationen och hänvisningen är borta; motorn lägger ut tre noder.
    assert!(ed.deck.pkg.read("ppt/diagrams/drawing1.xml").is_none());
    assert!(ed.deck.drawings.keys().any(|k| k.ends_with("#kladd")));
    // Den nya noden går att skriva i (motorns ritdel), och texten syns efter omläggningen.
    let nt = Target { drawing_part: None, shape_id: String::new(), pt: new.clone(), ..t.clone() };
    let mut m = edit_smartart::model(&ed.deck, &nt).unwrap();
    assert_eq!(m.text(), "");
    assert!(edit_smartart::carets(&ed.deck, &nt, &m).is_some(), "den nya nodens form");
    m.insert(Pos::default(), "Ny");
    ed.apply(|txn| edit_smartart::set_text(txn, &nt, &m));
    let text = ed.text();
    let (i1, i2, i3) = (text.find("Ett").unwrap(), text.find("Ny").unwrap(), text.find("Två").unwrap());
    assert!(i1 < i2 && i2 < i3, "{text}");
    let out = ed.save();
    assert!(part(&out, "ppt/diagrams/drawing1.xml").is_none());
    let rels = part(&out, "ppt/slides/_rels/slide1.xml.rels").unwrap();
    assert!(!rels.contains("diagramDrawing"), "{rels}");
    let d = part(&out, "ppt/diagrams/data1.xml").unwrap();
    assert!(!d.contains("dataModelExt"), "{d}");
    assert_eq!(pt_text(&d, &new), "Ny");
    assert!(d.contains(&format!(r#"srcId="doc" destId="{new}" srcOrd="1""#)), "{d}");
    assert!(d.contains(r#"destId="b" srcOrd="2""#), "{d}");
    // Öppnas igen med samma utläggning.
    let again = Deck::open(&out).unwrap();
    let t2 = super::render::slide_scene(&again, 0).text();
    assert!(t2.contains("Ny") && t2.contains("Ett") && t2.contains("Två"), "{t2}");
}

#[test]
fn nod_flyttas_ned_och_upp_och_tom_nod_tas_bort() {
    let orig = pptx(&frame(100.0, 100.0, 300.0, 200.0), &data(false), &layout(LIN_T), None);
    let mut ed = Ed::new(orig);
    let tb = edit_smartart::target_at(&ed.deck, 0, 250.0, 290.0).or_else(|| edit_smartart::target_at(&ed.deck, 0, 250.0, 240.0)).expect("noden b");
    assert_eq!((tb.pt.as_str(), tb.drawing_part.as_deref()), ("b", None));
    // Tab: b blir barn till a.
    assert_eq!(ed.apply(|txn| edit_smartart::restructure(txn, &tb, Op::Demote)).as_deref(), Some("b"));
    let d = String::from_utf8(ed.deck.pkg.read("ppt/diagrams/data1.xml").unwrap().to_vec()).unwrap();
    assert!(d.contains(r#"srcId="a" destId="b" srcOrd="0""#), "{d}");
    // Tab på den första noden ändrar inget.
    let ta = Target { pt: "a".into(), ..tb.clone() };
    let r = ed.s.apply(&ed.deck, None, |txn| edit_smartart::restructure(txn, &ta, Op::Demote).map(|r| assert!(r.is_none()))).unwrap();
    assert!(r.is_none(), "inget ändrat");
    // Skift+Tab: b tillbaka efter a.
    ed.apply(|txn| edit_smartart::restructure(txn, &tb, Op::Promote));
    let d = String::from_utf8(ed.deck.pkg.read("ppt/diagrams/data1.xml").unwrap().to_vec()).unwrap();
    assert!(d.contains(r#"srcId="doc" destId="b" srcOrd="1""#), "{d}");
    // En ny tom nod efter a tas bort igen; b behåller sin plats.
    let new = ed.apply(|txn| edit_smartart::restructure(txn, &ta, Op::After)).unwrap();
    let tn = Target { pt: new.clone(), ..tb.clone() };
    assert_eq!(ed.apply(|txn| edit_smartart::restructure(txn, &tn, Op::Remove)).as_deref(), Some("a"));
    let out = ed.save();
    let d = part(&out, "ppt/diagrams/data1.xml").unwrap();
    assert!(!d.contains(&new), "{d}");
    assert!(d.contains(r#"srcId="doc" destId="b" srcOrd="1""#), "{d}");
    // En nod med barn tas inte bort.
    let mut ed = Ed::new(out);
    ed.apply(|txn| edit_smartart::restructure(txn, &tb, Op::Demote));
    assert!(ed.s.apply(&ed.deck, None, |txn| edit_smartart::restructure(txn, &ta, Op::Remove).map(|r| assert!(r.is_none()))).unwrap().is_none());
}

#[test]
fn tangenterna_for_noder() {
    use egui::Key;
    assert_eq!(edit_smartart::key_op(Key::Enter, false, false, true, false), Some(Op::After));
    assert_eq!(edit_smartart::key_op(Key::Enter, false, false, false, false), None, "Enter mitt i texten delar stycket");
    assert_eq!(edit_smartart::key_op(Key::Enter, true, false, true, false), None, "Skift+Enter är radbrytning");
    assert_eq!(edit_smartart::key_op(Key::Tab, false, false, false, false), Some(Op::Demote));
    assert_eq!(edit_smartart::key_op(Key::Tab, true, false, false, false), Some(Op::Promote));
    assert_eq!(edit_smartart::key_op(Key::Backspace, false, false, true, true), Some(Op::Remove));
    assert_eq!(edit_smartart::key_op(Key::Delete, false, false, true, false), None);
}

/// Redigerar SmartArt i alla presentationer under `KLADD_PPTX_SWEEP`: i varje diagram skrivs text i en
/// nod (mitt i varje form provas tills en nod hittas), filen sparas och valideras; sedan läggs en nod
/// till, och filen sparas, valideras och öppnas igen. `cargo test smartart_alla -- --ignored --nocapture`.
#[test]
#[ignore]
fn smartart_alla() {
    let Some(root) = std::env::var_os("KLADD_PPTX_SWEEP") else { return };
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pptx")) {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(&root), &mut files);
    files.sort();
    let (mut edited, mut problems) = (0, Vec::new());
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { continue };
        if !bytes.windows(17).any(|w| w == b"ppt/diagrams/data") {
            continue;
        }
        let Ok(deck) = Deck::open(&bytes) else { continue };
        if !validate(&bytes).is_empty() {
            continue;
        }
        let name = f.display().to_string();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<usize, String> {
            let mut n = 0;
            for slide in 0..deck.slides.len() {
                if super::smartart::diagram_frames(&deck, slide).is_empty() {
                    continue;
                }
                // Mittpunkterna i bildens rutnät (var 20:e pt) som provpunkter.
                let (w, h) = deck.size_pt();
                let mut found = None;
                'outer: for gy in (10..h as i32).step_by(20) {
                    for gx in (10..w as i32).step_by(20) {
                        if let Some(t) = edit_smartart::target_at(&deck, slide, gx as f32, gy as f32) {
                            found = Some(t);
                            break 'outer;
                        }
                    }
                }
                let Some(t) = found else { continue };
                let mut ed = Ed { deck: Deck::open(&bytes).map_err(|e| e.to_string())?, s: Session::new(bytes.clone(), &deck) };
                let mut m = edit_smartart::model(&ed.deck, &t).ok_or("ingen modell")?;
                let end = m.end();
                m.insert(end, " Kladd");
                let a = ed.s.apply(&ed.deck, None, |txn| edit_smartart::set_text(txn, &t, &m))?.ok_or("ingen ändring")?;
                ed.deck = a.deck;
                let (out, _) = ed.s.save_bytes(&ed.deck, "pptx", "pptx")?;
                let e = validate(&out);
                if !e.is_empty() {
                    return Err(format!("bild {}: text: {e:?}", slide + 1));
                }
                let a = ed.s.apply(&ed.deck, None, |txn| edit_smartart::restructure(txn, &t, Op::After).map(|_| ()))?.ok_or("ingen ny nod")?;
                ed.deck = a.deck;
                let (out, _) = ed.s.save_bytes(&ed.deck, "pptx", "pptx")?;
                let e = validate(&out);
                if !e.is_empty() {
                    return Err(format!("bild {}: ny nod: {e:?}", slide + 1));
                }
                Deck::open(&out).map_err(|e| format!("öppnas inte igen: {e}"))?;
                n += 1;
            }
            Ok(n)
        }));
        match r {
            Ok(Ok(n)) => edited += n,
            Ok(Err(e)) => problems.push(format!("{name}: {e}")),
            Err(_) => problems.push(format!("{name}: panik")),
        }
    }
    println!("{edited} diagram redigerade");
    assert!(problems.is_empty(), "{problems:#?}");
}
