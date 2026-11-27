//! Ljud och video på en bild: vilka mediaformer bilden har, var filen finns och hur den ska spelas
//! (urklipp, volym, hur länge den fortsätter), samt ljudet som hör till bildens övergång.
//!
//! Rent och provat utan ljudenhet. Själva uppspelningen finns i `media_play.rs`.
//!
//! Mediaformen är en `p:pic` vars `p:nvPr` har `a:videoFile`/`a:audioFile`/`a:wavAudioFile` (`r:link`) och/eller
//! `p14:media` (`r:embed`, den inbäddade filen under `ppt/media/`). `p14:media` väljs före `r:link`: länken är
//! ofta en död extern referens (`Target="NULL"`). Uppspelningsvärdena står i tidsträdet: `p:video`/`p:audio`
//! med `p:cMediaNode` (`vol`, `mute`, `numSld`, `showWhenStopped`).

use super::model::{Deck, Slide};
use super::package::{self, Rel};
use super::xml::Node;

/// Ljud eller video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Audio,
    Video,
}

/// Var mediafilen finns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Inbäddad del i paketet (`ppt/media/media1.mp4`).
    Part(String),
    /// Länkad fil utanför presentationen (sökväg eller `file:`-adress som den står).
    File(String),
}

/// En mediaform på en bild.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaShape {
    /// `p:cNvPr/@id`.
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub source: Source,
    /// Urklipp ur `p14:trim`: millisekunder som klipps bort i början respektive slutet.
    pub trim_start_ms: f64,
    pub trim_end_ms: f64,
    /// Volym 0..1 (`cMediaNode/@vol`, 100000 = full; standard full).
    pub volume: f32,
    pub mute: bool,
    /// Antal bilder mediet fortsätter över (`numSld`, standard 1 = stannar när bilden byts).
    pub num_slides: u32,
    /// `showWhenStopped` (standard sant): formen syns när mediet står stilla.
    pub show_when_stopped: bool,
    /// Placeringen på bilden i punkter (x, y, bredd, höjd), för toppnivåformer.
    pub rect_pt: Option<[f32; 4]>,
}

impl MediaShape {
    /// Mediets spelbara längd efter urklipp, om hela längden `total_s` är känd.
    pub fn trimmed_len(&self, total_s: f64) -> f64 {
        (total_s - (self.trim_start_ms + self.trim_end_ms) / 1000.0).max(0.0)
    }
}

/// Ljudet i bildens övergång (`p:transition/p:sndAc/p:stSnd`).
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSound {
    pub part: String,
    pub name: String,
    pub looping: bool,
}

/// Allt medierelaterat på en bild.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlideMedia {
    pub shapes: Vec<MediaShape>,
    pub sound: Option<TransitionSound>,
    /// `p:endSnd`: ett pågående övergångsljud tystas när bilden visas.
    pub end_sound: bool,
}

impl SlideMedia {
    pub fn shape(&self, id: u32) -> Option<&MediaShape> {
        self.shapes.iter().find(|s| s.id == id)
    }
}

/// Mediet på bild `i`.
pub fn of_slide(deck: &Deck, i: usize) -> SlideMedia {
    deck.slides.get(i).map(|s| parse(s, &deck.pkg)).unwrap_or_default()
}

fn parse(slide: &Slide, pkg: &package::Package) -> SlideMedia {
    let mut out = SlideMedia::default();
    let Some(tree) = slide.root.path(&["p:cSld", "p:spTree"]) else { return out };
    let cfg = playback_settings(&slide.root);
    collect(tree, true, &slide.rels, &cfg, &mut out.shapes);
    // Bara ljud som finns inbäddat kan spelas (PowerPoints inbyggda ljud har bara ett namn).
    if let Some(tr) = slide.root.child("p:transition") {
        if let Some(ac) = tr.child("p:sndAc") {
            if ac.child("p:endSnd").is_some() {
                out.end_sound = true;
            }
            if let Some(snd) = ac.path(&["p:stSnd", "p:snd"]) {
                let rel = snd.attr("r:embed").and_then(|id| package::rel(&slide.rels, id));
                if let Some(r) = rel.filter(|r| !r.external && pkg.read(&r.target).is_some()) {
                    let looping = ac.child("p:stSnd").and_then(|s| s.attr_bool("loop")).unwrap_or(false);
                    out.sound = Some(TransitionSound { part: r.target.clone(), name: snd.attr("name").unwrap_or("").to_owned(), looping });
                }
            }
        }
    }
    out
}

/// Värdena ur `p:cMediaNode` per form-id.
#[derive(Default)]
struct Settings(Vec<(u32, f32, bool, u32, bool)>);

fn playback_settings(root: &Node) -> Settings {
    let mut s = Settings::default();
    if let Some(timing) = root.child("p:timing") {
        timing.walk(&mut |n| {
            if n.name != "p:cMediaNode" {
                return;
            }
            let Some(id) = n.path(&["p:tgtEl", "p:spTgt"]).and_then(|t| t.attr_i64("spid")) else { return };
            let vol = n.attr_f64("vol").map_or(1.0, |v| (v / 100_000.0).clamp(0.0, 1.0) as f32);
            let mute = n.attr_bool("mute").unwrap_or(false);
            let num = n.attr_i64("numSld").map_or(1, |v| v.clamp(1, 1000) as u32);
            let show = n.attr_bool("showWhenStopped").unwrap_or(true);
            s.0.push((id as u32, vol, mute, num, show));
        });
    }
    s
}

fn collect(tree: &Node, top: bool, rels: &[Rel], cfg: &Settings, out: &mut Vec<MediaShape>) {
    for c in &tree.children {
        match c.name.as_str() {
            "p:pic" => {
                if let Some(m) = shape_of(c, top, rels, cfg) {
                    out.push(m);
                }
            }
            "p:grpSp" => collect(c, false, rels, cfg, out),
            _ => {}
        }
    }
}

fn shape_of(pic: &Node, top: bool, rels: &[Rel], cfg: &Settings) -> Option<MediaShape> {
    let nv = pic.child("p:nvPicPr")?;
    let cnv = nv.child("p:cNvPr")?;
    let id = cnv.attr_i64("id")? as u32;
    let nvpr = nv.child("p:nvPr")?;
    let (kind, link) = ["a:videoFile", "a:audioFile", "a:wavAudioFile", "a:quickTimeFile"].iter().find_map(|n| {
        nvpr.child(n).map(|f| (if *n == "a:videoFile" || *n == "a:quickTimeFile" { Kind::Video } else { Kind::Audio }, f))
    })?;
    let media = nvpr.find("p14:media");
    let embed = media.and_then(|m| m.attr("r:embed")).and_then(|id| package::rel(rels, id));
    let link = link.attr("r:link").or_else(|| link.attr("r:embed")).and_then(|id| package::rel(rels, id));
    let source = match (embed, link) {
        (Some(r), _) if !r.external => Source::Part(r.target.clone()),
        (_, Some(r)) if !r.external => Source::Part(r.target.clone()),
        (_, Some(r)) if !r.target.is_empty() && r.target != "NULL" => Source::File(r.target.clone()),
        (Some(r), _) if !r.target.is_empty() && r.target != "NULL" => Source::File(r.target.clone()),
        _ => return None,
    };
    let trim = media.and_then(|m| m.child("p14:trim"));
    let ms = |a: &str| trim.and_then(|t| t.attr_f64(a)).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0);
    let (volume, mute, num_slides, show_when_stopped) =
        cfg.0.iter().find(|c| c.0 == id).map_or((1.0, false, 1, true), |c| (c.1, c.2, c.3, c.4));
    let rect_pt = if top {
        pic.path(&["p:spPr", "a:xfrm"]).and_then(|x| {
            let off = x.child("a:off")?;
            let ext = x.child("a:ext")?;
            let e = |n: &Node, a: &str| n.attr_f64(a).map(|v| (v / 12700.0) as f32);
            Some([e(off, "x")?, e(off, "y")?, e(ext, "cx")?, e(ext, "cy")?])
        })
    } else {
        None
    };
    Some(MediaShape {
        id,
        name: cnv.attr("name").unwrap_or("").to_owned(),
        kind,
        source,
        trim_start_ms: ms("st"),
        trim_end_ms: ms("end"),
        volume,
        mute,
        num_slides,
        show_when_stopped,
        rect_pt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::xml;

    const SLIDE: &str = r#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
        xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
        xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
        xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main"><p:cSld><p:spTree>
        <p:pic><p:nvPicPr><p:cNvPr id="8" name="film"/><p:cNvPicPr/><p:nvPr><a:videoFile r:link="rId2"/>
          <p:extLst><p:ext uri="{DAA4B4D4-6D71-4841-9C94-3DE7FCFB9230}"><p14:media r:embed="rId1"><p14:trim st="500" end="1500"/></p14:media></p:ext></p:extLst></p:nvPr></p:nvPicPr>
          <p:blipFill/><p:spPr><a:xfrm><a:off x="127000" y="254000"/><a:ext cx="1270000" cy="635000"/></a:xfrm></p:spPr></p:pic>
        <p:pic><p:nvPicPr><p:cNvPr id="9" name="ljud"/><p:cNvPicPr/><p:nvPr><a:audioFile r:link="rId3"/></p:nvPr></p:nvPicPr><p:blipFill/><p:spPr/></p:pic>
        </p:spTree></p:cSld>
        <p:transition><p:sndAc><p:stSnd><p:snd r:embed="rId4" name="klick.wav"/></p:stSnd></p:sndAc></p:transition>
        <p:timing><p:tnLst><p:par><p:cTn id="1" nodeType="tmRoot"><p:childTnLst>
          <p:seq><p:cTn id="2" nodeType="mainSeq"><p:childTnLst>
            <p:par><p:cTn id="3"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="5" presetClass="mediacall"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst>
              <p:cmd type="call" cmd="playFrom(1.5)"><p:cBhvr><p:cTn id="6"/><p:tgtEl><p:spTgt spid="8"/></p:tgtEl></p:cBhvr></p:cmd></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par>
          </p:childTnLst></p:cTn></p:seq>
          <p:video><p:cMediaNode vol="50000" numSld="3" showWhenStopped="0"><p:cTn id="7"/><p:tgtEl><p:spTgt spid="8"/></p:tgtEl></p:cMediaNode></p:video>
          <p:audio><p:cMediaNode mute="1"><p:cTn id="8"/><p:tgtEl><p:spTgt spid="9"/></p:tgtEl></p:cMediaNode></p:audio>
        </p:childTnLst></p:cTn></p:par></p:tnLst></p:timing></p:sld>"#;

    fn rel(id: &str, target: &str, external: bool) -> Rel {
        Rel { id: id.into(), kind: String::new(), target: target.into(), external }
    }

    #[test]
    fn mediaformer_trim_volym_och_källa() {
        let root = xml::parse(SLIDE.as_bytes()).unwrap();
        let slide = Slide {
            part: "ppt/slides/slide1.xml".into(),
            root,
            rels: vec![rel("rId1", "ppt/media/media1.mp4", false), rel("rId2", "ppt/media/media1.mp4", false), rel("rId3", "NULL", true), rel("rId4", "ppt/media/klick.wav", false)],
            layout: None,
            hidden: false,
            notes: None,
        };
        let pkg = package::Package::from_parts([("ppt/media/klick.wav".to_owned(), std::sync::Arc::new(vec![0u8; 4]))].into_iter().collect());
        let m = parse(&slide, &pkg);
        // Det döda länkmålet "NULL" ger ingen form; p14:media-delen väljs före r:link.
        assert_eq!(m.shapes.len(), 1);
        let v = &m.shapes[0];
        assert_eq!((v.id, v.kind), (8, Kind::Video));
        assert_eq!(v.source, Source::Part("ppt/media/media1.mp4".into()));
        assert_eq!((v.trim_start_ms, v.trim_end_ms), (500.0, 1500.0));
        assert!((v.volume - 0.5).abs() < 1e-6 && v.num_slides == 3 && !v.show_when_stopped);
        assert_eq!(v.rect_pt, Some([10.0, 20.0, 100.0, 50.0]));
        assert!((v.trimmed_len(10.0) - 8.0).abs() < 1e-9);
        assert_eq!(m.sound, Some(TransitionSound { part: "ppt/media/klick.wav".into(), name: "klick.wav".into(), looping: false }));
    }

    #[test]
    fn ljudform_med_tystat_läge() {
        let root = xml::parse(SLIDE.as_bytes()).unwrap();
        let slide = Slide {
            part: "ppt/slides/slide1.xml".into(),
            root,
            rels: vec![rel("rId3", "ppt/media/a.mp3", false)],
            layout: None,
            hidden: false,
            notes: None,
        };
        let m = parse(&slide, &package::Package::from_parts(Default::default()));
        let a = m.shape(9).unwrap();
        assert_eq!(a.kind, Kind::Audio);
        assert!(a.mute && a.show_when_stopped && a.num_slides == 1 && a.volume == 1.0);
        assert!(m.sound.is_none());
    }
}
