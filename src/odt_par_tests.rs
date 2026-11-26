//! Tester för den flertrådade tolkningen av `content.xml` (`parse_xml_par`): samma träd som den sekventiella.
use super::*;

fn dump(n: &Node, out: &mut String) {
    out.push('<');
    out.push_str(&n.name);
    for (k, v) in &n.attrs {
        out.push_str(&format!(" {k}={v:?}"));
    }
    out.push('>');
    for k in &n.kids {
        match k {
            Kid::El(e) => dump(e, out),
            Kid::Text(t) => out.push_str(&format!("[{t:?}]")),
        }
    }
    out.push_str("</>");
}

fn tree(n: &Node) -> String {
    let mut s = String::new();
    dump(n, &mut s);
    s
}

fn doc(paras: usize, flavour: usize) -> String {
    let mut x = String::from("<?xml version=\"1.0\"?>\n<office:document-content xmlns:office=\"o\" xmlns:text=\"t\">\n<office:scripts/>\n<office:automatic-styles><style:style style:name=\"P1\" a=\"x>y\"/></office:automatic-styles>\n<office:body>\n<office:text>\n<text:sequence-decls/>\n");
    for i in 0..paras {
        match (i + flavour) % 6 {
            0 => x.push_str(&format!("<text:p text:style-name=\"P1\">Rad {i} &amp; &lt;mer&gt; &#228;r <text:span a='q\"r'>fet</text:span> slut</text:p>\n")),
            1 => x.push_str(&format!("<text:p>Text <!-- kommentar --> efter {i}<![CDATA[ rå <text> ]]> klart</text:p>")),
            2 => x.push_str("<text:p/>\n"),
            3 => x.push_str(&format!("<table:table><table:table-row><table:table-cell><text:p>Cell {i}</text:p></table:table-cell></table:table-row></table:table>\n")),
            4 => x.push_str(&format!("\n  lös text {i}  \n<text:h text:outline-level=\"1\">Rubrik {i}</text:h>")),
            _ => x.push_str(&format!("<?pi x?><text:p a=\"{i}\">&apos;citat&apos;</text:p>\t")),
        }
    }
    x.push_str("\n</office:text>\n</office:body>\n</office:document-content>\n");
    x
}

#[test]
fn parallel_parse_gives_the_same_tree() {
    for flavour in 0..6 {
        for paras in [8, 9, 40, 333] {
            let xml = doc(paras, flavour);
            let seq = parse_xml_seq(&xml).unwrap();
            for workers in [2, 3, 8] {
                let par = parse_xml_par(&xml, workers);
                // Få barn ger ingen uppdelning; annars ska trädet vara identiskt.
                if let Some(par) = par {
                    assert_eq!(tree(&seq), tree(&par), "flavour {flavour}, {paras} stycken, {workers} trådar");
                } else {
                    assert!(paras < 4 * workers, "flavour {flavour}, {paras} stycken, {workers} trådar delades inte");
                }
            }
        }
    }
}

#[test]
fn parallel_parse_gives_up_on_odd_documents() {
    // Fel i XML:en: den sekventiella tolkningen ger felet, den parallella ger upp.
    let mut xml = doc(100, 0);
    xml = xml.replace("</office:body>", "</office:oops>");
    assert!(parse_xml_par(&xml, 4).is_none());
    assert!(parse_xml_seq(&xml).is_err());
    // Utan `office:text`.
    assert!(parse_xml_par("<a><b><c/><c/><c/><c/><c/><c/><c/><c/></b></a>", 2).is_none());
    // Doctype.
    assert!(parse_xml_par(&doc(100, 0).replace("<office:scripts/>", "<!DOCTYPE x>"), 4).is_none());
}

#[test]
fn parallel_parse_keeps_the_depth_limit() {
    let mut xml = doc(100, 0);
    let deep = "<text:span>".repeat(MAX_DEPTH) + &"</text:span>".repeat(MAX_DEPTH);
    xml = xml.replace("<text:sequence-decls/>", &format!("<text:p>{deep}</text:p>"));
    assert!(parse_xml_seq(&xml).is_err());
    assert!(parse_xml(&xml).is_err());
}

#[test]
fn big_odt_reads_the_same_with_and_without_threads() {
    // Ett .odt med tillräckligt många stycken för att tolkas i bitar.
    let mut md = String::new();
    for i in 0..4000 {
        md.push_str(&format!("Stycke {i} med **fet** och *kursiv* text samt en [länk](https://exempel.se/{i}).\n\n"));
        if i % 50 == 0 {
            md.push_str(&format!("## Rubrik {i}\n\n| a | b |\n|---|---|\n| {i} | x |\n\n"));
        }
    }
    let bytes = write(&md, "T", &|_| None).unwrap().bytes;
    let par = read(&bytes).unwrap();
    let content = {
        let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let mut s = String::new();
        z.by_name("content.xml").unwrap().read_to_string(&mut s).unwrap();
        s
    };
    assert!(content.len() >= PAR_MIN_BYTES, "{} byte", content.len());
    assert!(parse_xml_par(&content, 4).is_some());
    assert_eq!(tree(&parse_xml_seq(&content).unwrap()), tree(&parse_xml(&content).unwrap()));
    assert!(par.markdown.contains("Stycke 3999"));
}
