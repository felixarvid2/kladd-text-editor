//! Textigenkänning (OCR) med kommandoradsprogrammet Tesseract när det finns installerat.
//! Inget OCR-bibliotek buntas: Kladd hittar `tesseract`, listar språken, kör det på en PNG av
//! den renderade sidan med TSV-utdata och tolkar orden med rutor och säkerhet till rader och stycken.
//! Allt som startar en process ska köras i en bakgrundstråd (UI:t får aldrig vänta på Tesseract).

#![allow(dead_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Ord med lägre säkerhet än så (0–100) slängs: oftast brus från bilden.
const MIN_CONF: f32 = 20.0;
/// Längsta tid för en sida innan Tesseract avbryts.
const PAGE_TIMEOUT: Duration = Duration::from_secs(180);

/// Ett igenkänt ord. Rutan är i bildpunkter (origo uppe till vänster).
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub conf: f32,
    pub block: u32,
    pub par: u32,
    pub line: u32,
}

/// En rad ord (från vänster till höger).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// Ett stycke: rader som Tesseract räknar till samma block och stycke.
#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    pub lines: Vec<Line>,
}

impl Paragraph {
    /// Styckets text på en rad; radbrytningsstreck (`ord-` + `fortsättning`) sys ihop.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            let t = l.text();
            if t.is_empty() {
                continue;
            }
            if out.is_empty() {
                out = t;
            } else if out.ends_with('-') && out.chars().rev().nth(1).is_some_and(char::is_alphabetic) && t.starts_with(char::is_lowercase) {
                out.pop();
                out.push_str(&t);
            } else {
                out.push(' ');
                out.push_str(&t);
            }
        }
        out
    }
}

/// Tolkar Tesseracts TSV (nivå, sida, block, stycke, rad, ord, vänster, topp, bredd, höjd,
/// säkerhet, text). Bara ordrader (nivå 5) med text och tillräcklig säkerhet följer med.
pub fn parse_tsv(tsv: &str) -> Vec<Word> {
    let mut out = Vec::new();
    for row in tsv.lines() {
        let f: Vec<&str> = row.splitn(12, '\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let num = |s: &str| s.trim().parse::<f32>().ok();
        let int = |s: &str| s.trim().parse::<u32>().ok();
        let (Some(block), Some(par), Some(line)) = (int(f[2]), int(f[3]), int(f[4])) else { continue };
        let (Some(left), Some(top), Some(width), Some(height), Some(conf)) = (num(f[6]), num(f[7]), num(f[8]), num(f[9]), num(f[10])) else {
            continue;
        };
        let text = f[11].trim();
        if text.is_empty() || conf < MIN_CONF || width <= 0.0 || height <= 0.0 {
            continue;
        }
        out.push(Word { text: text.to_owned(), left, top, width, height, conf, block, par, line });
    }
    out
}

/// Grupperar orden till stycken och rader i den ordning Tesseract gav dem.
pub fn paragraphs(words: &[Word]) -> Vec<Paragraph> {
    let mut out: Vec<Paragraph> = Vec::new();
    let mut key: Option<(u32, u32, u32)> = None;
    for w in words {
        let k = (w.block, w.par, w.line);
        let new_par = key.is_none_or(|p| (p.0, p.1) != (k.0, k.1));
        if new_par {
            out.push(Paragraph { lines: vec![Line { words: vec![w.clone()] }] });
        } else if key != Some(k) {
            if let Some(p) = out.last_mut() {
                p.lines.push(Line { words: vec![w.clone()] });
            }
        } else if let Some(l) = out.last_mut().and_then(|p| p.lines.last_mut()) {
            l.words.push(w.clone());
        }
        key = Some(k);
    }
    out
}

/// Alla rader i sidan (för sökbart textlager).
pub fn lines(words: &[Word]) -> Vec<Line> {
    paragraphs(words).into_iter().flat_map(|p| p.lines).collect()
}

/// Sidans text som stycken åtskilda av tom rad.
pub fn plain_text(words: &[Word]) -> String {
    paragraphs(words).iter().map(Paragraph::text).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n")
}

// ------------------------------------------------------------ Tesseract

/// En hittad Tesseract-installation och dess språkdata.
#[derive(Clone, Debug)]
pub struct Tesseract {
    pub exe: PathBuf,
    pub langs: Vec<String>,
}

fn candidates() -> Vec<PathBuf> {
    let name = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
    #[allow(unused_mut)]
    let mut v: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join(name)).collect()).unwrap_or_default();
    #[cfg(windows)]
    {
        v.push(PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe"));
        v.push(PathBuf::from(r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe"));
    }
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from("/opt/homebrew/bin/tesseract"));
        v.push(PathBuf::from("/usr/local/bin/tesseract"));
    }
    v
}

/// Söker `tesseract` i PATH och på de vanliga installationsplatserna.
pub fn find_exe() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

/// Kör ett kommando med tidsgräns och ger standardutdata (och felutdata vid fel).
fn run(cmd: &mut Command, timeout: Duration) -> Result<(String, String), String> {
    crate::platform::quiet(cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("kunde inte starta Tesseract: {e}"))?;
    fn drain<R: Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    }
    let (out, err) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Tesseract tog för lång tid".to_owned());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("Tesseract: {e}")),
        }
    };
    let out = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if status.success() { Ok((out, err)) } else { Err(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("okänt fel").trim().to_owned()) }
}

/// Språkkoderna ur `--list-langs` (första raden är en rubrik; `osd` är inget språk).
pub fn parse_langs(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("List of") && !l.contains(' ') && *l != "osd" && *l != "equ")
        .map(str::to_owned)
        .collect()
}

impl Tesseract {
    /// Hittar Tesseract och dess språk. Startar en process: anropas bara från en bakgrundstråd.
    pub fn detect() -> Option<Tesseract> {
        let exe = find_exe()?;
        let (out, err) = run(Command::new(&exe).arg("--list-langs"), Duration::from_secs(15)).ok()?;
        // Äldre versioner skriver listan på felutdata.
        let mut langs = parse_langs(&out);
        if langs.is_empty() {
            langs = parse_langs(&err);
        }
        Some(Tesseract { exe, langs })
    }

    /// Språkargumentet (`swe+eng`) för användarens språk, bara av de språk som finns installerade.
    pub fn lang_arg(&self) -> Option<String> {
        pick_langs(&self.langs, crate::spell::system_locale_code().as_deref())
    }

    /// Känner igen texten i en PNG. Blockerar: kör i bakgrundstråd.
    pub fn recognize(&self, png: &Path) -> Result<Vec<Word>, String> {
        let lang = self.lang_arg().ok_or("Tesseract har inga språkpaket installerade")?;
        let mut cmd = Command::new(&self.exe);
        cmd.arg(png).arg("stdout").args(["-l", &lang, "--dpi", "300", "tsv"]);
        let (out, _) = run(&mut cmd, PAGE_TIMEOUT)?;
        Ok(parse_tsv(&out))
    }
}

/// Tesseract-kod (tre bokstäver) för en språkkod som `sv_SE` eller `sv`.
fn tess_code(locale: &str) -> Option<&'static str> {
    let l = locale.split(['_', '-']).next()?.to_ascii_lowercase();
    Some(match l.as_str() {
        "sv" => "swe",
        "en" => "eng",
        "nb" | "nn" | "no" => "nor",
        "da" => "dan",
        "fi" => "fin",
        "de" => "deu",
        "fr" => "fra",
        "es" => "spa",
        "it" => "ita",
        "nl" => "nld",
        "pt" => "por",
        "pl" => "pol",
        _ => return None,
    })
}

/// Väljer språk för igenkänningen: systemspråket först (om det inte är svenska/engelska), sedan
/// svenska och engelska, bara de som finns. Annars det första installerade språket.
pub fn pick_langs(available: &[String], locale: Option<&str>) -> Option<String> {
    let has = |c: &str| available.iter().any(|a| a == c);
    let mut chosen: Vec<&str> = Vec::new();
    for c in locale.and_then(tess_code).into_iter().chain(["swe", "eng"]) {
        if has(c) && !chosen.contains(&c) {
            chosen.push(c);
        }
    }
    if chosen.is_empty() {
        return available.first().cloned();
    }
    Some(chosen.join("+"))
}

/// Hur Tesseract installeras på det här systemet.
pub fn install_hint() -> &'static str {
    if cfg!(target_os = "windows") {
        "Installera Tesseract med installationsprogrammet från UB Mannheim (github.com/UB-Mannheim/tesseract/wiki) och välj svenska språkdata."
    } else if cfg!(target_os = "macos") {
        "Installera Tesseract: brew install tesseract tesseract-lang"
    } else {
        "Installera Tesseract: sudo apt install tesseract-ocr tesseract-ocr-swe"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t800\t600\t-1\t\n\
2\t1\t1\t0\t0\t0\t50\t50\t300\t100\t-1\t\n\
4\t1\t1\t1\t1\t0\t50\t50\t300\t30\t-1\t\n\
5\t1\t1\t1\t1\t1\t50\t50\t100\t30\t96.5\tHej\n\
5\t1\t1\t1\t1\t2\t160\t50\t140\t30\t91\tvärlden\n\
5\t1\t1\t1\t2\t1\t50\t90\t120\t30\t90\tför-\n\
5\t1\t1\t1\t2\t2\t180\t90\t60\t30\t12\tbrus\n\
5\t1\t1\t1\t3\t1\t50\t130\t150\t30\t88\tdelning\n\
5\t1\t1\t2\t1\t1\t50\t200\t90\t30\t93\tNytt\n\
5\t1\t1\t2\t1\t2\t150\t200\t0\t30\t93\tnoll\n\
5\t1\t2\t1\t1\t1\t50\t300\t110\t30\t95\tSlut\n\
5\t1\t2\t1\t1\t2\t170\t300\t60\t30\t95\t \n";

    #[test]
    fn tsv_ger_bara_ord() {
        let w = parse_tsv(TSV);
        let texts: Vec<&str> = w.iter().map(|w| w.text.as_str()).collect();
        // Rubrik, sida/block/rad, lågt säkra ord, tomma ord och noll bredd slängs.
        assert_eq!(texts, ["Hej", "världen", "för-", "delning", "Nytt", "Slut"]);
        assert_eq!(w[0], Word { text: "Hej".into(), left: 50.0, top: 50.0, width: 100.0, height: 30.0, conf: 96.5, block: 1, par: 1, line: 1 });
    }

    #[test]
    fn tsv_tal_som_inte_gar_att_tolka_hoppas_over() {
        assert!(parse_tsv("5\t1\tx\t1\t1\t1\t0\t0\t10\t10\t90\tabc\n5\t1\t1\t1\t1\t1\t0\t0\t10\n").is_empty());
        assert!(parse_tsv("").is_empty());
    }

    #[test]
    fn rader_och_stycken() {
        let w = parse_tsv(TSV);
        let l = lines(&w);
        assert_eq!(l.iter().map(Line::text).collect::<Vec<_>>(), ["Hej världen", "för-", "delning", "Nytt", "Slut"]);
        let p = paragraphs(&w);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].lines.len(), 3);
        // Radbrytningsstreck sys ihop; nytt stycke blir tom rad.
        assert_eq!(p[0].text(), "Hej världen fördelning");
        assert_eq!(plain_text(&w), "Hej världen fördelning\n\nNytt\n\nSlut");
    }

    #[test]
    fn streck_efter_siffra_eller_versal_behalls() {
        let p = Paragraph {
            lines: vec![Line { words: vec![Word { text: "1990-".into(), left: 0.0, top: 0.0, width: 1.0, height: 1.0, conf: 90.0, block: 1, par: 1, line: 1 }] },
                Line { words: vec![Word { text: "talet".into(), left: 0.0, top: 10.0, width: 1.0, height: 1.0, conf: 90.0, block: 1, par: 1, line: 2 }] }],
        };
        assert_eq!(p.text(), "1990- talet");
    }

    #[test]
    fn sprakval() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_langs(&l(&["eng", "swe", "osd"]), Some("sv_SE")).as_deref(), Some("swe+eng"));
        assert_eq!(pick_langs(&l(&["eng", "swe"]), None).as_deref(), Some("swe+eng"));
        assert_eq!(pick_langs(&l(&["eng"]), Some("sv_SE")).as_deref(), Some("eng"));
        assert_eq!(pick_langs(&l(&["eng", "deu", "swe"]), Some("de_DE")).as_deref(), Some("deu+swe+eng"));
        assert_eq!(pick_langs(&l(&["fra"]), Some("sv_SE")).as_deref(), Some("fra"));
        assert_eq!(pick_langs(&[], Some("sv_SE")), None);
    }

    #[test]
    fn sprakpaket_ur_listan() {
        let out = "List of available languages in \"/usr/share/tesseract-ocr/5/tessdata/\" (3):\neng\nosd\nswe\n";
        assert_eq!(parse_langs(out), ["eng", "swe"]);
        assert!(parse_langs("").is_empty());
    }

    #[test]
    fn installationstips_finns() {
        assert!(install_hint().contains("esseract"));
    }
}
