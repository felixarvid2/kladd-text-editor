//! Förberedelse av Markdown för förhandsvisningen. egui_commonmark ritar inte inbyggd HTML,
//! så Kladds HTML-format (`<u>`, `<span style>`, `<div align>`, `<img>`, `<br>`, kommentarerna
//! `<!-- kladd: … -->` och `<!--k:N-->`) tas bort eller görs om till vanlig Markdown innan
//! texten visas. Texten ändras bara för förhandsvisningen, aldrig i dokumentet.

/// Rensar `markdown` för förhandsvisning i ett enda svep. Kodstycken (``` / ~~~) och
/// kodspann lämnas orörda, liksom tecken efter ett backslash (`\<span` är vanlig text).
pub fn clean(markdown: &str) -> String {
    // Spårade ändringar godkänns och kommentarer utelämnas i förhandsvisningen.
    let cleaned = crate::review::clean_for_export(markdown);
    let markdown = cleaned.as_str();
    let mut out = String::with_capacity(markdown.len());
    // Öppet kodstaket: (tecken, antal).
    let mut fence: Option<(u8, usize)> = None;
    for (n, raw) in markdown.split_inclusive('\n').enumerate() {
        let mut line = raw;
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        if indent <= 3 {
            if let Some((ch, len)) = fence_marker(trimmed) {
                match fence {
                    None => fence = Some((ch, len)),
                    Some((c, l)) if c == ch && len >= l && trimmed[len..].trim().is_empty() => fence = None,
                    _ => {}
                }
                out.push_str(line);
                continue;
            }
        }
        if fence.is_some() {
            out.push_str(line);
            continue;
        }
        // Grundformatskommentaren på första raden.
        if n == 0 && line.starts_with("<!-- kladd:") {
            if let Some(end) = line.find("-->") {
                line = &line[end + 3..];
                if line.trim().is_empty() {
                    continue;
                }
            }
        }
        let before = out.len();
        inline(line, &mut out);
        // Rader som bara bestod av borttagna taggar (`<div …>`, `</div>`, markörer) försvinner helt.
        if out[before..].trim().is_empty() && !line.trim().is_empty() {
            out.truncate(before);
        }
    }
    out
}

/// Ett staket börjar med minst tre lika tecken (` eller ~); returnerar tecken och antal.
fn fence_marker(s: &str) -> Option<(u8, usize)> {
    let b = s.as_bytes();
    let c = *b.first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let len = b.iter().take_while(|&&x| x == c).count();
    // Ett inledande ```-staket får inte ha backticks i infosträngen.
    (len >= 3 && !(c == b'`' && s[len..].contains('`'))).then_some((c, len))
}

/// Behandlar en rad utanför kodstaket.
fn inline(line: &str, out: &mut String) {
    let b = line.as_bytes();
    let mut i = 0;
    let mut start = 0; // början på text som ännu inte kopierats
    while i < b.len() {
        match b[i] {
            b'\\' => {
                // Hoppa över det escapade tecknet (hela UTF-8-tecknet).
                i += 1;
                if i < b.len() {
                    i += line[i..].chars().next().map_or(1, char::len_utf8);
                }
            }
            b'`' => {
                let run = b[i..].iter().take_while(|&&x| x == b'`').count();
                let mut j = i + run;
                let mut end = None;
                while j < b.len() {
                    if b[j] == b'`' {
                        let r = b[j..].iter().take_while(|&&x| x == b'`').count();
                        if r == run {
                            end = Some(j + r);
                            break;
                        }
                        j += r;
                    } else {
                        j += 1;
                    }
                }
                i = end.unwrap_or(i + run);
            }
            b'<' => {
                let Some(close) = line[i..].find('>') else {
                    i += 1;
                    continue;
                };
                let tag = &line[i..i + close + 1];
                if let Some(rep) = replace_tag(tag) {
                    out.push_str(&line[start..i]);
                    out.push_str(&rep);
                    i += tag.len();
                    start = i;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out.push_str(&line[start..]);
}

/// Ersättning för en känd tagg, eller `None` om den ska stå kvar som den är.
fn replace_tag(tag: &str) -> Option<String> {
    if tag.starts_with("<!--k:") && tag.ends_with("-->") && tag.len() > 9 && tag[6..tag.len() - 3].bytes().all(|c| c.is_ascii_digit()) {
        return Some(String::new());
    }
    let inner = tag.strip_prefix('<')?.strip_suffix('>')?;
    let inner = inner.strip_suffix('/').unwrap_or(inner).trim_end();
    let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
    let name = inner[..name_end].to_ascii_lowercase();
    let attrs = &inner[name_end..];
    match name.as_str() {
        "u" | "/u" | "span" | "/span" | "div" | "/div" => Some(String::new()),
        "br" => Some("\\\n".into()),
        "img" => {
            let src = attr(attrs, "src")?;
            let alt = attr(attrs, "alt").unwrap_or_default();
            Some(format!("![{alt}]({})", src.replace(' ', "%20")))
        }
        _ => None,
    }
}

/// Värdet på attributet `name="…"` (eller `'…'`) i en attributlista.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(p) = attrs[from..].find(name) {
        let at = from + p;
        from = at + name.len();
        let boundary = attrs[..at].chars().next_back().is_none_or(char::is_whitespace);
        let rest = attrs[from..].trim_start();
        if !boundary || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let q = rest.chars().next()?;
        if q == '"' || q == '\'' {
            let body = &rest[1..];
            return Some(body[..body.find(q)?].to_string());
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn understrykning() {
        assert_eq!(clean("a <u>understruken</u> b\n"), "a understruken b\n");
    }

    #[test]
    fn span_med_stil() {
        assert_eq!(clean("<span style=\"color: #C00000; font-size: 14pt\">röd</span> text"), "röd text");
    }

    #[test]
    fn nästlat() {
        assert_eq!(clean("<span style=\"color: red\"><u>**fet**</u></span>\n"), "**fet**\n");
    }

    #[test]
    fn div_rader_tas_bort() {
        let t = "<div align=\"center\">\n\nMitten\n\n</div>\n\nSlut\n";
        assert_eq!(clean(t), "\nMitten\n\n\nSlut\n");
    }

    #[test]
    fn grundformatskommentar() {
        let t = "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->\n# Rubrik\n";
        assert_eq!(clean(t), "# Rubrik\n");
        // Bara på första raden.
        assert!(clean("x\n<!-- kladd: a -->\n").contains("<!-- kladd"));
    }

    #[test]
    fn objektmarkörer() {
        assert_eq!(clean("fotnot<!--k:12--> här<!--k:3-->\n"), "fotnot här\n");
        assert_eq!(clean("<!--k:7-->\ntext\n"), "text\n");
    }

    #[test]
    fn bild() {
        assert_eq!(
            clean("<img src=\"bild:image 1.png\" alt=\"Diagram över åäö\" width=\"240\">"),
            "![Diagram över åäö](bild:image%201.png)"
        );
        assert_eq!(clean("<img src='a.png' />"), "![](a.png)");
    }

    #[test]
    fn radbrytning() {
        assert_eq!(clean("a<br>b<br/>c"), "a\\\nb\\\nc");
    }

    #[test]
    fn kodspann_orörda() {
        let t = "Skriv `<u>x</u>` och ``<span style=\"a\">`</span>`` här <u>y</u>\n";
        assert_eq!(clean(t), "Skriv `<u>x</u>` och ``<span style=\"a\">`</span>`` här y\n");
    }

    #[test]
    fn kodblock_orörda() {
        let t = "```html\n<div align=\"center\">\n<u>x</u><br>\n</div>\n```\nefter <u>a</u>\n\n~~~\n<span>s</span>\n~~~\n";
        assert_eq!(
            clean(t),
            "```html\n<div align=\"center\">\n<u>x</u><br>\n</div>\n```\nefter a\n\n~~~\n<span>s</span>\n~~~\n"
        );
    }

    #[test]
    fn escapad_text() {
        assert_eq!(clean("skriv \\<span> så\n"), "skriv \\<span> så\n");
        assert_eq!(clean("\\<u>ö</u>\n"), "\\<u>ö\n");
    }

    #[test]
    fn okända_taggar_och_jämförelser_står_kvar() {
        assert_eq!(clean("a < b och c > d, <kbd>Ctrl</kbd>\n"), "a < b och c > d, <kbd>Ctrl</kbd>\n");
    }

    #[test]
    fn flerradigt_dokument_på_svenska() {
        let t = "<!-- kladd: font-size: 12pt -->\n# Årsrapport\n\n<div align=\"right\">\n\n<span style=\"color: #0000FF\">Blå</span> och <u>understruken</u> text.<!--k:1-->\n\n</div>\n\n- Punkt<br>två\n";
        assert_eq!(clean(t), "# Årsrapport\n\n\nBlå och understruken text.\n\n\n- Punkt\\\ntvå\n");
    }
}
