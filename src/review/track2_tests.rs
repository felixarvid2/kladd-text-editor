//! Tester för spårade stycketecken, radbrytningar och formateringsändringar.

use super::*;
use crate::word::markup::{self, Align, Block, Inline};

const AU: &str = "Ada";
const DT: &str = "2026-05-01T10:00:00Z";

fn rec(id: u32, old: Option<&str>) -> String {
    rev_line(&RevRec { id, author: AU.into(), date: DT.into(), old: old.map(str::to_owned) })
}

#[test]
fn scan_finds_paragraph_marks_and_fmt_spans() {
    let t = format!("Hej<!--k:pm ins 3 n=2-->\n\nDär <fmt data-k=\"4\">fet</fmt><!--k:pm del 5-->\n\nTitel<!--k:pm fmt 6-->\n\n{}\n{}\n", rec(3, None), rec(6, Some("normal|left")));
    let sc = scan(&t);
    assert_eq!(sc.pmarks.len(), 3);
    assert_eq!((sc.pmarks[0].kind, sc.pmarks[0].id, sc.pmarks[0].n), (RevKind::Ins, 3, Some(2)));
    assert_eq!((sc.pmarks[1].kind, sc.pmarks[1].id, sc.pmarks[1].n), (RevKind::Del, 5, None));
    assert_eq!((sc.pmarks[2].kind, sc.pmarks[2].id), (RevKind::Fmt, 6));
    assert_eq!(&t[sc.pmarks[0].range.clone()], "<!--k:pm ins 3 n=2-->");
    assert_eq!(sc.revs.len(), 1);
    assert_eq!(sc.revs[0].kind, RevKind::Fmt);
    assert_eq!(sc.rev_rec(6).and_then(|r| r.old.as_deref()), Some("normal|left"));
    assert_eq!(change_count(&t), 4);
    // Markörer och poster i kodstaket räknas inte.
    assert!(scan("```\na<!--k:pm ins 1-->\n```\n").pmarks.is_empty());
}

#[test]
fn enter_in_the_middle_of_a_line_is_tracked_and_can_be_rejected() {
    let old = "Hej där\n";
    let t = track_edit(old, "Hej\n där\n", 3..3, AU, DT).unwrap();
    assert!(t.text.starts_with("Hej<!--k:pm ins 1 n=1-->\n där\n"), "{}", t.text);
    assert!(t.text[t.caret..].starts_with(" där"), "markören står efter brytningen");
    assert!(t.text.contains("<!--k:r id=\"1\" author=\"Ada\""), "{}", t.text);
    assert_eq!(reject_all(&t.text), old);
    assert_eq!(accept_all(&t.text), "Hej\n där\n");
}

#[test]
fn enter_at_the_end_of_a_paragraph_keeps_the_original_gap_on_reject() {
    let old = "AB\n\nCD\n";
    let t = track_edit(old, "AB\n\n\nCD\n", 2..2, AU, DT).unwrap();
    assert!(t.text.starts_with("AB<!--k:pm ins 1 n=1-->\n\n\nCD"), "{}", t.text);
    assert_eq!(reject_all(&t.text), old);
    assert_eq!(accept_all(&t.text), "AB\n\n\nCD\n");
}

#[test]
fn second_enter_counts_up_the_same_mark() {
    let t = track_edit("Hej där\n", "Hej\n där\n", 3..3, AU, DT).unwrap();
    let t2 = track_edit(&t.text, &format!("{}\n{}", &t.text[..t.caret], &t.text[t.caret..]), t.caret..t.caret, AU, DT).unwrap();
    assert!(t2.text.starts_with("Hej<!--k:pm ins 1 n=2-->\n\n där"), "{}", t2.text);
    assert_eq!(reject_all(&t2.text), "Hej där\n");
}

#[test]
fn backspace_right_after_enter_removes_the_break_for_real() {
    let old = "Hej där\n";
    let t = track_edit(old, "Hej\n där\n", 3..3, AU, DT).unwrap();
    let without = format!("{}{}", &t.text[..t.caret - 1], &t.text[t.caret..]);
    let t2 = track_edit(&t.text, &without, t.caret..t.caret, AU, DT).unwrap();
    assert_eq!(t2.text, old);
    assert_eq!(&t2.text[t2.caret..], " där\n");
}

#[test]
fn enter_inside_an_own_insertion_splits_it() {
    let old = format!("<ins data-k=\"1\">hej alla</ins>\n\n{}\n", rec(1, None));
    let at = old.find("alla").unwrap();
    let new = format!("{}\n{}", &old[..at], &old[at..]);
    let t = track_edit(&old, &new, at..at, AU, DT).unwrap();
    assert!(t.text.starts_with("<ins data-k=\"1\">hej </ins><!--k:pm ins 2 n=1-->\n<ins data-k=\"1\">alla</ins>"), "{}", t.text);
    assert!(reject_all(&t.text).trim().is_empty());
    assert_eq!(accept_all(&t.text), "hej \nalla\n");
}

#[test]
fn backspace_over_a_paragraph_break_marks_it_deleted() {
    let old = "AB\n\nCD\n";
    // Backspace med markören före "C".
    let t = track_edit(old, "AB\nCD\n", 4..4, AU, DT).unwrap();
    assert!(t.text.starts_with("AB<!--k:pm del 1-->\n\nCD\n"), "{}", t.text);
    assert_eq!(&t.text[..t.caret], "AB", "markören före det raderade stycketecknet");
    assert_eq!(accept_all(&t.text), "ABCD\n");
    assert_eq!(reject_all(&t.text), old);
    // Delete med markören efter "B".
    let d = track_edit(old, "AB\nCD\n", 2..2, AU, DT).unwrap();
    assert!(d.text.starts_with("AB<!--k:pm del 1-->\n\nCD"), "{}", d.text);
    assert!(d.text[d.caret..].starts_with("CD"), "markören efter brytningen: {}", &d.text[d.caret..]);
    // Ett andra Backspace över samma brytning lägger inte till något.
    let at = t.text.find("CD").unwrap();
    let again = track_edit(&t.text, &t.text.replacen("\n\n", "\n", 1), at..at, AU, DT).unwrap();
    assert_eq!(again.text, t.text);
    assert_eq!(&again.text[..again.caret], "AB");
}

#[test]
fn accepted_deleted_mark_joins_with_the_next_line_without_its_prefix() {
    assert_eq!(accept_all(&format!("# Rubrik<!--k:pm del 1-->\n\nText\n\n{}\n", rec(1, None))), "# RubrikText\n");
    assert_eq!(accept_all(&format!("- a<!--k:pm del 1-->\n- b\n\n{}\n", rec(1, None))), "- ab\n");
    // En tabell efter stycket går inte att slå ihop: bara markören försvinner.
    let t = format!("Före<!--k:pm del 1-->\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n{}\n", rec(1, None));
    assert!(accept_all(&t).starts_with("Före\n\n| a | b |"), "{}", accept_all(&t));
}

#[test]
fn inserted_mark_from_word_rejects_to_one_paragraph() {
    let t = format!("Första<!--k:pm ins 1-->\n\nAndra\n\n{}\n", rec(1, None));
    assert_eq!(reject_all(&t), "FörstaAndra\n");
    assert_eq!(accept_all(&t), "Första\n\nAndra\n");
    // Per ändring: markören intill markören.
    let at = t.find("<!--k:pm").unwrap();
    let e = resolve_range(&t, at..at, false).unwrap();
    assert_eq!(e.text, "FörstaAndra\n");
}

#[test]
fn pasted_lines_get_a_mark_per_break() {
    let old = "ab\n";
    let t = track_edit(old, "aXY\nZW\nb\n", 1..1, AU, DT);
    // Klistrad text med radbrytningar: varje brytning markeras och allt kan avvisas.
    let t = t.unwrap();
    assert_eq!(t.text.matches("<!--k:pm ins").count(), 2, "{}", t.text);
    assert_eq!(reject_all(&t.text), old);
}

#[test]
fn selection_delete_over_a_break_marks_it() {
    let old = "ab\n\ncd\n";
    let t = track_edit(old, "ad\n", 1..5, AU, DT).unwrap();
    assert!(t.text.contains("<!--k:pm del"), "{}", t.text);
    assert_eq!(reject_all(&t.text), old);
    assert_eq!(accept_all(&t.text), "ad\n");
}

// ------------------------------------------------------------------ formatering

fn bold(t: &str, r: std::ops::Range<usize>) -> String {
    markup::toggle_inline(t, r, Inline::Bold).text
}

#[test]
fn char_formatting_is_tracked_with_the_old_format() {
    let old = "ett ord här\n";
    let new = bold(old, 4..7);
    assert_eq!(new, "ett **ord** här\n");
    let tg = fmt::track_format(old, &new, AU, DT).unwrap();
    assert!(tg.text.starts_with("ett **<fmt data-k=\"1\">ord</fmt>** här\n"), "{}", tg.text);
    assert_eq!(scan(&tg.text).rev_rec(1).and_then(|r| r.old.clone()).as_deref(), Some(";"));
    assert_eq!(accept_all(&tg.text), new);
    assert_eq!(reject_all(&tg.text), old);
    // Markeringen flyttas med taggarna.
    assert_eq!(tg.map(6, false), tg.text.find("ord").unwrap());
}

#[test]
fn unbolding_restores_bold_on_reject() {
    let old = "ett **ord** här\n";
    let new = bold(old, 6..9);
    assert_eq!(new, "ett ord här\n");
    let tg = fmt::track_format(old, &new, AU, DT).unwrap();
    assert_eq!(scan(&tg.text).rev_rec(1).and_then(|r| r.old.clone()).as_deref(), Some("b;"));
    assert_eq!(accept_all(&tg.text), new);
    assert_eq!(reject_all(&tg.text), old);
}

#[test]
fn span_properties_round_trip_through_reject() {
    let old = "röd text\n";
    let new = markup::set_span_prop(old, 0..4, markup::SpanProp::Color(Some([0xC0, 0, 0]))).text;
    let tg = fmt::track_format(old, &new, AU, DT).unwrap();
    assert!(tg.text.contains("<fmt data-k=\"1\">röd</fmt>"), "{}", tg.text);
    assert_eq!(accept_all(&tg.text), new);
    let rejected = reject_all(&tg.text);
    assert!(!rejected.contains("span") && rejected.contains("röd text"), "{rejected}");
}

#[test]
fn already_tracked_formatting_keeps_its_first_old_format() {
    let old = "ett ord här\n";
    let first = fmt::track_format(old, &bold(old, 4..7), AU, DT).unwrap();
    // Kursiv på samma ord: inget nytt spår (det gamla formatet gäller).
    let ital = markup::toggle_inline(&first.text, first.map(4, false)..first.map(7, true), Inline::Italic).text;
    let second = fmt::track_format(&first.text, &ital, AU, DT);
    assert!(second.is_none(), "{:?}", second.map(|s| s.text));
}

#[test]
fn typed_text_in_own_insertion_is_not_a_format_change() {
    let old = format!("<ins data-k=\"1\">nytt ord</ins>\n\n{}\n", rec(1, None));
    let at = old.find("ord").unwrap();
    let new = bold(&old, at..at + 3);
    assert!(fmt::track_format(&old, &new, AU, DT).is_none());
}

#[test]
fn paragraph_formatting_is_tracked_with_a_mark() {
    let old = "Rubrik\n\nText\n";
    let new = markup::set_block(old, 0..0, Block::Heading(2)).text;
    assert_eq!(new, "## Rubrik\n\nText\n");
    let tg = fmt::track_format(old, &new, AU, DT).unwrap();
    assert!(tg.text.starts_with("## Rubrik<!--k:pm fmt 1-->\n\nText\n"), "{}", tg.text);
    assert_eq!(scan(&tg.text).rev_rec(1).and_then(|r| r.old.clone()).as_deref(), Some("normal|left"));
    assert_eq!(accept_all(&tg.text), new);
    assert_eq!(reject_all(&tg.text), old);
    // Ett stycke med spårat styckeformat spåras inte igen.
    let again = markup::set_block(&tg.text, 0..0, Block::Quote).text;
    assert!(fmt::track_format(&tg.text, &again, AU, DT).is_none());
}

#[test]
fn alignment_change_is_tracked_and_rejected() {
    let old = "Mitt stycke\n";
    let new = markup::set_align(old, 0..0, Align::Center).text;
    assert!(new.contains("<div align=\"center\">"), "{new}");
    let tg = fmt::track_format(old, &new, AU, DT).unwrap();
    assert!(tg.text.contains("<!--k:pm fmt 1-->"), "{}", tg.text);
    assert_eq!(scan(&tg.text).rev_rec(1).and_then(|r| r.old.clone()).as_deref(), Some("normal|left"));
    let rejected = reject_all(&tg.text);
    assert!(!rejected.contains("div") && rejected.contains("Mitt stycke"), "{rejected}");
    assert!(accept_all(&tg.text).contains("<div align=\"center\">"));
}

#[test]
fn list_change_is_tracked_and_rejected() {
    let old = "Punkt\n";
    let new = markup::set_block(old, 0..0, Block::Bullet).text;
    let tg = fmt::track_format(old, &new, AU, DT).unwrap();
    assert!(tg.text.contains("- Punkt<!--k:pm fmt 1-->"), "{}", tg.text);
    assert_eq!(reject_all(&tg.text), old);
}

#[test]
fn format_descriptions_name_what_changed() {
    let plain = fmt::CharFmt::default();
    let b = fmt::CharFmt { bold: true, ..Default::default() };
    assert_eq!(fmt::describe_char(&plain, &b), crate::i18n::t("Fet"));
    assert_eq!(fmt::describe_char(&b, &plain), crate::i18n::t("Inte fet"));
    let p0 = fmt::ParaFmt { block: Block::Normal, align: Align::Left };
    let p1 = fmt::ParaFmt { block: Block::Heading(2), align: Align::Center };
    assert_eq!(fmt::describe_para(&p0, &p1), format!("{}, {}", crate::tf!("Rubrik {n}", n = 2), crate::i18n::t("Centrerad")));
    assert_eq!(fmt::CharFmt::decode(&b.encode()), b);
    assert_eq!(fmt::ParaFmt::decode(&p1.encode()), p1);
}

#[test]
fn untouched_text_without_review_marks_is_unchanged() {
    let t = "Rubrik\n\n- a\n- b\n\n**fet** text\n";
    assert_eq!(accept_all(t), t);
    assert_eq!(reject_all(t), t);
    assert!(fmt::track_format(t, t, AU, DT).is_none());
}
