//! Prov för SmartArt-layouten (`dgm/`): egna, minimala diagram (datamodell, layoutdefinition,
//! stil- och färgdefinition) med och utan ritdel, och den ignorerade jämförelsen mot PowerPoints
//! sparade ritdelar (`KLADD_DGM_ORACLE`).

use std::io::Write;

use super::xml::Node;
use super::{raster, render, smartart, Deck};

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const DGM: &str = r#"xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Prov"><a:themeElements>
<a:clrScheme name="Prov"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="Prov"><a:majorFont><a:latin typeface="Liberation Sans"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Liberation Sans"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>
<a:fmtScheme name="Prov"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="25400"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="38100"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme>
</a:themeElements></a:theme>"#;

/// Färgdefinitionen för proven: noder i accent 2 med vit linje och vit text, mellanrum i accent 1.
const COLORS: &str = r#"<dgm:colorsDef xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" uniqueId="urn:prov/colors"><dgm:styleLbl name="node1"><dgm:fillClrLst meth="repeat"><a:schemeClr val="accent2"/></dgm:fillClrLst><dgm:linClrLst meth="repeat"><a:schemeClr val="lt1"/></dgm:linClrLst><dgm:effectClrLst/><dgm:txLinClrLst/><dgm:txFillClrLst meth="repeat"><a:schemeClr val="lt1"/></dgm:txFillClrLst><dgm:txEffectClrLst/></dgm:styleLbl></dgm:colorsDef>"#;
const QUICK: &str = r#"<dgm:styleDef xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" uniqueId="urn:prov/qs"><dgm:styleLbl name="node1"><dgm:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></dgm:scene3d><dgm:sp3d/><dgm:txPr/><dgm:style><a:lnRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef><a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dgm:style></dgm:styleLbl></dgm:styleDef>"#;

/// En .pptx (720 × 540 pt) med `slide_inner` på bilden och SmartArt-delarna `parts`
/// (`ppt/diagrams/…`); bildens relationer rIdD/rIdL/rIdQ/rIdC (och rIdW om `drawing`).
fn pptx(slide_inner: &str, data: &str, layout: &str, drawing: Option<&str>) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &str| {
        z.start_file(name, opt).unwrap();
        z.write_all(data.as_bytes()).unwrap();
    };
    put("[Content_Types].xml", r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/></Types>"#);
    put("_rels/.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#));
    put("ppt/_rels/presentation.xml.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rIdS1" Type="{REL}/slide" Target="slides/slide1.xml"/></Relationships>"#));
    put("ppt/presentation.xml", &format!(r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rIdS1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#));
    let tree = |inner: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#);
    put("ppt/slideMasters/slideMaster1.xml", &format!(r#"<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:sldMaster>"#));
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="../theme/theme1.xml"/></Relationships>"#));
    put("ppt/slideLayouts/slideLayout1.xml", &format!(r#"<p:sldLayout {NS}>{}</p:sldLayout>"#, tree("")));
    put("ppt/slideLayouts/_rels/slideLayout1.xml.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#));
    put("ppt/theme/theme1.xml", THEME);
    put("ppt/slides/slide1.xml", &format!(r#"<p:sld {NS}>{}</p:sld>"#, tree(slide_inner)));
    let w = if drawing.is_some() { r#"<Relationship Id="rIdW" Type="http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" Target="../diagrams/drawing1.xml"/>"# } else { "" };
    put(
        "ppt/slides/_rels/slide1.xml.rels",
        &format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rIdD" Type="{REL}/diagramData" Target="../diagrams/data1.xml"/><Relationship Id="rIdL" Type="{REL}/diagramLayout" Target="../diagrams/layout1.xml"/><Relationship Id="rIdQ" Type="{REL}/diagramQuickStyle" Target="../diagrams/quickStyle1.xml"/><Relationship Id="rIdC" Type="{REL}/diagramColors" Target="../diagrams/colors1.xml"/>{w}</Relationships>"#
        ),
    );
    put("ppt/diagrams/data1.xml", data);
    put("ppt/diagrams/layout1.xml", layout);
    put("ppt/diagrams/quickStyle1.xml", QUICK);
    put("ppt/diagrams/colors1.xml", COLORS);
    if let Some(d) = drawing {
        put("ppt/diagrams/drawing1.xml", d);
    }
    z.finish().unwrap().into_inner()
}

/// SmartArt-ramen: (x, y, b, h) i punkter.
fn frame(x: f64, y: f64, w: f64, h: f64) -> String {
    let e = |v: f64| (v * 12_700.0) as i64;
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="SmartArt"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rIdD" r:lo="rIdL" r:qs="rIdQ" r:cs="rIdC"/></a:graphicData></a:graphic></p:graphicFrame>"#,
        e(x),
        e(y),
        e(w),
        e(h)
    )
}

/// En datamodell: noder under dokumentet (`(id, text, barn)`); `pres` är extra punkter och
/// `ext` dokumentets tillägg (t.ex. `dsp:dataModelExt`).
fn data(nodes: &[(&str, &str, &[(&str, &str)])], pres: &str, ext: &str) -> String {
    let pt = |id: &str, t: &str| format!(r#"<dgm:pt modelId="{id}"><dgm:prSet/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE"/><a:t>{t}</a:t></a:r></a:p></dgm:t></dgm:pt>"#);
    let tr = |id: &str, kind: &str| format!(r#"<dgm:pt modelId="{id}" type="{kind}"><dgm:t><a:bodyPr/><a:p/></dgm:t></dgm:pt>"#);
    let mut pts = String::from(r#"<dgm:pt modelId="doc" type="doc"><dgm:prSet csTypeId="urn:prov/colors"/></dgm:pt>"#);
    let mut cxns = String::new();
    let mut link = |src: &str, dst: &str, ord: usize, pts: &mut String| {
        pts.push_str(&tr(&format!("{dst}-p"), "parTrans"));
        pts.push_str(&tr(&format!("{dst}-s"), "sibTrans"));
        cxns.push_str(&format!(r#"<dgm:cxn modelId="c-{dst}" srcId="{src}" destId="{dst}" srcOrd="{ord}" destOrd="0" parTransId="{dst}-p" sibTransId="{dst}-s"/>"#));
    };
    for (i, (id, t, kids)) in nodes.iter().enumerate() {
        pts.push_str(&pt(id, t));
        link("doc", id, i, &mut pts);
        for (j, (kid, kt)) in kids.iter().enumerate() {
            pts.push_str(&pt(kid, kt));
            link(id, kid, j, &mut pts);
        }
    }
    format!(r#"<dgm:dataModel {DGM}><dgm:ptLst>{pts}{pres}</dgm:ptLst><dgm:cxnLst>{cxns}</dgm:cxnLst>{ext}</dgm:dataModel>"#)
}

fn layout(inner: &str) -> String {
    format!(r#"<dgm:layoutDef {DGM} uniqueId="urn:prov/layout">{inner}</dgm:layoutDef>"#)
}

/// Layoutmotorns former för bildens första SmartArt: (presentationspunktens id, ruta, geometri, text, storlek).
#[allow(clippy::type_complexity)]
fn laid_out(deck: &Deck) -> Vec<(String, (f64, f64, f64, f64), String, String, Option<i64>)> {
    let (gf, sx, sy) = smartart::diagram_frames(deck, 0)[0];
    smartart::lay_out_frame(deck, 0, gf, sx, sy).map(|(_, r)| shapes(&r)).unwrap_or_default()
}

fn pixel(deck: &Deck, x: u32, y: u32) -> [u8; 3] {
    let (pw, _, rgba) = raster::to_rgba(raster::render(&render::slide_scene(deck, 0), 720, 540).unwrap());
    let o = ((y * pw + x) * 4) as usize;
    [rgba[o], rgba[o + 1], rgba[o + 2]]
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.6
}

/// Formens synliga ruta (x, y, b, h) i punkter ur `a:xfrm` (vriden 90°/270°: bredd och höjd byter plats).
fn visual_box(sp: &Node) -> Option<(f64, f64, f64, f64)> {
    let x = sp.path(&["p:spPr", "a:xfrm"])?;
    let off = x.child("a:off")?;
    let ext = x.child("a:ext")?;
    let (ox, oy) = (off.attr_f64("x")? / 12_700.0, off.attr_f64("y")? / 12_700.0);
    let (w, h) = (ext.attr_f64("cx")? / 12_700.0, ext.attr_f64("cy")? / 12_700.0);
    let rot = (x.attr_f64("rot").unwrap_or(0.0) / 60_000.0).rem_euclid(180.0);
    let (cx, cy) = (ox + w / 2.0, oy + h / 2.0);
    let (w, h) = if (rot - 90.0).abs() < 1.0 { (h, w) } else { (w, h) };
    Some((cx - w / 2.0, cy - h / 2.0, w, h))
}

/// Formerna med `modelId` i en ritdel: id → (ruta, geometri, text, teckenstorlek).
#[allow(clippy::type_complexity)]
fn shapes(root: &Node) -> Vec<(String, (f64, f64, f64, f64), String, String, Option<i64>)> {
    let mut out = Vec::new();
    root.walk(&mut |n| {
        if n.name == "p:sp" {
            let id = n.attr("modelId").unwrap_or("").to_owned();
            let prst = n.path(&["p:spPr", "a:prstGeom"]).and_then(|g| g.attr("prst")).unwrap_or("").to_owned();
            let mut text = String::new();
            let mut sz = None;
            if let Some(b) = n.child("p:txBody") {
                b.walk(&mut |t| {
                    if t.name == "a:t" {
                        text.push_str(&t.text);
                    }
                    if t.name == "a:rPr" && sz.is_none() {
                        sz = t.attr_i64("sz");
                    }
                });
            }
            if let Some(b) = visual_box(n) {
                out.push((id, b, prst, text, sz));
            }
        }
    });
    out
}

fn iou(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    let i = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let u = a.2 * a.3 + b.2 * b.3 - i;
    if u > 0.0 { i / u } else { 0.0 }
}

/// Jämför layoutmotorn med PowerPoints sparade ritdelar i presentationerna i `KLADD_DGM_ORACLE`
/// (sökvägar åtskilda med `,` eller en fil med en per rad): `cargo test smartart_tests::oracle --
/// --ignored --nocapture`. `KLADD_DGM_VERBOSE=1` skriver varje form.
#[test]
#[ignore]
fn oracle() {
    let Ok(list) = std::env::var("KLADD_DGM_ORACLE") else { return };
    let verbose = std::env::var_os("KLADD_DGM_VERBOSE").is_some();
    let paths: Vec<String> = match std::fs::read_to_string(&list) {
        Ok(s) if !list.contains(',') && !list.ends_with(".pptx") => s.lines().map(str::to_owned).filter(|l| !l.trim().is_empty()).collect(),
        _ => list.split(',').map(str::to_owned).collect(),
    };
    let mut all = Vec::new();
    for p in paths {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let Ok(deck) = Deck::open(&bytes) else { continue };
        for i in 0..deck.slides.len() {
            for (gf, sx, sy) in smartart::diagram_frames(&deck, i) {
                if !smartart::has_drawing(&deck, i, gf) {
                    continue;
                }
                let gd = gf.path(&["a:graphic", "a:graphicData"]).unwrap();
                let slide = &deck.slides[i];
                let rels = &slide.rels;
                let ids = gd.child("dgm:relIds").unwrap();
                let rel = |a: &str| ids.attr(a).and_then(|id| super::package::rel(rels, id)).map(|r| r.target.clone()).unwrap_or_default();
                let layout = deck.pkg.xml(&rel("r:lo")).and_then(|l| l.attr("uniqueId").map(|u| u.rsplit('/').next().unwrap_or("").to_owned())).unwrap_or_default();
                let data = deck.pkg.xml(&rel("r:dm"));
                let mut dr_part = None;
                if let Some(d) = &data {
                    d.walk(&mut |n| {
                        if n.local() == "dataModelExt" {
                            dr_part = n.attr("relId").and_then(|r| super::package::rel(rels, r)).map(|r| r.target.clone());
                        }
                    });
                }
                let Some(dr) = dr_part.and_then(|p| deck.drawing(&p)) else { continue };
                let want = shapes(&dr.root);
                let got = smartart::lay_out_frame(&deck, i, gf, sx, sy).map(|(_, r)| shapes(&r)).unwrap_or_default();
                let mut sum = 0.0;
                for w in &want {
                    let g = got.iter().find(|g| g.0 == w.0);
                    let v = g.map(|g| iou(w.1, g.1)).unwrap_or(0.0);
                    sum += v;
                    if verbose {
                        let f = |b: (f64, f64, f64, f64)| format!("({:.0},{:.0} {:.0}×{:.0})", b.0, b.1, b.2, b.3);
                        println!(
                            "    {:.2} {:<14} ppt {} {:?} sz {:?} | kladd {} {:?} sz {:?} {:?}",
                            v,
                            w.2,
                            f(w.1),
                            w.3.chars().take(16).collect::<String>(),
                            w.4,
                            g.map(|g| f(g.1)).unwrap_or_default(),
                            g.map(|g| g.2.clone()).unwrap_or_default(),
                            g.and_then(|g| g.4),
                            g.map(|g| g.3.chars().take(16).collect::<String>()).unwrap_or_default()
                        );
                    }
                }
                let extra = got.iter().filter(|g| !want.iter().any(|w| w.0 == g.0)).count();
                let score = if want.is_empty() { 1.0 } else { sum / want.len() as f64 };
                println!("{score:.3} {layout:<22} {} bild {} ({} former, {} extra)", p.rsplit('/').nth(1).unwrap_or(&p), i + 1, want.len(), extra);
                all.push(score);
            }
        }
    }
    if !all.is_empty() {
        println!("medel {:.3} över {} diagram", all.iter().sum::<f64>() / all.len() as f64, all.len());
    }
}

/// Ritar bilderna med SmartArt utan användbar ritdel (layoutmotorn) i presentationerna i
/// `KLADD_DGM_RENDER` (som `KLADD_DGM_ORACLE`) till PNG i `KLADD_DGM_OUT`.
#[test]
#[ignore]
fn rita_layoutmotorns_bilder() {
    let (Ok(list), Ok(out)) = (std::env::var("KLADD_DGM_RENDER"), std::env::var("KLADD_DGM_OUT")) else { return };
    let paths: Vec<String> = match std::fs::read_to_string(&list) {
        Ok(s) if !list.ends_with(".pptx") && !list.ends_with(".potx") => s.lines().map(str::to_owned).filter(|l| !l.trim().is_empty()).collect(),
        _ => list.split(',').map(str::to_owned).collect(),
    };
    let _ = std::fs::create_dir_all(&out);
    for (n, p) in paths.iter().enumerate() {
        let Ok(bytes) = std::fs::read(p) else { continue };
        let t0 = std::time::Instant::now();
        let Ok(deck) = Deck::open(&bytes) else { continue };
        let open_ms = t0.elapsed().as_millis();
        for i in 0..deck.slides.len() {
            if !smartart::diagram_frames(&deck, i).iter().any(|(gf, _, _)| !smartart::has_drawing(&deck, i, gf)) {
                continue;
            }
            let Some((w, h, px)) = super::render_slide(&deck, i, 960, (960.0 * deck.size_emu.1 as f64 / deck.size_emu.0 as f64) as u32) else { continue };
            let name = format!("{out}/{n:02}-{}-{:02}.png", p.rsplit('/').next().unwrap_or("x").replace(' ', "_"), i + 1);
            if let Some(img) = image::RgbaImage::from_raw(w, h, px) {
                let _ = img.save(&name);
            }
            println!("{name} (öppning {open_ms} ms)");
        }
    }
}

/// En lista uppifrån (`lin`, `fromT`) med mellanrum (`sp`) efter varje nod utom den sista.
const LIN_T: &str = r#"<dgm:layoutNode name="root"><dgm:alg type="lin"><dgm:param type="linDir" val="fromT"/></dgm:alg><dgm:shape/><dgm:presOf/>
<dgm:constrLst><dgm:constr type="w" for="ch" forName="node" refType="w"/><dgm:constr type="h" for="ch" forName="node" refType="h"/><dgm:constr type="h" for="ch" forName="space" refType="h" fact="0.1"/><dgm:constr type="primFontSz" for="ch" forName="node" op="equ" val="65"/></dgm:constrLst>
<dgm:ruleLst><dgm:rule type="primFontSz" for="ch" forName="node" val="5"/></dgm:ruleLst>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="node" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="roundRect"/><dgm:presOf axis="self"/></dgm:layoutNode>
<dgm:forEach name="s" axis="followSib" ptType="sibTrans" cnt="1"><dgm:layoutNode name="space"><dgm:alg type="sp"/><dgm:shape/><dgm:presOf/></dgm:layoutNode></dgm:forEach></dgm:forEach></dgm:layoutNode>"#;

#[test]
fn lin_staplar_noderna_och_krymper_dem_i_flodesriktningen() {
    let long = "En lång text som inte får plats på en rad i 65 punkter och därför måste krympas en hel del";
    let d = data(&[("a", "Ett", &[]), ("b", long, &[]), ("c", "Tre", &[])], "", "");
    let deck = Deck::open(&pptx(&frame(100.0, 100.0, 400.0, 300.0), &d, &layout(LIN_T), None)).unwrap();
    let s = laid_out(&deck);
    assert_eq!(s.len(), 3, "{s:?}");
    // 3 h + 2 × 0,1 h = 300 → h = 93,75; bredden är ramens (krymps inte tvärs över).
    let ys: Vec<f64> = s.iter().map(|x| x.1 .1).collect();
    assert!(close(ys[0], 0.0) && close(ys[1], 103.125) && close(ys[2], 206.25), "{ys:?}");
    assert!(s.iter().all(|x| close(x.1 .2, 400.0) && close(x.1 .3, 93.75) && x.2 == "roundRect"));
    // Teckenstorleken krymps tills den långa texten får plats, och gruppen (`op="equ"`) får samma.
    let sizes: Vec<i64> = s.iter().filter_map(|x| x.4).collect();
    assert_eq!(sizes.len(), 3);
    assert!(sizes.iter().all(|z| *z == sizes[0]) && sizes[0] < 6500 && sizes[0] >= 500, "{sizes:?}");
    // Formen ritas i färgdefinitionens accent 2 på bilden (ramen vid 100, 100).
    assert!(pixel(&deck, 125, 147).iter().zip([0xED, 0x7D, 0x31]).all(|(a, b)| (*a as i32 - b).abs() < 8), "{:?}", pixel(&deck, 125, 147));
}

#[test]
fn egen_skalning_och_forskjutning_i_lin() {
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="lin"/><dgm:shape/><dgm:presOf/><dgm:constrLst><dgm:constr type="w" for="ch" forName="node" refType="w"/><dgm:constr type="h" for="ch" forName="node" refType="h"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="node" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="rect"/><dgm:presOf axis="self"/></dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let pres = r#"<dgm:pt modelId="Pa" type="pres"><dgm:prSet presAssocID="a" presName="node" custScaleX="50000"/></dgm:pt><dgm:pt modelId="Pb" type="pres"><dgm:prSet presAssocID="b" presName="node" custLinFactNeighborX="-10000"/></dgm:pt>"#;
    let d = data(&[("a", "A", &[]), ("b", "B", &[])], pres, "");
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 600.0, 200.0), &d, &l, None)).unwrap();
    let s = laid_out(&deck);
    let a = s.iter().find(|x| x.0 == "Pa").unwrap().1;
    let b = s.iter().find(|x| x.0 == "Pb").unwrap().1;
    // Utbredningar 300 + 600 vid skala 1 → skala 2/3: A 200 (halva sin ruta), B 400 direkt efter A,
    // sedan B 10 % av grannens (A:s) ruta (400) åt vänster.
    assert!(close(a.0, 0.0) && close(a.2, 200.0), "{a:?}");
    assert!(close(b.0, 160.0) && close(b.2, 400.0), "{b:?}");
}

#[test]
fn composite_med_bildforhallande() {
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="composite"><dgm:param type="ar" val="1"/></dgm:alg><dgm:shape/><dgm:presOf/>
<dgm:constrLst><dgm:constr type="l" for="ch" forName="c" refType="w" fact="0.25"/><dgm:constr type="t" for="ch" forName="c" refType="w" fact="0.25"/><dgm:constr type="w" for="ch" forName="c" refType="w" fact="0.5"/><dgm:constr type="h" for="ch" forName="c" refType="w" refFor="ch" refForName="c"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node" cnt="1"><dgm:layoutNode name="c" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="ellipse"/><dgm:presOf axis="self"/></dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let d = data(&[("a", "A", &[]), ("b", "B", &[])], "", "");
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 400.0, 200.0), &d, &l, None)).unwrap();
    let s = laid_out(&deck);
    // Kvadraten 200 × 200 centrerad i ramen; bara den första noden (`cnt="1"`).
    assert_eq!(s.len(), 1);
    let (x, y, w, h) = s[0].1;
    assert!(close(x, 150.0) && close(y, 50.0) && close(w, 100.0) && close(h, 100.0), "{:?}", s[0].1);
    assert_eq!(s[0].2, "ellipse");
}

#[test]
fn snake_valjer_antal_per_rad_och_centrerar_sista_raden() {
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="snake"><dgm:param type="grDir" val="tL"/><dgm:param type="flowDir" val="row"/></dgm:alg><dgm:shape/><dgm:presOf/>
<dgm:constrLst><dgm:constr type="w" for="ch" forName="node" refType="w"/><dgm:constr type="h" for="ch" forName="node" refType="w" refFor="ch" refForName="node" fact="0.6"/><dgm:constr type="w" for="ch" forName="sib" refType="w" refFor="ch" refForName="node" fact="0.1"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="node" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="rect"/><dgm:presOf axis="self"/></dgm:layoutNode>
<dgm:forEach name="s" axis="followSib" ptType="sibTrans" cnt="1"><dgm:layoutNode name="sib"><dgm:alg type="sp"/><dgm:shape/><dgm:presOf/></dgm:layoutNode></dgm:forEach></dgm:forEach></dgm:layoutNode>"#,
    );
    let d = data(&[("a", "1", &[]), ("b", "2", &[]), ("c", "3", &[]), ("d", "4", &[]), ("e", "5", &[])], "", "");
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 600.0, 300.0), &d, &l, None)).unwrap();
    let s = laid_out(&deck);
    let pos: Vec<(f64, f64)> = s.iter().map(|x| (x.1 .0, x.1 .1)).collect();
    // Tre per rad ger störst noder (skala 0,3125: 187,5 × 112,5, mellanrum 18,75).
    let want = [(0.0, 37.5), (206.25, 37.5), (412.5, 37.5), (103.125, 150.0), (309.375, 150.0)];
    assert_eq!(pos.len(), 5);
    for (p, w) in pos.iter().zip(want) {
        assert!(close(p.0, w.0) && close(p.1, w.1), "{pos:?}");
    }
}

#[test]
fn cykel_lagger_noderna_pa_en_cirkel() {
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="cycle"><dgm:param type="stAng" val="0"/><dgm:param type="spanAng" val="360"/></dgm:alg><dgm:shape/><dgm:presOf/>
<dgm:constrLst><dgm:constr type="w" for="ch" forName="node" refType="w" fact="0.2"/><dgm:constr type="h" for="ch" forName="node" refType="w" refFor="ch" refForName="node"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="node" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="ellipse"/><dgm:presOf axis="self"/></dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let d = data(&[("a", "1", &[]), ("b", "2", &[]), ("c", "3", &[]), ("d", "4", &[])], "", "");
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 400.0, 400.0), &d, &l, None)).unwrap();
    let s = laid_out(&deck);
    assert_eq!(s.len(), 4);
    let centers: Vec<(f64, f64)> = s.iter().map(|x| (x.1 .0 + x.1 .2 / 2.0, x.1 .1 + x.1 .3 / 2.0)).collect();
    let r: Vec<f64> = centers.iter().map(|c| ((c.0 - 200.0).powi(2) + (c.1 - 200.0).powi(2)).sqrt()).collect();
    assert!(r.iter().all(|v| close(*v, r[0])) && r[0] > 40.0, "{centers:?}");
    // Första noden överst, sedan medurs.
    assert!(close(centers[0].0, 200.0) && centers[0].1 < 200.0 && centers[1].0 > 200.0, "{centers:?}");
}

#[test]
fn pyramidens_nivaer_vaxer_nedat() {
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="pyra"><dgm:param type="pyraLvlNode" val="level"/></dgm:alg><dgm:shape/><dgm:presOf/>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="level" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="trapezoid"/><dgm:presOf axis="self"/></dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let d = data(&[("a", "Topp", &[]), ("b", "Mitt", &[]), ("c", "Bas", &[])], "", "");
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 300.0, 300.0), &d, &l, None)).unwrap();
    let s = laid_out(&deck);
    let boxes: Vec<_> = s.iter().map(|x| x.1).collect();
    assert_eq!(boxes.len(), 3);
    for (i, b) in boxes.iter().enumerate() {
        let w = 100.0 * (i + 1) as f64;
        assert!(close(b.2, w) && close(b.0, (300.0 - w) / 2.0) && close(b.1, 100.0 * i as f64) && close(b.3, 100.0), "{boxes:?}");
    }
    assert!(s.iter().all(|x| x.2 == "trapezoid"));
}

#[test]
fn hierarki_med_roten_over_barnen_och_linjer() {
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="hierChild"/><dgm:shape/><dgm:presOf/>
<dgm:constrLst><dgm:constr type="w" for="des" forName="box" val="50"/><dgm:constr type="h" for="des" forName="box" val="25"/><dgm:constr type="sibSp" for="des" forName="kids" val="10"/><dgm:constr type="sp" for="des" forName="hr" val="10"/><dgm:constr type="bendDist" for="des" ptType="parTrans" val="5"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="hr"><dgm:alg type="hierRoot"/><dgm:shape/><dgm:presOf/>
<dgm:layoutNode name="box" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="rect"/><dgm:presOf axis="self"/></dgm:layoutNode>
<dgm:layoutNode name="kids"><dgm:alg type="hierChild"/><dgm:shape/><dgm:presOf/>
<dgm:forEach name="g" axis="ch" ptType="nonAsst">
<dgm:forEach name="p" axis="precedSib" ptType="parTrans" st="-1" cnt="1"><dgm:layoutNode name="line" styleLbl="node1"><dgm:alg type="conn"><dgm:param type="connRout" val="bend"/><dgm:param type="dim" val="1D"/><dgm:param type="begPts" val="bCtr"/><dgm:param type="endPts" val="tCtr"/><dgm:param type="bendPt" val="end"/></dgm:alg><dgm:shape type="conn"/><dgm:presOf axis="self"/></dgm:layoutNode></dgm:forEach>
<dgm:layoutNode name="hr2"><dgm:alg type="hierRoot"/><dgm:shape/><dgm:presOf/><dgm:layoutNode name="box" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="rect"/><dgm:presOf axis="self"/></dgm:layoutNode></dgm:layoutNode></dgm:forEach>
</dgm:layoutNode></dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let d = data(&[("a", "Chef", &[("b", "Ett"), ("c", "Två")])], "", "");
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 600.0, 400.0), &d, &l, None)).unwrap();
    assert!(deck.drawings.keys().any(|k| k.ends_with("#kladd")));
    let s = laid_out(&deck);
    let by = |t: &str| s.iter().find(|x| x.3 == t).unwrap().1;
    let (a, b, c) = (by("Chef"), by("Ett"), by("Två"));
    let mm = 72.0 / 25.4;
    assert!(close(a.2, 50.0 * mm) && close(a.3, 25.0 * mm), "{a:?}");
    // Barnen sida vid sida med 10 mm emellan, 10 mm under roten; roten centrerad över dem.
    assert!(close(b.1, c.1) && close(b.1, a.1 + a.3 + 10.0 * mm), "{a:?} {b:?} {c:?}");
    assert!(close(c.0 - (b.0 + b.2), 10.0 * mm));
    assert!(close(a.0 + a.2 / 2.0, (b.0 + c.0 + c.2) / 2.0), "{a:?} {b:?} {c:?}");
    // En bruten linje per barn: från rotens underkant mitt till barnets överkant mitt.
    let lines: Vec<_> = s.iter().filter(|x| x.2.is_empty()).map(|x| x.1).collect();
    assert_eq!(lines.len(), 2, "{s:?}");
    for (l, k) in lines.iter().zip([b, c]) {
        assert!(close(l.1, a.1 + a.3) && close(l.1 + l.3, k.1), "{l:?} {k:?}");
        assert!(close(l.0.min(l.0 + l.2), (a.0 + a.2 / 2.0).min(k.0 + k.2 / 2.0)), "{l:?}");
    }
}

#[test]
fn bojd_koppling_blir_en_cirkelbagspil() {
    // Som kugghjulen: pilen runt `g` från källnodens till målnodens vinkel, diametern 1,2 × g.
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="composite"/><dgm:shape/><dgm:presOf/>
<dgm:constrLst><dgm:constr type="l" for="ch" forName="g" val="50"/><dgm:constr type="t" for="ch" forName="g" val="20"/><dgm:constr type="w" for="ch" forName="g" val="40"/><dgm:constr type="h" for="ch" forName="g" val="40"/>
<dgm:constr type="ctrX" for="ch" forName="gsrcNode" val="70"/><dgm:constr type="t" for="ch" forName="gsrcNode" val="20"/><dgm:constr type="w" for="ch" forName="gsrcNode" val="1"/><dgm:constr type="h" for="ch" forName="gsrcNode" val="1"/>
<dgm:constr type="r" for="ch" forName="gdstNode" val="90"/><dgm:constr type="ctrY" for="ch" forName="gdstNode" val="40"/><dgm:constr type="w" for="ch" forName="gdstNode" val="1"/><dgm:constr type="h" for="ch" forName="gdstNode" val="1"/>
<dgm:constr type="diam" for="des" forName="arc" refType="w" refFor="ch" refForName="g" fact="1.2"/><dgm:constr type="h" for="des" forName="arc" refType="w" refFor="ch" refForName="g" fact="0.1"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node" cnt="1"><dgm:layoutNode name="g" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="ellipse"/><dgm:presOf axis="self"/></dgm:layoutNode>
<dgm:layoutNode name="gsrcNode"><dgm:alg type="sp"/><dgm:shape type="rect" hideGeom="1"/></dgm:layoutNode><dgm:layoutNode name="gdstNode"><dgm:alg type="sp"/><dgm:shape type="rect" hideGeom="1"/></dgm:layoutNode></dgm:forEach>
<dgm:forEach name="s" axis="ch" ptType="sibTrans" hideLastTrans="0" cnt="1"><dgm:layoutNode name="arc" styleLbl="node1"><dgm:alg type="conn"><dgm:param type="connRout" val="curve"/><dgm:param type="srcNode" val="gsrcNode"/><dgm:param type="dstNode" val="gdstNode"/></dgm:alg><dgm:shape type="conn"/></dgm:layoutNode></dgm:forEach>
</dgm:layoutNode>"#,
    );
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 400.0, 300.0), &data(&[("a", "A", &[])], "", ""), &l, None)).unwrap();
    let (gf, sx, sy) = smartart::diagram_frames(&deck, 0)[0];
    let (_, root) = smartart::lay_out_frame(&deck, 0, gf, sx, sy).unwrap();
    let mut arc = None;
    root.walk(&mut |n| {
        if n.name == "p:sp" && n.path(&["p:spPr", "a:prstGeom"]).and_then(|g| g.attr("prst")) == Some("circularArrow") {
            arc = Some(n.clone());
        }
    });
    let arc = arc.expect("cirkelbågspil");
    let mm = 72.0 / 25.4;
    let b = visual_box(&arc).unwrap();
    // Mitten i g:s mitt (70, 40 mm), storleken 48 mm.
    assert!(close(b.0 + b.2 / 2.0, 70.0 * mm) && close(b.1 + b.3 / 2.0, 40.0 * mm) && close(b.2, 48.0 * mm), "{b:?}");
    let adj = |name: &str| arc.path(&["p:spPr", "a:prstGeom", "a:avLst"]).and_then(|l| l.children.iter().find(|g| g.attr("name") == Some(name))).and_then(|g| g.attr("fmla")).map(str::to_owned);
    // Från källan överst (270°) medurs till målet till höger (0°).
    assert_eq!(adj("adj4").as_deref(), Some("val 16200000"));
    assert_eq!(adj("adj3").as_deref(), Some("val 0"));
}

#[test]
fn datamodellens_axlar_och_urval() {
    use super::dgm::data::Model;
    let d = data(&[("a", "A", &[("a1", "A1")]), ("b", "B", &[]), ("c", "C", &[])], "", "");
    let root = super::xml::parse(d.as_bytes()).unwrap();
    let m = Model::new(&root);
    let doc = m.root.unwrap();
    let ids = |v: Vec<usize>| v.into_iter().map(|i| m.pts[i].id.to_owned()).collect::<Vec<_>>();
    let sel = |attrs: &str| {
        let n = super::xml::parse(format!(r#"<dgm:forEach xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" {attrs}/>"#).as_bytes()).unwrap();
        ids(m.select(doc, &n))
    };
    assert_eq!(sel(r#"axis="ch" ptType="node""#), ["a", "b", "c"]);
    // Den sista syskonövergången döljs om inte `hideLastTrans="0"`.
    assert_eq!(sel(r#"axis="ch" ptType="sibTrans""#), ["a-s", "b-s"]);
    assert_eq!(sel(r#"axis="ch" ptType="sibTrans" hideLastTrans="0""#), ["a-s", "b-s", "c-s"]);
    assert_eq!(sel(r#"axis="ch" ptType="node" st="2" cnt="1""#), ["b"]);
    assert_eq!(sel(r#"axis="ch" ptType="node" step="2""#), ["a", "c"]);
    assert_eq!(sel(r#"axis="des" ptType="node""#), ["a", "a1", "b", "c"]);
    assert_eq!(sel(r#"axis="ch ch" ptType="node node""#), ["a1"]);
    let a = m.pts.iter().position(|p| p.id == "a").unwrap();
    let n = super::xml::parse(br#"<dgm:forEach xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" axis="followSib" ptType="sibTrans" cnt="1"/>"#).unwrap();
    assert_eq!(ids(m.select(a, &n)), ["a-s"]);
    assert_eq!(m.depth(a), 1);
}

#[test]
fn villkor_valjer_gren_efter_antal_barn() {
    // Med två noder väljs `if` (cnt ≥ 2): rektanglar; med en nod `else`: ellipser.
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="lin"/><dgm:shape/><dgm:presOf/><dgm:constrLst><dgm:constr type="w" for="ch" forName="node" refType="w"/><dgm:constr type="h" for="ch" forName="node" refType="h"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="node" styleLbl="node1"><dgm:alg type="tx"/><dgm:presOf axis="self"/>
<dgm:choose name="c"><dgm:if name="i" axis="par ch" ptType="doc node" func="cnt" op="gte" val="2"><dgm:shape type="rect"/></dgm:if><dgm:else name="e"><dgm:shape type="ellipse"/></dgm:else></dgm:choose>
</dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let two = Deck::open(&pptx(&frame(0.0, 0.0, 400.0, 100.0), &data(&[("a", "A", &[]), ("b", "B", &[])], "", ""), &l, None)).unwrap();
    assert!(laid_out(&two).iter().all(|x| x.2 == "rect"));
    let one = Deck::open(&pptx(&frame(0.0, 0.0, 400.0, 100.0), &data(&[("a", "A", &[])], "", ""), &l, None)).unwrap();
    assert!(laid_out(&one).iter().all(|x| x.2 == "ellipse"));
}

const EMPTY_DRAWING: &str = r#"<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/></dsp:spTree></dsp:drawing>"#;
const EXT: &str = r#"<dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rIdW"/></a:ext></dgm:extLst>"#;

#[test]
fn tom_ritdel_laggs_ut_av_layoutmotorn() {
    // PowerPoint sparar ibland en tom ritdel (owasp-top10-2017-he): då lägger layoutmotorn ut diagrammet.
    let d = data(&[("a", "Ett", &[]), ("b", "Två", &[])], "", EXT);
    let deck = Deck::open(&pptx(&frame(100.0, 100.0, 400.0, 300.0), &d, &layout(LIN_T), Some(EMPTY_DRAWING))).unwrap();
    assert!(deck.drawings.keys().any(|k| k.ends_with("#kladd")));
    let texts: Vec<String> = render::slide_scene(&deck, 0).items.into_iter().filter_map(|i| if let super::scene::Item::Glyphs(g) = i { Some(g.text) } else { None }).collect();
    assert!(texts.iter().any(|t| t.contains("Ett")) && texts.iter().any(|t| t.contains("Två")), "{texts:?}");
    assert!(pixel(&deck, 125, 150).iter().zip([0xED, 0x7D, 0x31]).all(|(a, b)| (*a as i32 - b).abs() < 8), "{:?}", pixel(&deck, 125, 150));
}

#[test]
fn oläslig_layout_ger_reserven() {
    // 30 noder med lång text i en liten ram: texten skulle krympas under 5 punkter, så diagrammet
    // visas som datamodellens text i en punktlista i stället.
    let nodes: Vec<(String, String)> = (0..30).map(|i| (format!("n{i}"), format!("Nod nummer {i} med en ganska lång text"))).collect();
    let refs: Vec<(&str, &str, &[(&str, &str)])> = nodes.iter().map(|(a, b)| (a.as_str(), b.as_str(), &[][..])).collect();
    let d = data(&refs, "", "");
    let deck = Deck::open(&pptx(&frame(100.0, 100.0, 120.0, 60.0), &d, &layout(LIN_T), None)).unwrap();
    assert!(!deck.drawings.keys().any(|k| k.ends_with("#kladd")));
    let (gf, sx, sy) = smartart::diagram_frames(&deck, 0)[0];
    assert!(smartart::lay_out_frame(&deck, 0, gf, sx, sy).is_none());
    // Samma diagram i en stor ram läggs ut av motorn.
    let few: Vec<_> = refs[..3].to_vec();
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 600.0, 400.0), &data(&few, "", ""), &layout(LIN_T), None)).unwrap();
    assert!(deck.drawings.keys().any(|k| k.ends_with("#kladd")));
}

/// En ritdel med en röd rektangel 216 × 72 pt i ramens övre vänstra hörn.
const RED_DRAWING: &str = r#"<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/><dsp:sp modelId="x"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2743200" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></dsp:spPr></dsp:sp></dsp:spTree></dsp:drawing>"#;

#[test]
fn ritdel_i_skalad_grupp_ritas_i_visad_storlek() {
    // Ramen ligger i en grupp skalad till hälften; ritdelen är sparad i ramens visade storlek, så
    // rektangeln blir 216 × 72 pt på bilden (inte 108 × 36) med ramens visade övre vänstra hörn
    // vid (50, 50).
    let group = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="9" name="Grupp"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="635000" y="635000"/><a:ext cx="4572000" cy="3429000"/><a:chOff x="0" y="0"/><a:chExt cx="9144000" cy="6858000"/></a:xfrm></p:grpSpPr>{}</p:grpSp>"#,
        frame(0.0, 0.0, 600.0, 400.0)
    );
    let d = data(&[("a", "A", &[])], "", EXT);
    let deck = Deck::open(&pptx(&group, &d, &layout(LIN_T), Some(RED_DRAWING))).unwrap();
    assert!(!deck.drawings.keys().any(|k| k.ends_with("#kladd")), "ritdelen används");
    assert_eq!(pixel(&deck, 250, 110), [255, 0, 0]);
    assert_eq!(pixel(&deck, 60, 60), [255, 0, 0]);
    assert_ne!(pixel(&deck, 275, 110), [255, 0, 0]);
    assert_ne!(pixel(&deck, 150, 130), [255, 0, 0]);
}

#[test]
fn langt_ord_krymps_inom_rutan_och_gruppen_far_samma_storlek() {
    // Ett ord som inte kan brytas ("Vattenmelonerna") krymps tills det ryms innanför insättningarna
    // i den smala rutan; de andra noderna i gruppen (`op="equ"`) får samma storlek.
    let d = data(&[("a", "Vattenmelonerna", &[]), ("b", "Ett", &[]), ("c", "Två", &[])], "", "");
    let deck = Deck::open(&pptx(&frame(100.0, 100.0, 100.0, 300.0), &d, &layout(LIN_T), None)).unwrap();
    let s = laid_out(&deck);
    let sizes: Vec<i64> = s.iter().filter_map(|x| x.4).collect();
    assert_eq!(sizes.len(), 3);
    assert!(sizes.iter().all(|z| *z == sizes[0]), "{sizes:?}");
    for it in render::slide_scene(&deck, 0).items {
        if let super::scene::Item::Glyphs(g) = it {
            for gl in &g.glyphs {
                let (x0, _) = g.t.apply(gl.x, 0.0);
                let (x1, _) = g.t.apply(gl.x + gl.advance, 0.0);
                assert!(x0 >= 100.0 && x1 <= 200.0, "{} {x0}–{x1}", g.text);
            }
        }
    }
}

#[test]
fn bildplatshallare_far_en_bildsymbol() {
    // En tom bildplatshållare (`blipPhldr`) ritas med PowerPoints bildsymbol i mitten.
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="lin"/><dgm:shape/><dgm:presOf/><dgm:constrLst><dgm:constr type="w" for="ch" refType="w" fact="0.5"/><dgm:constr type="h" for="ch" refType="h"/></dgm:constrLst>
<dgm:forEach name="f" axis="ch" ptType="node" cnt="1"><dgm:layoutNode name="img" styleLbl="node1"><dgm:alg type="sp"/><dgm:shape type="roundRect" blipPhldr="1"/><dgm:presOf/></dgm:layoutNode>
<dgm:layoutNode name="txt" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="rect"/><dgm:presOf axis="self"/></dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 200.0, 100.0), &data(&[("a", "A", &[])], "", ""), &l, None)).unwrap();
    let s = laid_out(&deck);
    assert_eq!(s.len(), 3, "{s:?}");
    assert_eq!(s[0].2, "roundRect");
    // Symbolen: en kvadrat på 40 % av den kortaste sidan (100 → 40) mitt i formen (0–100 × 0–100).
    let g = s[1].1;
    assert!(s[1].2.is_empty() && close(g.2, 40.0) && close(g.3, 40.0) && close(g.0, 30.0) && close(g.1, 30.0), "{g:?}");
}

#[test]
fn rutor_vaxer_med_texten_och_storleken_krymps_tills_allt_ryms() {
    // Som `vList2`: rutans höjd ur teckenstorleken, regeln `h INF` låter rutan växa med texten och
    // den gemensamma storleken krymps tills hela listan ryms i ramen.
    let l = layout(
        r#"<dgm:layoutNode name="root"><dgm:alg type="lin"><dgm:param type="linDir" val="fromT"/></dgm:alg><dgm:shape/><dgm:presOf/>
<dgm:constrLst><dgm:constr type="w" for="ch" forName="node" refType="w"/><dgm:constr type="h" for="ch" forName="node" refType="primFontSz" refFor="ch" refForName="node" fact="0.5"/><dgm:constr type="primFontSz" for="ch" forName="node" op="equ" val="65"/><dgm:constr type="tMarg" for="ch" forName="node" refType="primFontSz" refFor="ch" refForName="node" fact="0.2"/><dgm:constr type="bMarg" for="ch" forName="node" refType="primFontSz" refFor="ch" refForName="node" fact="0.2"/></dgm:constrLst>
<dgm:ruleLst><dgm:rule type="primFontSz" for="ch" forName="node" val="5"/></dgm:ruleLst>
<dgm:forEach name="f" axis="ch" ptType="node"><dgm:layoutNode name="node" styleLbl="node1"><dgm:alg type="tx"/><dgm:shape type="rect"/><dgm:presOf axis="self"/><dgm:ruleLst><dgm:rule type="h" val="INF"/></dgm:ruleLst></dgm:layoutNode></dgm:forEach></dgm:layoutNode>"#,
    );
    let long = "En lång text som behöver flera rader i rutan och därför får den att växa";
    let d = data(&[("a", "Kort", &[]), ("b", long, &[]), ("c", "Kort", &[])], "", "");
    let deck = Deck::open(&pptx(&frame(0.0, 0.0, 300.0, 300.0), &d, &l, None)).unwrap();
    let s = laid_out(&deck);
    assert_eq!(s.len(), 3, "{s:?}");
    let sizes: Vec<i64> = s.iter().filter_map(|x| x.4).collect();
    assert!(sizes.iter().all(|z| *z == sizes[0]) && sizes[0] < 6500, "{sizes:?}");
    let (a, b, c) = (s[0].1, s[1].1, s[2].1);
    // Den långa texten har en högre ruta; de korta minst villkorets höjd (0,5 × storleken i mm).
    assert!(b.3 > a.3 * 1.5 && close(a.3, c.3), "{a:?} {b:?} {c:?}");
    let f = sizes[0] as f64 / 100.0;
    assert!(a.3 >= 0.5 * f * 72.0 / 25.4 - 0.5, "{a:?} {f}");
    // Rutorna efter varandra, inom ramen.
    assert!(close(b.1, a.1 + a.3) && close(c.1, b.1 + b.3) && a.1 >= -0.5 && c.1 + c.3 <= 300.5, "{a:?} {b:?} {c:?}");
}
