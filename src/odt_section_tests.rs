//! Tester för avsnitt i OpenDocument: sidmallar (liggande sidor), spalter i `text:section`.
use super::*;
use crate::word::sections::SectionProps;

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn marks(md: &str) -> Vec<SectionProps> {
    md.lines().filter_map(SectionProps::from_comment).collect()
}

#[test]
fn landscape_section_becomes_a_master_page() {
    let md = "Första\n\n<!-- kladd: avsnitt; page: A4 landscape; page-margin: 15mm -->\n\nLiggande\n\n<!-- kladd: avsnitt -->\n\nTillbaka\n";
    let w = write(md, "T", &|_| None).unwrap();
    let content = part(&w.bytes, "content.xml");
    let styles = part(&w.bytes, "styles.xml");
    // Två nya sidmallar; styckena efter brytningarna byter sidmall.
    assert!(styles.contains(r#"<style:master-page style:name="M1""#) && styles.contains(r#"<style:master-page style:name="M2""#), "{styles}");
    assert!(styles.contains(r#"style:print-orientation="landscape""#));
    assert!(content.contains(r#"style:master-page-name="M1""#) && content.contains(r#"style:master-page-name="M2""#), "{content}");
    // Läser man filen igen finns avsnitten kvar: liggande A4 med 15 mm marginaler, sedan grundformatet.
    let back = read(&w.bytes).unwrap();
    let m = marks(&back.markdown);
    assert_eq!(m.len(), 2, "{}", back.markdown);
    assert_eq!(m[0].page_size.map(|(w, h)| (w.round(), h.round())), Some((842.0, 595.0)));
    assert!(m[0].page_margin.is_some_and(|m| (m[0] - 42.5).abs() < 0.5));
    assert_eq!(m[1], SectionProps::default());
    let text: Vec<&str> = back.markdown.lines().filter(|l| !l.is_empty() && !l.starts_with("<!--")).collect();
    assert_eq!(text, vec!["Första", "Liggande", "Tillbaka"]);
}

#[test]
fn columns_become_text_sections_that_read_back() {
    let md = "Rubrik\n\n<!-- kladd: avsnitt; columns: 2; column-gap: 10mm; type: continuous -->\n\nI två spalter\n\n<!-- kladd: avsnitt; type: continuous -->\n\nSlut\n";
    let w = write(md, "T", &|_| None).unwrap();
    let content = part(&w.bytes, "content.xml");
    assert_eq!(content.matches("<text:section ").count(), 1, "{content}");
    assert!(content.contains(r#"fo:column-count="2""#) && content.contains(r#"fo:column-gap="1cm""#), "{content}");
    assert!(!content.contains("master-page-name"), "löpande avsnitt byter inte sida");
    let back = read(&w.bytes).unwrap();
    let m = marks(&back.markdown);
    assert_eq!(m.len(), 2, "{}", back.markdown);
    assert_eq!(m[0].columns, Some(2));
    assert!(m[0].continuous);
    assert!((m[0].column_gap.unwrap() - 28.35).abs() < 0.2);
    assert_eq!(m[1].columns, None);
    assert!(m[1].continuous);
}

#[test]
fn new_page_with_columns_and_leading_marker() {
    let md = "<!-- kladd: avsnitt; columns: 3 -->\n\nA\n\n<!-- kladd: spaltbrytning -->\n\nB\n\n<!-- kladd: avsnitt; page: A5 -->\n\nC\n";
    let w = write(md, "T", &|_| None).unwrap();
    let content = part(&w.bytes, "content.xml");
    assert!(content.contains(r#"fo:column-count="3""#) && content.contains(r#"fo:break-before="column""#), "{content}");
    assert!(content.contains(r#"style:master-page-name="M1""#));
    let back = read(&w.bytes).unwrap();
    let m = marks(&back.markdown);
    // Första avsnittets spalter, sedan en avsnittsrad som återställer (löpande) och en ny sida i A5.
    assert!(m.iter().any(|p| p.columns == Some(3)), "{}", back.markdown);
    assert!(m.iter().any(|p| p.page_size.is_some_and(|(w, _)| (w - 419.5).abs() < 1.0)), "{}", back.markdown);
    for t in ["A", "B", "C"] {
        assert!(back.markdown.lines().any(|l| l == t), "{t} saknas: {}", back.markdown);
    }
}

#[test]
fn three_part_header_gets_tab_stops_per_master_page() {
    let md = "<!-- kladd: header: \"V|M|H\"; header-first: \"Omslag\" -->\n\nEtt\n\n<!-- kladd: avsnitt; page: A4 landscape -->\n\nTvå\n";
    let w = write(md, "T", &|_| None).unwrap();
    let styles = part(&w.bytes, "styles.xml");
    // Egna styckeformat med flikar efter den liggande sidans bredd, och första sidans sidhuvud på grundmallen.
    assert!(styles.contains(r#"style:name="MP1_1""#), "{styles}");
    assert!(styles.contains("<style:header-first>"), "{styles}");
    let tab = |name: &str| {
        let at = styles.find(&format!(r#"style:name="{name}""#)).unwrap();
        let rest = &styles[at..];
        let i = rest.find(r#"style:type="right""#).unwrap();
        rest[..i].rsplit(r#"style:position=""#).next().unwrap().trim_end_matches('"').trim_end_matches(' ').to_owned()
    };
    assert_ne!(tab("MP1"), tab("MP1_1"));
}
