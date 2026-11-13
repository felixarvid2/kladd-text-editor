use super::collate::{self, SortMode};
use super::model::{self, Format, Table};
use super::sniff;
use super::stats::{self, parse_number};

fn parse(text: &str) -> Table {
    Table::parse(text, Format::COMMA)
}

fn grid(text: &str, fmt: Format) -> Vec<Vec<String>> {
    let t = Table::parse(text, fmt);
    (0..t.rows()).map(|r| (0..t.row_len(r)).map(|c| t.value(text, r, c).into_owned()).collect()).collect()
}

fn s(v: &[&[&str]]) -> Vec<Vec<String>> {
    v.iter().map(|r| r.iter().map(|c| (*c).to_owned()).collect()).collect()
}

fn apply(text: &str, p: &model::Patch) -> String {
    let mut t = text.to_owned();
    assert!(p.fits(&t));
    p.apply(&mut t);
    // Ångra ska ge tillbaka originalet.
    let mut back = t.clone();
    let inv = p.inverse();
    assert!(inv.fits(&back));
    inv.apply(&mut back);
    assert_eq!(back, text, "ångra ger inte originalet");
    t
}

// ---------------------------------------------------------------- tolkning

#[test]
fn parses_quotes_and_embedded_newlines() {
    let text = "a,\"b,1\",\"say \"\"hi\"\"\"\n\"multi\nline\",x,\n";
    assert_eq!(grid(text, Format::COMMA), s(&[&["a", "b,1", "say \"hi\""], &["multi\nline", "x", ""]]));
}

#[test]
fn parses_crlf_and_trailing_newline() {
    let t = parse("a,b\r\nc,d\r\n");
    assert_eq!(t.rows(), 2);
    assert_eq!(t.eol(), "\r\n");
    let t = parse("a,b\nc,d");
    assert_eq!(t.rows(), 2);
    assert_eq!(parse("").rows(), 0);
    assert_eq!(parse("\n").rows(), 1);
}

#[test]
fn ragged_rows_keep_their_length() {
    let t = parse("a,b,c\nd\ne,f\n");
    assert_eq!(t.ncols(), 3);
    assert_eq!(t.row_len(1), 1);
    assert_eq!(t.raw("a,b,c\nd\ne,f\n", 2, 5), "");
}

#[test]
fn cell_at_finds_cell() {
    let text = "ab,cd\nef,gh\n";
    let t = parse(text);
    assert_eq!(t.cell_at(0), Some((0, 0)));
    assert_eq!(t.cell_at(3), Some((0, 1)));
    assert_eq!(t.cell_at(7), Some((1, 0)));
    assert_eq!(t.cell_at(10), Some((1, 1)));
}

#[test]
fn round_trip_is_byte_identical_without_edits() {
    for text in ["a,b\r\nc,\"d\r\ne\"\r\n", "x;y\n1;2", "\"q\"\"\",z\n\n,\n"] {
        let t = Table::parse(text, Format { delim: if text.contains(';') { b';' } else { b',' }, quote: b'"' });
        // Sätt varje cell till sitt eget värde: ingenting får ändras.
        let mut changes = Vec::new();
        for r in 0..t.rows() {
            for c in 0..t.row_len(r) {
                changes.push((r, c, t.value(text, r, c).into_owned()));
            }
        }
        let p = t.set_values(text, &changes);
        assert_eq!(apply(text, &p), text, "{text:?}");
    }
}

#[test]
fn encode_and_unquote() {
    let f = Format::COMMA;
    assert_eq!(model::encode_cell("plain", f, false), "plain");
    assert_eq!(model::encode_cell("a,b", f, false), "\"a,b\"");
    assert_eq!(model::encode_cell("say \"x\"", f, false), "\"say \"\"x\"\"\"");
    assert_eq!(model::encode_cell("a\nb", f, false), "\"a\nb\"");
    assert_eq!(model::encode_cell("x", f, true), "\"x\"");
    assert_eq!(model::unquote("\"a\"\"b\"", b'"'), "a\"b");
    assert_eq!(model::unquote("abc", b'"'), "abc");
}

// ---------------------------------------------------------------- redigering

#[test]
fn edit_keeps_other_rows_identical() {
    let text = "name,qty\n\"Smith, J\",1\nlast,3\n";
    let t = parse(text);
    let p = t.set_values(text, &[(2, 1, "30".into())]);
    assert_eq!(apply(text, &p), "name,qty\n\"Smith, J\",1\nlast,30\n");
    // Bara en rad ändras.
    assert_eq!(p.edits.len(), 1);
}

#[test]
fn edit_preserves_quoting_style_and_crlf() {
    let text = "\"a\",\"b\"\r\n\"c\",d\r\n";
    let t = parse(text);
    // Citerad cell förblir citerad, ociterad citeras bara vid behov.
    let p = t.set_values(text, &[(0, 0, "x".into()), (1, 1, "y,z".into())]);
    assert_eq!(apply(text, &p), "\"x\",\"b\"\r\n\"c\",\"y,z\"\r\n");
}

#[test]
fn edit_keeps_missing_trailing_newline() {
    let text = "a,b\nc,d";
    let t = parse(text);
    let p = t.set_values(text, &[(1, 0, "z".into())]);
    assert_eq!(apply(text, &p), "a,b\nz,d");
}

#[test]
fn set_value_beyond_table_extends_it() {
    let text = "a,b\nc,d\n";
    let t = parse(text);
    let p = t.set_values(text, &[(3, 1, "x".into()), (0, 3, "y".into())]);
    assert_eq!(apply(text, &p), "a,b,,y\nc,d\n,\n,x\n");
    let text = "a,b\nc,d";
    let p = parse(text).set_values(text, &[(2, 0, "x".into())]);
    assert_eq!(apply(text, &p), "a,b\nc,d\nx,");
}

#[test]
fn insert_and_delete_rows() {
    let text = "a,b\nc,d\ne,f\n";
    let t = parse(text);
    assert_eq!(apply(text, &t.insert_rows(text, 1, 2)), "a,b\n,\n,\nc,d\ne,f\n");
    assert_eq!(apply(text, &t.insert_rows(text, 3, 1)), "a,b\nc,d\ne,f\n,\n");
    assert_eq!(apply(text, &t.delete_rows(text, &[0, 2])), "c,d\n");
    assert_eq!(apply(text, &t.delete_rows(text, &[1])), "a,b\ne,f\n");
    // Utan avslutande radbrytning gäller det även efteråt.
    let text = "a,b\nc,d\ne,f";
    let t = parse(text);
    assert_eq!(apply(text, &t.delete_rows(text, &[2])), "a,b\nc,d");
    assert_eq!(apply(text, &t.delete_rows(text, &[1, 2])), "a,b");
    assert_eq!(apply(text, &t.insert_rows(text, 3, 1)), "a,b\nc,d\ne,f\n,");
    assert_eq!(apply(text, &t.delete_rows(text, &[0, 1, 2])), "");
}

#[test]
fn insert_and_delete_columns() {
    let text = "a,b,c\n\"d,1\",e,f\n";
    let t = parse(text);
    assert_eq!(apply(text, &t.insert_cols(text, 1, 1)), "a,,b,c\n\"d,1\",,e,f\n");
    assert_eq!(apply(text, &t.insert_cols(text, 3, 1)), "a,b,c,\n\"d,1\",e,f,\n");
    assert_eq!(apply(text, &t.delete_cols(text, &[0])), "b,c\ne,f\n");
    assert_eq!(apply(text, &t.delete_cols(text, &[1, 2])), "a\n\"d,1\"\n");
}

#[test]
fn tsv_clipboard_round_trip() {
    let rows = s(&[&["a", "b\tc"], &["multi\nline", "q\"x"]]);
    let tsv = model::to_tsv(&rows);
    assert_eq!(model::from_tsv(&tsv), rows);
    assert_eq!(model::from_tsv("1\t2\n3\t4\n"), s(&[&["1", "2"], &["3", "4"]]));
    assert_eq!(model::from_tsv("x"), s(&[&["x"]]));
    assert_eq!(model::from_tsv("1\t2\r\n3\t4"), s(&[&["1", "2"], &["3", "4"]]));
}

// ---------------------------------------------------------------- sortering

#[test]
fn sort_numeric_and_keeps_header() {
    let text = "n,v\nb,10\na,2\nc,\nd,33\n";
    let t = parse(text);
    let p = t.sort_rows(text, 1, false, SortMode::Auto, 1).expect("ändras");
    assert_eq!(apply(text, &p), "n,v\na,2\nb,10\nd,33\nc,\n");
    let p = t.sort_rows(text, 1, true, SortMode::Auto, 1).expect("ändras");
    // Tomma sist även fallande.
    assert_eq!(apply(text, &p), "n,v\nd,33\nb,10\na,2\nc,\n");
}

#[test]
fn sort_text_swedish_and_natural() {
    let text = "x\nöl\nzebra\nåsa\nApa\närta\nfil10\nfil2\n";
    let t = parse(text);
    let p = t.sort_rows(text, 0, false, SortMode::Auto, 0).expect("ändras");
    assert_eq!(apply(text, &p), "Apa\nfil2\nfil10\nx\nzebra\nåsa\närta\nöl\n");
    let p = t.sort_rows(text, 0, false, SortMode::Text, 0).expect("ändras");
    assert!(apply(text, &p).starts_with("Apa\nfil10\nfil2\n"));
}

#[test]
fn sort_unchanged_gives_none_and_keeps_missing_newline() {
    let text = "1\n2\n3\n";
    assert!(parse(text).sort_rows(text, 0, false, SortMode::Auto, 0).is_none());
    let text = "b,1\na,2";
    let t = parse(text);
    let p = t.sort_rows(text, 0, false, SortMode::Auto, 0).expect("ändras");
    assert_eq!(apply(text, &p), "a,2\nb,1");
}

#[test]
fn sort_keeps_crlf_and_rows_byte_identical() {
    let text = "k,v\r\n\"b\nb\",2\r\na,1\r\n";
    let t = parse(text);
    let p = t.sort_rows(text, 0, false, SortMode::Auto, 1).expect("ändras");
    assert_eq!(apply(text, &p), "k,v\r\na,1\r\n\"b\nb\",2\r\n");
}

#[test]
fn collation_orders_swedish_letters_last() {
    use std::cmp::Ordering::*;
    assert_eq!(collate::compare("z", "å", false), Less);
    assert_eq!(collate::compare("å", "ä", false), Less);
    assert_eq!(collate::compare("ä", "ö", false), Less);
    assert_eq!(collate::compare("apa", "Banan", false), Less);
    assert_eq!(collate::compare("fil2", "fil10", true), Less);
    assert_eq!(collate::compare("fil2", "fil10", false), Greater);
    assert_eq!(collate::compare("e", "é", false), Less);
}

// ---------------------------------------------------------------- tal och statistik

#[test]
fn parses_numbers_in_both_styles() {
    assert_eq!(parse_number("1 234,5"), Some(1234.5));
    assert_eq!(parse_number("1\u{a0}234,5"), Some(1234.5));
    assert_eq!(parse_number("1,234.5"), Some(1234.5));
    assert_eq!(parse_number("1.234,5"), Some(1234.5));
    assert_eq!(parse_number("-12,5"), Some(-12.5));
    assert_eq!(parse_number("\u{2212}3"), Some(-3.0));
    assert_eq!(parse_number("42"), Some(42.0));
    assert_eq!(parse_number("3.14"), Some(3.14));
    assert_eq!(parse_number("1,5e3"), Some(1500.0));
    assert_eq!(parse_number(",5"), Some(0.5));
    assert_eq!(parse_number("1 234 567"), Some(1_234_567.0));
    assert_eq!(parse_number("1,234,567"), Some(1_234_567.0));
    for bad in ["", "abc", "1,2,3x", "12 34", "1.2.3,4.5", "--1", "1e", "e5", "1,,2"] {
        assert_eq!(parse_number(bad), None, "{bad:?}");
    }
}

#[test]
fn stats_over_values() {
    let st = stats::compute(["10", "1 234,5", "", "text", "-4,5"]);
    assert_eq!(st.count, 4);
    assert_eq!(st.numeric, 3);
    assert!((st.sum - 1240.0).abs() < 1e-9);
    assert_eq!(st.min, -4.5);
    assert_eq!(st.max, 1234.5);
    assert!((st.average().expect("medel") - 1240.0 / 3.0).abs() < 1e-9);
    assert_eq!(stats::compute(["a", "b"]).average(), None);
}

#[test]
fn number_formatting() {
    crate::i18n::set_lang(crate::i18n::Lang::Sv);
    assert_eq!(stats::format_number(1234.5), "1\u{a0}234,5");
    assert_eq!(stats::format_number(-3.0), "-3");
    assert_eq!(stats::format_number(0.1 + 0.2), "0,3");
    crate::i18n::set_lang(crate::i18n::Lang::En);
    assert_eq!(stats::format_number(1234567.25), "1,234,567.25");
    crate::i18n::set_lang(crate::i18n::Lang::Sv);
}

// ---------------------------------------------------------------- avgränsare

#[test]
fn sniffs_delimiters() {
    let d = |t: &str, ext: &str| sniff::sniff(t, ext).map(|s| s.fmt.delim);
    assert_eq!(d("a,b,c\n1,2,3\n4,5,6\n", "csv"), Some(b','));
    assert_eq!(d("a;b;c\n1;2;3\n4;5;6\n", "csv"), Some(b';'));
    assert_eq!(d("a\tb\tc\n1\t2\t3\n4\t5\t6\n", "txt"), Some(b'\t'));
    assert_eq!(d("a|b\n1|2\n3|4\n", "txt"), Some(b'|'));
    // Decimalkomma med semikolon: kommatecknen är inte avgränsare.
    assert_eq!(d("namn;pris\nstol;1,5\nbord;12,25\nlampa;3,75\n", "csv"), Some(b';'));
    // Citerade kommatecken räknas inte.
    assert_eq!(d("a;b\n\"x,y\";1\n\"p,q\";2\n", "csv"), Some(b';'));
}

#[test]
fn sniff_rejects_prose_but_accepts_table_extensions() {
    let prose = "Hej, det här är en text.\nDen har ett kommatecken ibland, men inte alltid.\nTredje raden saknar det.\n";
    assert_eq!(sniff::sniff(prose, "txt"), None);
    assert_eq!(sniff::sniff("a,b\n1,2\n", "txt"), None, "för få rader utanför csv");
    // En kolumn i en csv-fil visas ändå som tabell.
    assert!(sniff::sniff("a\nb\nc\n", "csv").is_some());
    assert_eq!(sniff::sniff("a\tb\n", "tsv").map(|s| s.fmt.delim), Some(b'\t'));
}

#[test]
fn sniffs_quote_and_header() {
    let s = sniff::sniff("'a','b'\n'1','2'\n'3','4'\n'5','6'\n", "csv").expect("tabell");
    assert_eq!(s.fmt.quote, b'\'');
    let s = sniff::sniff("namn,ålder\nAnna,30\nBo,41\n", "csv").expect("tabell");
    assert!(s.header);
    let s = sniff::sniff("1,2\n3,4\n5,6\n", "csv").expect("tabell");
    assert!(!s.header);
    let s = sniff::sniff("a,b\nc,d\ne,f\n", "csv").expect("tabell");
    assert!(s.header, "olika textrubriker räknas som rubrik");
    assert_eq!(s.cols, 2);
}

#[test]
fn tab_semicolon_quote_survive_edit_round_trip() {
    let text = "a;b\n\"x;y\";2\n";
    let fmt = Format { delim: b';', quote: b'"' };
    let t = Table::parse(text, fmt);
    let p = t.set_values(text, &[(1, 1, "3".into()), (0, 0, "p;q".into())]);
    assert_eq!(apply(text, &p), "\"p;q\";b\n\"x;y\";3\n");
}
