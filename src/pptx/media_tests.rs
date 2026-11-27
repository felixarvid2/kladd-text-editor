//! Prov för ljud och video i bildspelet (`media_play.rs`) mot en presentation byggd i provet: en video, en
//! ljudfil och ett övergångsljud. Ingen ljudenhet behövs: mixern körs för hand. Prov som kräver ffmpeg hoppar
//! över sig själva när det saknas.

use std::io::Write;
use std::sync::Arc;

use super::media_audio::{AudioOut, tests::wav};
use super::anim::{MediaCmd, MediaKind};
use super::media_play::MediaHub;
use super::media_video;
use super::Deck;

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const MS_REL: &str = "http://schemas.microsoft.com/office/2007/relationships/media";
const PPTX_CT: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";

fn poster() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(16, 9, image::Rgba([30, 60, 160, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Bild 1: en video (id 8, startar vid första klicket) och en ljudfil (id 9, en sekund ton). Bild 2: ett
/// övergångsljud. Bild 3: tom.
pub fn media_deck_bytes() -> Vec<u8> {
    media_deck_with("", "<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>", "")
}

/// Som `media_deck_bytes`; `video_attrs` läggs på videons `p:cMediaNode`, `video_geom` ersätter dess form och
/// `xfrm_attrs` läggs på dess `a:xfrm` (vridning, spegling).
pub fn media_deck_with(video_attrs: &str, video_geom: &str, xfrm_attrs: &str) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opt).unwrap();
        z.write_all(data).unwrap();
    };
    put("[Content_Types].xml", format!(r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="mp4" ContentType="video/mp4"/><Default Extension="wav" ContentType="audio/wav"/><Override PartName="/ppt/presentation.xml" ContentType="{PPTX_CT}"/></Types>"#).as_bytes());
    put("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#).as_bytes());
    let pic = |id: u32, name: &str, kind: &str, rect: (i64, i64, i64, i64)| {
        let (x, y, w, h) = rect;
        let (geom, xa) = if id == 8 { (video_geom, xfrm_attrs) } else { ("<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>", "") };
        let xa_sp = if xa.is_empty() { String::new() } else { format!(" {xa}") };
        format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="{name}"/><p:cNvPicPr/><p:nvPr><a:{kind}File r:link="rIdL{id}"/><p:extLst><p:ext uri="{{DAA4B4D4-6D71-4841-9C94-3DE7FCFB9230}}"><p14:media r:embed="rIdM{id}"/></p:ext></p:extLst></p:nvPr></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm{xa_sp}><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>{geom}</p:spPr></p:pic>"#
        )
    };
    let cmd = |spid: u32, c: &str| format!(r#"<p:cmd type="call" cmd="{c}"><p:cBhvr><p:cTn id="{}" dur="1" fill="hold"/><p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl></p:cBhvr></p:cmd>"#, 20 + spid);
    let timing = format!(
        r#"<p:timing><p:tnLst><p:par><p:cTn id="1" nodeType="tmRoot"><p:childTnLst><p:seq><p:cTn id="2" nodeType="mainSeq"><p:childTnLst><p:par><p:cTn id="3"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="4" presetClass="mediacall"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst>{}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:seq><p:video><p:cMediaNode vol="80000"{video_attrs}><p:cTn id="5"/><p:tgtEl><p:spTgt spid="8"/></p:tgtEl></p:cMediaNode></p:video><p:audio><p:cMediaNode vol="50000"><p:cTn id="6"/><p:tgtEl><p:spTgt spid="9"/></p:tgtEl></p:cMediaNode></p:audio></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"#,
        cmd(8, "playFrom(0.0)")
    );
    let tree = |inner: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#);
    let s1 = format!(
        r#"<p:sld {NS}>{}{timing}</p:sld>"#,
        tree(&(pic(8, "film", "video", (914400, 914400, 4572000, 2571750)) + &pic(9, "ton", "audio", (6400000, 914400, 609600, 609600))))
    );
    let s2 = format!(r#"<p:sld {NS}>{}<p:transition><p:sndAc><p:stSnd><p:snd r:embed="rIdSnd" name="klick.wav"/></p:stSnd></p:sndAc></p:transition></p:sld>"#, tree(""));
    let s3 = format!(r#"<p:sld {NS}>{}</p:sld>"#, tree(""));
    let mut ids = String::new();
    let mut rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#);
    for (i, s) in [s1, s2, s3].iter().enumerate() {
        let n = i + 1;
        ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rIdS{n}"/>"#, 255 + n));
        rels.push_str(&format!(r#"<Relationship Id="rIdS{n}" Type="{REL}/slide" Target="slides/slide{n}.xml"/>"#));
        put(&format!("ppt/slides/slide{n}.xml"), s.as_bytes());
        let mut sr = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
        match n {
            1 => {
                sr.push_str(&format!(r#"<Relationship Id="rIdImg" Type="{REL}/image" Target="../media/poster.png"/>"#));
                sr.push_str(&format!(r#"<Relationship Id="rIdL8" Type="{REL}/video" Target="../media/film.mp4"/><Relationship Id="rIdM8" Type="{MS_REL}" Target="../media/film.mp4"/>"#));
                sr.push_str(&format!(r#"<Relationship Id="rIdL9" Type="{REL}/audio" Target="../media/ton.wav"/><Relationship Id="rIdM9" Type="{MS_REL}" Target="../media/ton.wav"/>"#));
            }
            2 => sr.push_str(&format!(r#"<Relationship Id="rIdSnd" Type="{REL}/audio" Target="../media/klick.wav"/>"#)),
            _ => {}
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
    put("ppt/media/poster.png", &poster());
    put("ppt/media/film.mp4", include_bytes!("media_fixtures/film.mp4"));
    // En sekund 440 Hz-liknande ton (en fyrkantsvåg räcker för provet), 8 kHz mono.
    put("ppt/media/ton.wav", &wav(8000, 1, 8000, |i| if (i / 9) % 2 == 0 { 0.6 } else { -0.6 }));
    put("ppt/media/klick.wav", &wav(8000, 1, 400, |_| 0.4));
    z.finish().unwrap().into_inner()
}

fn deck() -> Arc<Deck> {
    Arc::new(Deck::open(&media_deck_bytes()).expect("presentationen öppnas"))
}

fn cmd(shape_id: u32, kind: MediaKind) -> MediaCmd {
    MediaCmd { shape_id, kind }
}

/// Skriver provfilen till `$KLADD_PMEDIA_OUT` (för att titta på bildspelet för hand).
#[test]
#[ignore]
fn skriv_provfil() {
    let Ok(p) = std::env::var("KLADD_PMEDIA_OUT") else { return };
    std::fs::write(&p, media_deck_bytes()).unwrap();
    // Och en vriden, rundad video som döljs när den inte spelas.
    let geom = r#"<a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val 30000"/></a:avLst></a:prstGeom>"#;
    std::fs::write(format!("{p}.rund.pptx"), media_deck_with(r#" showWhenStopped="0""#, geom, r#"rot="900000""#)).unwrap();
}

#[test]
fn provpresentationen_har_media_och_övergångsljud() {
    let d = deck();
    let m = super::media_model::of_slide(&d, 0);
    assert_eq!(m.shapes.len(), 2);
    assert!((m.shape(8).unwrap().volume - 0.8).abs() < 1e-6);
    assert!((m.shape(9).unwrap().volume - 0.5).abs() < 1e-6);
    assert!(super::media_model::of_slide(&d, 1).sound.is_some());
}

#[test]
fn ljud_spelas_pausas_och_tar_slut_utan_enhet() {
    let ctx = egui::Context::default();
    let audio = AudioOut::test_ready(8000);
    let mixer = audio.mixer.clone();
    let mut hub = MediaHub::new(deck(), true).with_backends(audio, None);
    hub.apply(&ctx, 0, cmd(9, MediaKind::Play { from_s: 0.0 }));
    std::thread::sleep(std::time::Duration::from_millis(300));
    let take = |frames: usize| {
        let mut out = vec![0.0f32; frames * 2];
        mixer.lock().unwrap().mix(&mut out, 2);
        out
    };
    let out = take(2000);
    // Volym 0,5 av 0,6.
    let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    assert!((peak - 0.3).abs() < 0.02, "{peak}");
    // Paus: tystnad och stillastående klocka.
    hub.apply(&ctx, 0, cmd(9, MediaKind::TogglePause));
    assert!(take(500).iter().all(|s| *s == 0.0));
    hub.apply(&ctx, 0, cmd(9, MediaKind::Resume));
    assert!(take(500).iter().any(|s| *s != 0.0));
    // Resten av sekunden, sedan är rösten klar och hubben släpper den.
    let _ = take(8000);
    hub.update(&ctx);
    assert!(hub.is_quiet(), "ljudet borde vara klart");
}

#[test]
fn stopp_och_bildbyte_stoppar_ljud_enligt_numsld() {
    let ctx = egui::Context::default();
    let audio = AudioOut::test_ready(8000);
    let mut hub = MediaHub::new(deck(), true).with_backends(audio, None);
    hub.on_position(Some((0, 0)));
    hub.apply(&ctx, 0, cmd(9, MediaKind::Play { from_s: 0.0 }));
    assert_eq!(hub.playing_on(0), vec![9]);
    // numSld = 1: ett bildbyte stoppar ljudet.
    hub.on_position(Some((1, 0)));
    assert!(hub.playing_on(0).is_empty());
    hub.apply(&ctx, 0, cmd(9, MediaKind::Play { from_s: 0.0 }));
    hub.apply(&ctx, 0, cmd(9, MediaKind::Stop));
    assert!(hub.playing_on(0).is_empty());
}

#[test]
fn övergångsljudet_spelas_vid_bildbytet() {
    let audio = AudioOut::test_ready(8000);
    let mixer = audio.mixer.clone();
    let mut hub = MediaHub::new(deck(), true).with_backends(audio, None);
    hub.on_position(Some((0, 0)));
    hub.on_position(Some((1, 0)));
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut out = vec![0.0f32; 1000 * 2];
    mixer.lock().unwrap().mix(&mut out, 2);
    assert!(out.iter().any(|s| (s.abs() - 0.4).abs() < 0.01), "klickljudet borde höras");
    // Utan animering spelas inga övergångsljud.
    let audio = AudioOut::test_ready(8000);
    let mixer = audio.mixer.clone();
    let mut hub = MediaHub::new(deck(), false).with_backends(audio, None);
    hub.on_position(Some((1, 0)));
    std::thread::sleep(std::time::Duration::from_millis(150));
    let mut out = vec![0.0f32; 400 * 2];
    mixer.lock().unwrap().mix(&mut out, 2);
    assert!(out.iter().all(|s| *s == 0.0));
}

#[test]
fn videons_ljudspår_spelas_utan_ffmpeg_och_notis_visas() {
    let ctx = egui::Context::default();
    let audio = AudioOut::test_ready(8000);
    let mut hub = MediaHub::new(deck(), true).with_backends(audio, None);
    hub.on_position(Some((0, 0)));
    assert!(hub.playing_on(0).is_empty());
    // Kommandot kommer från uppspelningen (`take_media`): utan ffmpeg spelas bara MP4:ns ljud.
    hub.apply(&ctx, 0, cmd(8, MediaKind::Play { from_s: 0.0 }));
    assert_eq!(hub.playing_on(0), vec![8], "ljudet i MP4:n spelas utan ffmpeg");
    assert!(hub.notice().is_some_and(|n| n.contains("ffmpeg")));
    hub.apply(&ctx, 0, cmd(8, MediaKind::Stop));
    assert!(hub.playing_on(0).is_empty());
}

#[test]
fn utan_ljudenhet_och_utan_ffmpeg_hängs_inget_upp() {
    let ctx = egui::Context::default();
    let mut hub = MediaHub::new(deck(), true).with_backends(AudioOut::silent(), None);
    hub.on_position(Some((0, 0)));
    hub.on_position(Some((1, 0)));
    hub.apply(&ctx, 0, cmd(8, MediaKind::TogglePause));
    hub.apply(&ctx, 0, cmd(99, MediaKind::Stop));
    hub.update(&ctx);
    assert!(hub.is_quiet());
    assert!(hub.notice().is_some());
}

#[test]
fn video_via_ffmpeg_ger_bildrutor_och_dödas_vid_stopp() {
    let Some(ff) = media_video::find_ffmpeg() else {
        eprintln!("hoppar över: ffmpeg saknas");
        return;
    };
    let ctx = egui::Context::default();
    let mut hub = MediaHub::new(deck(), true).with_backends(AudioOut::silent(), Some(ff));
    hub.on_position(Some((0, 0)));
    hub.apply(&ctx, 0, cmd(8, MediaKind::Play { from_s: 0.0 }));
    assert_eq!(hub.playing_on(0), vec![8]);
    let pid = hub.video_pid(0, 8).expect("ffmpeg startad");
    let t0 = std::time::Instant::now();
    while !hub.has_frame(0, 8) && t0.elapsed().as_secs() < 20 {
        hub.update(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(hub.has_frame(0, 8), "ingen bildruta kom");
    hub.apply(&ctx, 0, cmd(8, MediaKind::Stop));
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists(), "ffmpeg lever kvar");
    let _ = pid;
    // Bildbyte stoppar också.
    hub.apply(&ctx, 0, cmd(8, MediaKind::Play { from_s: 0.0 }));
    let pid = hub.video_pid(0, 8).unwrap();
    hub.on_position(Some((1, 0)));
    assert!(hub.playing_on(0).is_empty());
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    let _ = pid;
}

#[test]
fn video_med_showwhenstopped_noll_döljs_när_den_inte_spelas() {
    let ctx = egui::Context::default();
    let d = Arc::new(Deck::open(&media_deck_with(r#" showWhenStopped="0""#, "<a:prstGeom prst=\"ellipse\"><a:avLst/></a:prstGeom>", r#"rot="1200000" flipH="1""#)).unwrap());
    let mut hub = MediaHub::new(d, true).with_backends(AudioOut::test_ready(8000), None);
    hub.ensure(0);
    // Före start: dold. Ljudformen (id 9) har standardvärdet och syns.
    assert!(hub.hidden_when_stopped(0, 8));
    assert!(!hub.hidden_when_stopped(0, 9));
    // Okänd bild eller form döljs inte.
    assert!(!hub.hidden_when_stopped(7, 8));
    hub.apply(&ctx, 0, cmd(8, MediaKind::Play { from_s: 0.0 }));
    if !hub.playing_on(0).is_empty() {
        assert!(!hub.hidden_when_stopped(0, 8));
    }
    let m = super::media_model::of_slide(&Deck::open(&media_deck_with("", "<a:prstGeom prst=\"ellipse\"><a:avLst/></a:prstGeom>", r#"rot="1200000" flipH="1""#)).unwrap(), 0);
    let v = m.shape(8).unwrap();
    assert_eq!((v.rot_deg, v.flip_h, v.crop), (20.0, true, super::media_model::Crop::Ellipse));
    assert!(v.show_when_stopped);
}
