//! Förankrade textrutor i dokumentläget: markera vid kanten, flytta med musen och ändra storlek med handtag.
//!
//! Att klicka inne i rutan flyttar bara textmarkören (texten i rutan är vanlig text); kanten (några pixlar
//! runt rutan) är en egen träffyta. Ändringen skrivs som nytt mått och läge i öppningsraden
//! `<!--k:tb N w= h= x= y= …-->` och i rutans ram (`k:pp fr=`), och går till .docx/.odt vid sparning. Nytt läge
//! skrivs alltid som avstånd från spaltens vänsterkant och styckets överkant (`hrel` kolumn, `vrel` stycke).

use std::ops::Range;

use egui::{CursorIcon, Id, LayerId, Order, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use super::anchor::{Anchor, HRel, Pos, VRel};
use super::images::{Handle, hit_handle, paint_selection, resize, resized_rect};
use super::pbox::{FrameX, PPr};
use crate::docx_tbox::{self, Head};
use crate::theme::Palette;

/// Träffytans bredd runt kanten (skärmpx).
const HIT: f32 = 6.0;
/// Minsta mått (CSS-px).
const MIN_W: f32 = 48.0;
const MIN_H: f32 = 24.0;

/// En ritad ruta: skärmrutan och det som behövs för att räkna om ett nytt läge.
#[derive(Clone, Debug)]
pub struct BoxView {
    /// Öppningsradens byteområde (utan `\n`).
    pub open: Range<usize>,
    pub head: Head,
    pub rect: Rect,
    /// Överkanten på stycket rutan är förankrad i och spaltens vänsterkant (skärmpx).
    pub para_top: f32,
    pub col_left: f32,
    /// Spaltens bredd och papperet utanför den (vänster, höger; marginalerna), skärmpx: rutan får dras ut i
    /// marginalen men inte förbi papperets kant.
    pub col_w: f32,
    pub room: (f32, f32),
}

/// Ny storlek/läge för en ruta: det som skrivs i öppningsraden och ramarna.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub open: Range<usize>,
    pub head: Head,
    /// Nytt vågrätt läge (pt från spaltens vänsterkant) för ramarna om läget ändrats.
    pub x_pt: Option<f32>,
}

#[derive(Default)]
pub struct State {
    pub doc_id: u64,
    /// Vald ruta (öppningsradens start).
    pub selected: Option<usize>,
    drag: Option<Drag>,
    mv: Option<Moving>,
}

struct Drag {
    key: usize,
    handle: Handle,
    orig: Rect,
    origin: egui::Pos2,
    last: Vec2,
}

struct Moving {
    key: usize,
    grab: Vec2,
    orig: Rect,
}

impl State {
    pub fn busy(&self) -> bool {
        self.drag.is_some() || self.mv.is_some()
    }

    pub fn deselect(&mut self) {
        self.selected = None;
        self.drag = None;
        self.mv = None;
    }
}

/// Hela ändringen för en ruta som hamnar på `new` (skärmpx, zoom `z`). Oförändrad övre vänster hörn behåller
/// ankaret (justering mot kanten bevaras); annars blir det ett avstånd från spalten och stycket.
pub fn change_for(b: &BoxView, new: Rect, z: f32) -> Change {
    let moved = (new.min - b.rect.min).length() > 0.5;
    let mut head = b.head;
    head.w = (new.width() / z).max(MIN_W);
    head.h = (new.height() / z).max(MIN_H);
    let mut x_pt = None;
    if moved {
        let (lo, hi) = (-b.room.0, (b.col_w + b.room.1 - new.width()).max(-b.room.0));
        let x = (new.left() - b.col_left).clamp(lo, hi) / z;
        let y = ((new.top() - b.para_top) / z).max(0.0);
        let mut a: Anchor = head.anchor.unwrap_or_default();
        a.hrel = HRel::Column;
        a.h = Pos::Off(x);
        a.vrel = VRel::Paragraph;
        a.v = Pos::Off(y);
        head.anchor = Some(a);
        x_pt = Some(x * 0.75);
    }
    Change { open: b.open.clone(), head, x_pt }
}

/// Skriver ändringen i texten: öppningsraden och `fr=` i alla stycken i rutan. `None` om blocket inte hittas.
pub fn apply(text: &str, c: &Change) -> Option<String> {
    let line = text.get(c.open.clone())?;
    let (n, _) = docx_tbox::parse_head(line)?;
    let close = text[c.open.end..].find(docx_tbox::CLOSE)? + c.open.end;
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str(&text[..c.open.start]);
    out.push_str(&docx_tbox::open_tag_head(n, &c.head));
    let inner = &text[c.open.end..close];
    let mut rest = inner;
    let key = super::pbox::MARK.trim_end();
    while let Some(i) = rest.find(key) {
        out.push_str(&rest[..i]);
        let Some(e) = rest[i..].find("-->").map(|e| i + e + 3) else { break };
        let tag = &rest[i..e];
        match PPr::parse_marker(tag) {
            Some(mut pp) if pp.frame.is_some() => {
                if let Some(f) = pp.frame.as_mut() {
                    f.w = c.head.w * 0.75;
                    f.h = c.head.h * 0.75;
                    if let Some(x) = c.x_pt {
                        f.x = FrameX::Off(x);
                    }
                }
                out.push_str(&pp.marker());
            }
            _ => out.push_str(tag),
        }
        rest = &rest[e..];
    }
    out.push_str(rest);
    out.push_str(&text[close..]);
    Some(out)
}

/// Rutorna som är förankrade (`frames` från `pboxui::shapes_rects`) med det som behövs för att dra dem.
pub fn views(
    text: &str,
    galley: &egui::Galley,
    gpos: egui::Pos2,
    frames: &[(Range<usize>, Rect)],
    col_at: &dyn Fn(f32, f32) -> (f32, f32),
    room_at: &dyn Fn(f32) -> (f32, f32),
) -> Vec<BoxView> {
    let mut out = Vec::new();
    for (range, rect) in frames {
        let before = text[..range.start].trim_end();
        let ls = before.rfind('\n').map_or(0, |i| i + 1);
        let line = &before[ls..];
        let Some((_, head)) = docx_tbox::parse_head(line).filter(|(_, h)| h.anchor.is_some()) else { continue };
        let Some(close) = text[range.end..].find(docx_tbox::CLOSE).map(|i| i + range.end) else { continue };
        let ci = text[..close].chars().count();
        let para_top = galley.pos_from_cursor(egui::text::CCursor::new(ci)).top() + gpos.y;
        let (cl, col_w) = col_at(rect.left() - gpos.x, rect.top() - gpos.y);
        let room = room_at(rect.top() - gpos.y);
        out.push(BoxView { open: ls..ls + line.len(), head, rect: *rect, para_top, col_left: gpos.x + cl, col_w, room });
    }
    out
}

fn strips(r: Rect) -> [Rect; 4] {
    let e = r.expand(HIT);
    [
        Rect::from_min_max(e.min, pos2(e.max.x, r.min.y + HIT)),
        Rect::from_min_max(pos2(e.min.x, r.max.y - HIT), e.max),
        Rect::from_min_max(pos2(e.min.x, r.min.y + HIT), pos2(r.min.x + HIT, r.max.y - HIT)),
        Rect::from_min_max(pos2(r.max.x - HIT, r.min.y + HIT), pos2(e.max.x, r.max.y - HIT)),
    ]
}

/// Klick, dragning och handtag för de ritade rutorna. Ger en ändring när en flytt eller storleksändring släpps.
pub fn ui(ui: &mut Ui, st: &mut State, doc_id: u64, boxes: &[BoxView], z: f32, max_w: f32, editor_id: Id, p: &Palette) -> Option<Change> {
    if st.doc_id != doc_id {
        st.deselect();
    }
    st.doc_id = doc_id;
    let ctx = ui.ctx().clone();
    let mut act = None;

    for b in boxes {
        let key = b.open.start;
        let selected = st.selected == Some(key);
        for (k, s) in strips(b.rect).into_iter().enumerate() {
            let r = ui.interact(s, editor_id.with(("kladd-tbox", key, k)), Sense::click_and_drag());
            let near = ctx.pointer_hover_pos().filter(|q| s.contains(*q));
            if near.is_some() && st.drag.is_none() && st.mv.is_none() {
                match near.and_then(|q| selected.then(|| hit_handle(b.rect, q)).flatten()) {
                    Some(h) => ctx.set_cursor_icon(h.cursor()),
                    None => ctx.set_cursor_icon(CursorIcon::Grab),
                }
            }
            if (r.clicked() || r.is_pointer_button_down_on() || r.drag_started()) && !selected && !st.busy() {
                st.selected = Some(key);
                ctx.memory_mut(|m| m.surrender_focus(editor_id));
            }
            if r.drag_started() && st.selected == Some(key) && !st.busy() {
                let origin = ctx.input(|i| i.pointer.press_origin()).unwrap_or(b.rect.center());
                match hit_handle(b.rect, origin) {
                    Some(h) => st.drag = Some(Drag { key, handle: h, orig: b.rect, origin, last: b.rect.size() }),
                    None => st.mv = Some(Moving { key, grab: origin - b.rect.min, orig: b.rect }),
                }
            }
        }
    }

    // Storleksändring: förhandsruta; vid släpp en ändring.
    if let Some(d) = &mut st.drag {
        let pos = ctx.pointer_latest_pos().unwrap_or(d.orig.center());
        let down = ctx.input(|i| i.pointer.primary_down());
        if down {
            d.last = resize(d.handle, d.orig.size(), pos - d.origin, false, max_w).max(vec2(MIN_W, MIN_H) * z);
        }
        let rect = resized_rect(d.orig, d.handle, d.last);
        ctx.set_cursor_icon(d.handle.cursor());
        let pt = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("kladd-tbox-preview")));
        pt.rect_stroke(rect, 0.0, Stroke::new(1.5, p.accent), egui::StrokeKind::Middle);
        if !down {
            if let Some(b) = boxes.iter().find(|b| b.open.start == d.key) {
                act = Some(change_for(b, rect, z));
            }
        }
        ctx.request_repaint();
        if !down {
            st.drag = None;
        }
    }

    // Flytt: spökruta; vid släpp ett nytt läge.
    if let Some(m) = st.mv.take() {
        let pos = ctx.pointer_latest_pos().unwrap_or_default();
        let down = ctx.input(|i| i.pointer.primary_down());
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if !esc {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            let rect = Rect::from_min_size(pos - m.grab, m.orig.size());
            let pt = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("kladd-tbox-move")));
            pt.rect_filled(rect, 0.0, p.accent.gamma_multiply(0.12));
            pt.rect_stroke(rect, 0.0, Stroke::new(1.5, p.accent), egui::StrokeKind::Middle);
            if down {
                st.mv = Some(m);
            } else if let Some(b) = boxes.iter().find(|b| b.open.start == m.key) {
                if (rect.min - b.rect.min).length() > 1.0 {
                    act = Some(change_for(b, rect, z));
                }
            }
        }
        ctx.request_repaint();
    }

    if let Some(b) = st.selected.and_then(|k| boxes.iter().find(|b| b.open.start == k)) {
        paint_selection(ui, b.rect, p);
    }
    // Tryck utanför rutan avmarkerar.
    if ctx.input(|i| i.pointer.primary_pressed()) && !st.busy() {
        let on = ctx.pointer_interact_pos().is_some_and(|q| boxes.iter().any(|b| st.selected == Some(b.open.start) && b.rect.expand(HIT).contains(q)));
        if !on {
            st.selected = None;
        }
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    const TXT: &str = "A\n\n<!--k:tb 3 w=\"150\" h=\"90\" x=\"right\" y=\"6.3\"-->\n\nHej<!--k:pp fr=\"w=112.5 x=end pl=4 pr=4 h=67.5\"-->\n\n<!--k:/tb-->\n\nSlut\n";

    fn view() -> BoxView {
        let open = TXT.find("<!--k:tb").unwrap();
        let open = open..TXT[open..].find("-->").unwrap() + open + 3;
        let head = docx_tbox::parse_head(&TXT[open.clone()]).unwrap().1;
        BoxView { open, head, rect: Rect::from_min_size(pos2(500.0, 200.0), vec2(150.0, 90.0)), para_top: 194.0, col_left: 100.0, col_w: 600.0, room: (50.0, 80.0) }
    }

    #[test]
    fn resize_only_keeps_anchor() {
        let v = view();
        let c = change_for(&v, Rect::from_min_size(v.rect.min, vec2(200.0, 100.0)), 1.0);
        assert_eq!((c.head.w, c.head.h), (200.0, 100.0));
        assert_eq!(c.head.anchor, v.head.anchor);
        assert_eq!(c.x_pt, None);
        let out = apply(TXT, &c).unwrap();
        assert!(out.contains("w=\"200\" h=\"100\" x=\"right\""), "{out}");
        assert!(out.contains("fr=\"w=150 x=end pl=4 pr=4 h=75\""), "{out}");
        assert!(out.ends_with("<!--k:/tb-->\n\nSlut\n"));
    }

    #[test]
    fn move_into_margin_keeps_position_but_not_past_the_paper() {
        let v = view();
        // Spalten 100..700 px (zoom 1), marginalerna 50 vänster och 80 höger: rutan (150 bred) kan stå ut till x=730.
        let c = change_for(&v, v.rect.translate(vec2(150.0, 0.0)), 1.0);
        assert_eq!(c.head.anchor.unwrap().h, Pos::Off(530.0));
        let c = change_for(&v, v.rect.translate(vec2(-470.0, 0.0)), 1.0);
        assert_eq!(c.head.anchor.unwrap().h, Pos::Off(-50.0));
        // Delvis i marginalen: skrivs som det står.
        let c = change_for(&v, Rect::from_min_size(pos2(100.0 + 520.0, 200.0), vec2(150.0, 90.0)), 1.0);
        assert_eq!(c.head.anchor.unwrap().h, Pos::Off(520.0));
        assert_eq!(c.x_pt, Some(390.0));
    }

    #[test]
    fn move_gives_column_and_paragraph_offsets() {
        let v = view();
        let c = change_for(&v, v.rect.translate(vec2(-50.0, 30.0)), 2.0);
        let a = c.head.anchor.unwrap();
        assert_eq!((a.hrel, a.vrel), (HRel::Column, VRel::Paragraph));
        // (450 - 100) / 2 = 175 px från spaltens vänsterkant; (230 - 194) / 2 = 18 px under stycket.
        assert_eq!((a.h, a.v), (Pos::Off(175.0), Pos::Off(18.0)));
        assert_eq!(c.x_pt, Some(131.25));
        let out = apply(TXT, &c).unwrap();
        assert!(out.contains("x=\"175\" y=\"18\""), "{out}");
        assert!(out.contains("x=131.25"), "{out}");
        // Läses tillbaka som samma huvud.
        let line = out.lines().find(|l| l.starts_with("<!--k:tb")).unwrap();
        assert_eq!(docx_tbox::parse_head(line).unwrap().1, c.head);
    }
}
