//! En ny, tom presentation med Kladds eget tema, egen bildmall och de vanliga layouterna. Allt här är
//! skrivet för Kladd (färger, typsnitt, mått och formatmatris är våra egna; ingenting är kopierat
//! ur Microsofts teman eller mallar). Filen byggs som en zip i minnet och öppnas sedan som vilken
//! presentation som helst: ingen sökväg förrän man sparar.

use std::io::Write;

/// Färgvarianterna man väljer bland.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Kladd,
    Skog,
    Skymning,
}

impl Variant {
    pub const ALL: [Variant; 3] = [Variant::Kladd, Variant::Skog, Variant::Skymning];

    /// Namnet på färgschemat i temat.
    fn name(self) -> &'static str {
        match self {
            Variant::Kladd => "Kladd",
            Variant::Skog => "Skog",
            Variant::Skymning => "Skymning",
        }
    }

    /// (dk2, lt2, accent1–6, hlink, folHlink) som RRGGBB.
    fn palette(self) -> [&'static str; 10] {
        match self {
            Variant::Kladd => ["1F3A5F", "EEF1F5", "2F6DB5", "E8793A", "4FA37B", "8A63B8", "D9A21B", "C2455A", "1F6FB2", "7A4FA0"],
            Variant::Skog => ["23402E", "EDF2EA", "3B7D4F", "B5892E", "6FA8A0", "A4573C", "7B8F3A", "4B6A88", "2E7D6B", "6B5B8C"],
            Variant::Skymning => ["2B2347", "F0EDF6", "6A4FB3", "D8576B", "3E8FB0", "E0A53A", "5BA66B", "A24E8C", "5A4FB3", "8E3F7A"],
        }
    }

    /// Visningsfärgerna (accent1, accent2, accent3) för valknapparna.
    pub fn swatches(self) -> [[u8; 3]; 3] {
        let p = self.palette();
        let rgb = |h: &str| [u8::from_str_radix(&h[0..2], 16).unwrap_or(0), u8::from_str_radix(&h[2..4], 16).unwrap_or(0), u8::from_str_radix(&h[4..6], 16).unwrap_or(0)];
        [rgb(p[2]), rgb(p[3]), rgb(p[4])]
    }
}

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Bildens mått i EMU (16:9).
pub const SLIDE: (i64, i64) = (12_192_000, 6_858_000);

fn theme(v: Variant, name: &str) -> String {
    let p = v.palette();
    format!(
        r#"{XML}<a:theme xmlns:a="{NS_A}" name="{name}"><a:themeElements><a:clrScheme name="{cs}"><a:dk1><a:srgbClr val="1A1A1A"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="{}"/></a:dk2><a:lt2><a:srgbClr val="{}"/></a:lt2><a:accent1><a:srgbClr val="{}"/></a:accent1><a:accent2><a:srgbClr val="{}"/></a:accent2><a:accent3><a:srgbClr val="{}"/></a:accent3><a:accent4><a:srgbClr val="{}"/></a:accent4><a:accent5><a:srgbClr val="{}"/></a:accent5><a:accent6><a:srgbClr val="{}"/></a:accent6><a:hlink><a:srgbClr val="{}"/></a:hlink><a:folHlink><a:srgbClr val="{}"/></a:folHlink></a:clrScheme><a:fontScheme name="Kladd"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Kladd"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="85000"/></a:schemeClr></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:shade val="90000"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="70000"/></a:schemeClr></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:shade val="80000"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill></a:fillStyleLst><a:lnStyleLst><a:ln w="9525" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="31750" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst><a:outerShdw blurRad="57150" dist="19050" dir="5400000" algn="ctr" rotWithShape="0"><a:srgbClr val="000000"><a:alpha val="40000"/></a:srgbClr></a:outerShdw></a:effectLst></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="90000"/></a:schemeClr></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:shade val="95000"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#,
        p[0],
        p[1],
        p[2],
        p[3],
        p[4],
        p[5],
        p[6],
        p[7],
        p[8],
        p[9],
        cs = v.name(),
    )
}

/// En platshållare (`p:sp`) med eget läge (`None` = ärvs) och tom text.
fn ph(id: u32, name: &str, ph: &str, xfrm: Option<(i64, i64, i64, i64)>, body_pr: &str, list: &str, text: &str) -> String {
    let x = xfrm.map(|(x, y, w, h)| format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)).unwrap_or_default();
    let para = if text.is_empty() { r#"<a:p><a:endParaRPr lang="sv-SE"/></a:p>"#.to_owned() } else { format!(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>{text}</a:t></a:r></a:p>"#) };
    format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph {ph}/></p:nvPr></p:nvSpPr><p:spPr>{x}</p:spPr><p:txBody><a:bodyPr{body_pr}/><a:lstStyle>{list}</a:lstStyle>{para}</p:txBody></p:sp>"#)
}

fn tree(inner: &str) -> String {
    format!(r#"<p:cSld{{NAME}}><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{inner}</p:spTree></p:cSld>"#)
}

// Egna mått (EMU) för 16:9.
const TITLE: (i64, i64, i64, i64) = (685_800, 457_200, 10_820_400, 1_143_000);
const BODY: (i64, i64, i64, i64) = (685_800, 1_828_800, 10_820_400, 4_114_800);
const DATE: (i64, i64, i64, i64) = (685_800, 6_400_800, 2_743_200, 304_800);
const FOOT: (i64, i64, i64, i64) = (4_038_600, 6_400_800, 4_114_800, 304_800);
const NUM: (i64, i64, i64, i64) = (8_763_000, 6_400_800, 2_743_200, 304_800);

/// Datum, sidfot och bildnummer: platshållare på mallen (med eget läge) och, utan läge, på layouterna.
fn footers(first_id: u32, own: bool) -> String {
    let pos = |p| if own { Some(p) } else { None };
    let small = r#"<a:lvl1pPr algn="l"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr>"#;
    let mid = small.replace("algn=\"l\"", "algn=\"ctr\"");
    let right = small.replace("algn=\"l\"", "algn=\"r\"");
    let (l, m, r) = if own { (small, mid.as_str(), right.as_str()) } else { ("", "", "") };
    format!(
        "{}{}{}",
        ph(first_id, "Datum", r#"type="dt" sz="half" idx="10""#, pos(DATE), r#" vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr""#, l, "").replace(r#"<a:p><a:endParaRPr lang="sv-SE"/></a:p>"#, r#"<a:p><a:endParaRPr lang="sv-SE"/></a:p>"#),
        ph(first_id + 1, "Sidfot", r#"type="ftr" sz="quarter" idx="11""#, pos(FOOT), r#" vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr""#, m, ""),
        sldnum(first_id + 2, pos(NUM), r),
    )
}

fn sldnum(id: u32, xfrm: Option<(i64, i64, i64, i64)>, list: &str) -> String {
    let x = xfrm.map(|(x, y, w, h)| format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)).unwrap_or_default();
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Bildnummer"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldNum" sz="quarter" idx="12"/></p:nvPr></p:nvSpPr><p:spPr>{x}</p:spPr><p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr"/><a:lstStyle>{list}</a:lstStyle><a:p><a:fld id="{{B6F15528-21DE-4FAA-801E-634DDDAF4B2B}}" type="slidenum"><a:rPr lang="sv-SE"/><a:t>‹#›</a:t></a:fld><a:endParaRPr lang="sv-SE"/></a:p></p:txBody></p:sp>"#
    )
}

fn master(v: Variant) -> String {
    let _ = v;
    let body_lvls: String = (1..=9)
        .map(|l| {
            let marl = 342_900 + 457_200 * (l - 1);
            let sz = [2800, 2400, 2000, 1800, 1800, 1600, 1600, 1600, 1600][l as usize - 1];
            format!(r#"<a:lvl{l}pPr marL="{marl}" indent="-342900" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1"><a:lnSpc><a:spcPct val="100000"/></a:lnSpc><a:spcBef><a:spcPts val="800"/></a:spcBef><a:buFont typeface="Arial"/><a:buChar char="{}"/><a:defRPr sz="{sz}" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl{l}pPr>"#, if l % 2 == 1 { "•" } else { "–" })
        })
        .collect();
    let other_lvls: String = (1..=9).map(|l| format!(r#"<a:lvl{l}pPr marL="{}" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1"><a:defRPr sz="1800" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl{l}pPr>"#, 457_200 * (l - 1))).collect();
    let shapes = format!(
        "{}{}{}",
        ph(2, "Rubrik", r#"type="title""#, Some(TITLE), r#" vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr""#, "", "Klicka för att ändra rubrik").replace("<a:bodyPr", "<a:bodyPr").replace("anchor=\"ctr\"/>", "anchor=\"ctr\"><a:normAutofit/></a:bodyPr>").replace("<a:bodyPr vert=\"horz\" lIns=\"91440\" tIns=\"45720\" rIns=\"91440\" bIns=\"45720\" rtlCol=\"0\" anchor=\"ctr\"><a:normAutofit/></a:bodyPr>", "<a:bodyPr vert=\"horz\" lIns=\"91440\" tIns=\"45720\" rIns=\"91440\" bIns=\"45720\" rtlCol=\"0\" anchor=\"ctr\"><a:normAutofit/></a:bodyPr>"),
        ph(3, "Innehåll", r#"type="body" idx="1""#, Some(BODY), r#" vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0""#, "", "Klicka för att ändra text").replace("rtlCol=\"0\"/>", "rtlCol=\"0\"><a:normAutofit/></a:bodyPr>"),
        footers(4, true),
    );
    let t = tree(&shapes).replace("{NAME}", "");
    format!(
        r#"{XML}<p:sldMaster xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}">{t}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst>{}</p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1"><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:spcBef><a:spcPct val="0"/></a:spcBef><a:buNone/><a:defRPr sz="4400" kern="1200"><a:solidFill><a:schemeClr val="tx2"/></a:solidFill><a:latin typeface="+mj-lt"/><a:ea typeface="+mj-ea"/><a:cs typeface="+mj-cs"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle>{body_lvls}</p:bodyStyle><p:otherStyle><a:defPPr><a:defRPr lang="sv-SE"/></a:defPPr>{other_lvls}</p:otherStyle></p:txStyles></p:sldMaster>"#,
        (1..=LAYOUTS.len()).map(|i| format!(r#"<p:sldLayoutId id="{}" r:id="rId{}"/>"#, 2_147_483_649 + i as i64 - 1, i)).collect::<String>(),
    )
}

/// (namn, `type`, formernas XML med id från 2).
const LAYOUTS: &[(&str, &str)] = &[
    ("Rubrikbild", "title"),
    ("Rubrik och innehåll", "obj"),
    ("Avsnittsrubrik", "secHead"),
    ("Två innehåll", "twoObj"),
    ("Jämförelse", "twoTxTwoObj"),
    ("Endast rubrik", "titleOnly"),
    ("Tom", "blank"),
];

fn layout(i: usize) -> String {
    let (name, typ) = LAYOUTS[i];
    let title = |id| ph(id, "Rubrik", r#"type="title""#, None, "", "", "Klicka för att ändra rubrik");
    let body = |id, idx: u32, pos: Option<(i64, i64, i64, i64)>| ph(id, "Innehåll", &format!(r#"idx="{idx}""#), pos, "", "", "Klicka för att ändra text");
    let half = BODY.2 / 2 - 114_300;
    let left = (BODY.0, BODY.1, half, BODY.3);
    let right = (BODY.0 + half + 228_600, BODY.1, half, BODY.3);
    let foot = footers(20, false);
    let shapes = match typ {
        "title" => format!(
            "{}{}{foot}",
            ph(2, "Rubrik", r#"type="ctrTitle""#, Some((1_524_000, 1_371_600, 9_144_000, 1_920_240)), r#" anchor="b""#, r#"<a:lvl1pPr algn="ctr"><a:defRPr sz="6000"/></a:lvl1pPr>"#, "Klicka för att ändra rubrik"),
            ph(3, "Underrubrik", r#"type="subTitle" idx="1""#, Some((1_524_000, 3_429_000, 9_144_000, 1_371_600)), "", r#"<a:lvl1pPr marL="0" indent="0" algn="ctr"><a:buNone/><a:defRPr sz="2400"/></a:lvl1pPr>"#, "Klicka för att ändra underrubrik"),
        ),
        "obj" => format!("{}{}{foot}", title(2), body(3, 1, None)),
        "secHead" => format!(
            "{}{}{foot}",
            ph(2, "Rubrik", r#"type="title""#, Some((1_143_000, 1_828_800, 9_906_000, 1_600_200)), r#" anchor="b""#, r#"<a:lvl1pPr><a:defRPr sz="5400"/></a:lvl1pPr>"#, "Klicka för att ändra rubrik"),
            ph(3, "Text", r#"type="body" idx="1""#, Some((1_143_000, 3_543_300, 9_906_000, 1_257_300)), "", r#"<a:lvl1pPr marL="0" indent="0"><a:buNone/><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr>"#, "Klicka för att ändra text"),
        ),
        "twoObj" => format!("{}{}{}{foot}", title(2), body(3, 1, Some(left)), body(4, 2, Some(right))),
        "twoTxTwoObj" => {
            let head = |id, idx: u32, x: i64| ph(id, "Text", &format!(r#"type="body" idx="{idx}""#), Some((x, 1_737_360, half, 640_080)), r#" anchor="b""#, r#"<a:lvl1pPr marL="0" indent="0"><a:buNone/><a:defRPr sz="2400" b="1"/></a:lvl1pPr>"#, "Klicka för att ändra text");
            let obj = |id, idx: u32, x: i64| ph(id, "Innehåll", &format!(r#"idx="{idx}""#), Some((x, 2_377_440, half, 3_566_160)), "", "", "Klicka för att ändra text");
            format!("{}{}{}{}{}{foot}", title(2), head(3, 1, left.0), obj(4, 2, left.0), head(5, 3, right.0), obj(6, 4, right.0))
        }
        "titleOnly" => format!("{}{foot}", title(2)),
        _ => foot,
    };
    let t = tree(&shapes).replace("{NAME}", &format!(r#" name="{name}""#));
    format!(r#"{XML}<p:sldLayout xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}" type="{typ}" preserve="1">{t}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#)
}

fn rels(items: &[(&str, &str, &str)]) -> String {
    let body: String = items.iter().map(|(id, kind, target)| format!(r#"<Relationship Id="{id}" Type="{REL}/{kind}" Target="{target}"/>"#)).collect();
    format!(r#"{XML}<Relationships xmlns="{PKG_REL}">{body}</Relationships>"#)
}

fn notes_master() -> String {
    let sp = |id, name, ph: &str, (x, y, w, h): (i64, i64, i64, i64), body: &str| {
        format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr><a:spLocks noGrp="1"{}/></p:cNvSpPr><p:nvPr><p:ph {ph}/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{}</p:spPr>{body}</p:sp>"#, if ph.contains("sldImg") { " noRot=\"1\" noChangeAspect=\"1\"" } else { "" }, if ph.contains("sldImg") { r#"<a:noFill/><a:ln w="12700"><a:solidFill><a:prstClr val="black"/></a:solidFill></a:ln>"# } else { "" })
    };
    let shapes = format!(
        "{}{}",
        sp(2, "Bildbild", r#"type="sldImg" idx="2""#, (685_800, 1_143_000, 5_486_400, 3_086_100), ""),
        sp(3, "Anteckningar", r#"type="body" sz="quarter" idx="3""#, (685_800, 4_400_550, 5_486_400, 3_600_450), r#"<p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0"/><a:lstStyle/><a:p><a:pPr lvl="0"/><a:r><a:rPr lang="sv-SE"/><a:t>Klicka för att ändra anteckningar</a:t></a:r></a:p></p:txBody>"#),
    );
    let t = tree(&shapes).replace("{NAME}", "");
    let lvls: String = (1..=9).map(|l| format!(r#"<a:lvl{l}pPr marL="{}" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1"><a:defRPr sz="1200" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl{l}pPr>"#, 457_200 * (l - 1))).collect();
    format!(r#"{XML}<p:notesMaster xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}">{t}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:notesStyle>{lvls}</p:notesStyle></p:notesMaster>"#)
}

/// En ny presentation (`.pptx`) med en första bild i layouten Rubrikbild.
pub fn build(v: Variant) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, data: &str| {
        let _ = z.start_file(name, opt);
        let _ = z.write_all(data.as_bytes());
    };
    let pm = "application/vnd.openxmlformats-officedocument.presentationml";
    let mut ct = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="{pm}.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="{pm}.slideMaster+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="{pm}.slide+xml"/><Override PartName="/ppt/notesMasters/notesMaster1.xml" ContentType="{pm}.notesMaster+xml"/><Override PartName="/ppt/presProps.xml" ContentType="{pm}.presProps+xml"/><Override PartName="/ppt/viewProps.xml" ContentType="{pm}.viewProps+xml"/><Override PartName="/ppt/tableStyles.xml" ContentType="{pm}.tableStyles+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/theme/theme2.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#
    );
    for i in 1..=LAYOUTS.len() {
        ct.push_str(&format!(r#"<Override PartName="/ppt/slideLayouts/slideLayout{i}.xml" ContentType="{pm}.slideLayout+xml"/>"#));
    }
    ct.push_str("</Types>");
    put("[Content_Types].xml", &ct);
    put("_rels/.rels", &rels(&[("rId1", "officeDocument", "ppt/presentation.xml"), ("rId2", "extended-properties", "docProps/app.xml")]).replace("</Relationships>", r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#));
    put("docProps/app.xml", &format!(r#"{XML}<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><TotalTime>0</TotalTime><Application>Kladd</Application><PresentationFormat>Bredbild</PresentationFormat><Slides>1</Slides><Notes>0</Notes><HiddenSlides>0</HiddenSlides></Properties>"#));
    put("docProps/core.xml", &format!(r#"{XML}<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title></dc:title><dc:creator>Kladd</dc:creator></cp:coreProperties>"#));
    let mut prels: Vec<(String, &str, String)> = vec![("rId1".into(), "slideMaster", "slideMasters/slideMaster1.xml".into()), ("rId2".into(), "notesMaster", "notesMasters/notesMaster1.xml".into()), ("rId3".into(), "slide", "slides/slide1.xml".into()), ("rId4".into(), "presProps", "presProps.xml".into()), ("rId5".into(), "viewProps", "viewProps.xml".into()), ("rId6".into(), "theme", "theme/theme1.xml".into()), ("rId7".into(), "tableStyles", "tableStyles.xml".into())];
    prels.sort_by(|a, b| a.0.cmp(&b.0));
    let prels: Vec<(&str, &str, &str)> = prels.iter().map(|(a, b, c)| (a.as_str(), *b, c.as_str())).collect();
    put("ppt/_rels/presentation.xml.rels", &rels(&prels));
    let defaults: String = (1..=9).map(|l| format!(r#"<a:lvl{l}pPr marL="{}" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1"><a:defRPr sz="1800" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl{l}pPr>"#, 457_200 * (l - 1))).collect();
    put("ppt/presentation.xml", &format!(r#"{XML}<p:presentation xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}" saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:notesMasterIdLst><p:notesMasterId r:id="rId2"/></p:notesMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId3"/></p:sldIdLst><p:sldSz cx="{}" cy="{}"/><p:notesSz cx="6858000" cy="9144000"/><p:defaultTextStyle><a:defPPr><a:defRPr lang="sv-SE"/></a:defPPr>{defaults}</p:defaultTextStyle></p:presentation>"#, SLIDE.0, SLIDE.1));
    put("ppt/presProps.xml", &format!(r#"{XML}<p:presentationPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"/>"#));
    put("ppt/viewProps.xml", &format!(r#"{XML}<p:viewPr xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:normalViewPr><p:restoredLeft sz="15620"/><p:restoredTop sz="94660"/></p:normalViewPr><p:gridSpacing cx="72008" cy="72008"/></p:viewPr>"#));
    put("ppt/tableStyles.xml", &format!(r#"{XML}<a:tblStyleLst xmlns:a="{NS_A}" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#));
    put("ppt/theme/theme1.xml", &theme(v, "Kladd"));
    put("ppt/theme/theme2.xml", &theme(v, "Kladd anteckningar"));
    put("ppt/slideMasters/slideMaster1.xml", &master(v));
    let mut mrels: Vec<(String, &str, String)> = (1..=LAYOUTS.len()).map(|i| (format!("rId{i}"), "slideLayout", format!("../slideLayouts/slideLayout{i}.xml"))).collect();
    mrels.push((format!("rId{}", LAYOUTS.len() + 1), "theme", "../theme/theme1.xml".into()));
    let mrels: Vec<(&str, &str, &str)> = mrels.iter().map(|(a, b, c)| (a.as_str(), *b, c.as_str())).collect();
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", &rels(&mrels));
    for i in 0..LAYOUTS.len() {
        put(&format!("ppt/slideLayouts/slideLayout{}.xml", i + 1), &layout(i));
        put(&format!("ppt/slideLayouts/_rels/slideLayout{}.xml.rels", i + 1), &rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]));
    }
    put("ppt/notesMasters/notesMaster1.xml", &notes_master());
    put("ppt/notesMasters/_rels/notesMaster1.xml.rels", &rels(&[("rId1", "theme", "../theme/theme2.xml")]));
    // Första bilden: Rubrikbild med tomma platshållare (redigeringen visar ledtexterna).
    let slide1 = format!("{}{}", ph(2, "Rubrik 1", r#"type="ctrTitle""#, None, "", "", ""), ph(3, "Underrubrik 2", r#"type="subTitle" idx="1""#, None, "", "", ""));
    let t = tree(&slide1).replace("{NAME}", "");
    put("ppt/slides/slide1.xml", &format!(r#"{XML}<p:sld xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}">{t}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#));
    put("ppt/slides/_rels/slide1.xml.rels", &rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]));
    z.finish().map(|c| c.into_inner()).unwrap_or_default()
}
