//! Kommandopaletten (Ctrl+Skift+P): sök bland alla kommandon i registret (`commands.rs`).
//! Fuzzy-sökningen matchar etiketten på båda språken, senast använda kommandon kommer först.

use egui::{Align2, FontFamily, FontId, Frame, Id, Key, Margin, Modifiers, Order, RichText, ScrollArea, Sense, Stroke, TextEdit, pos2, vec2};

use crate::app::KladdApp;
use crate::app::commands::{self, Command, Env};
use crate::i18n::{self, Lang, t};
use crate::theme::Palette as Colors;

/// Så många senast använda kommandon sparas.
pub const MAX_RECENT: usize = 8;
const MAX_ROWS: usize = 12;
/// Avdrag för kommandon som inte går att köra nu: de sjunker men kan fortfarande hittas.
const UNAVAILABLE_PENALTY: i32 = 60;

/// Öppen palett.
pub struct Palette {
    query: String,
    selected: usize,
    focus: bool,
}

// -------------------------------------------------------------------------- fuzzy

/// Gemener, och å/ä/ö/é utan prickar så att "sok" hittar "Sök".
fn fold(c: char) -> char {
    match c {
        'å' | 'ä' | 'Å' | 'Ä' => 'a',
        'ö' | 'Ö' => 'o',
        'é' | 'É' => 'e',
        _ => c.to_lowercase().next().unwrap_or(c),
    }
}

/// Poäng för hur väl `query` passar `text` (högre = bättre), `None` om bokstäverna inte finns i
/// ordning. Blanksteg i frågan hoppas över. Konsekutiva tecken, ordstarter och början av texten
/// ger bonus; luckor ger avdrag. Hittar bästa placeringen (inte bara den första).
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(fold).collect();
    if q.is_empty() {
        return Some(0);
    }
    let raw: Vec<char> = text.chars().collect();
    let tx: Vec<char> = raw.iter().copied().map(fold).collect();
    if q.len() > tx.len() {
        return None;
    }
    let word_start = |j: usize| j == 0 || !raw[j - 1].is_alphanumeric();
    // dp[j]: bästa poäng när frågans hittills sista tecken står på textposition j.
    let mut prev: Vec<Option<i32>> = vec![None; tx.len()];
    for (i, qc) in q.iter().enumerate() {
        let mut cur: Vec<Option<i32>> = vec![None; tx.len()];
        for j in 0..tx.len() {
            if tx[j] != *qc {
                continue;
            }
            let mut here = 10;
            if word_start(j) {
                here += 12;
            }
            if j == 0 {
                here += 8;
            }
            let best_before = if i == 0 {
                Some(-(j as i32).min(12))
            } else {
                (0..j)
                    .filter_map(|k| {
                        prev[k].map(|s| {
                            if k + 1 == j { s + 15 } else { s - ((j - k - 1) as i32).min(8) }
                        })
                    })
                    .max()
            };
            cur[j] = best_before.map(|b| b + here);
        }
        prev = cur;
    }
    let mut best = prev.into_iter().flatten().max()?;
    // Hela texten eller dess början: träffar som liknar frågan mest ska ligga överst.
    let compact: String = tx.iter().collect();
    let qs: String = q.iter().collect();
    if compact == qs {
        best += 60;
    } else if compact.starts_with(&qs) {
        best += 30;
    } else if tx.len() > q.len() {
        best -= ((tx.len() - q.len()) as i32).min(20) / 4;
    }
    Some(best)
}

/// En träff i paletten.
pub struct Hit {
    pub cmd: &'static Command,
    pub score: i32,
    /// Träffade bara på det andra språkets etikett (visas som liten ledtråd).
    pub other_lang: bool,
}

/// Rangordnar `cmds` för frågan. Tom fråga: senast använda först (i den ordningen), sedan
/// registrets ordning. Annars bäst poäng först; senast använda får bonus och kommandon som inte
/// går att köra nu sjunker.
pub fn rank(query: &str, cmds: &'static [Command], recent: &[String], env: &Env, lang: Lang) -> Vec<Hit> {
    let other = if lang == Lang::Sv { Lang::En } else { Lang::Sv };
    let recency = |c: &Command| recent.iter().position(|r| r == c.id);
    let mut hits: Vec<(usize, Hit)> = Vec::new();
    for (order, cmd) in cmds.iter().enumerate() {
        let (mine, theirs) = (score(query, cmd.label_in(lang)), score(query, cmd.label_in(other)));
        let (mut s, other_lang) = match (mine, theirs) {
            (Some(a), Some(b)) if b > a => (b, true),
            (Some(a), _) => (a, false),
            (None, Some(b)) => (b, true),
            (None, None) => continue,
        };
        if !query.trim().is_empty() {
            if let Some(pos) = recency(cmd) {
                s += (MAX_RECENT - pos.min(MAX_RECENT)) as i32 * 3;
            }
            if !cmd.available(env) {
                s -= UNAVAILABLE_PENALTY;
            }
        }
        hits.push((order, Hit { cmd, score: s, other_lang }));
    }
    if query.trim().is_empty() {
        hits.sort_by_key(|(order, h)| (recency(h.cmd).unwrap_or(usize::MAX), *order));
    } else {
        hits.sort_by(|(oa, a), (ob, b)| b.score.cmp(&a.score).then(oa.cmp(ob)));
    }
    hits.into_iter().map(|(_, h)| h).collect()
}

// ---------------------------------------------------------------------------- UI

impl KladdApp {
    pub(crate) fn open_palette(&mut self) {
        // Ctrl+Skift+P igen stänger.
        self.cmd.palette = match self.cmd.palette {
            Some(_) => None,
            None => Some(Palette { query: String::new(), selected: 0, focus: true }),
        };
    }

    pub(crate) fn command_palette(&mut self, ctx: &egui::Context, p: &Colors) {
        let Some(mut pal) = self.cmd.palette.take() else { return };
        let lang = i18n::lang();
        let hits: Vec<Hit> = {
            let env = Env::of(self);
            let mut h = rank(&pal.query, commands::ALL, &self.settings.recent_commands, &env, lang);
            h.truncate(60);
            h
        };
        let avail: Vec<bool> = {
            let env = Env::of(self);
            hits.iter().map(|h| h.cmd.available(&env)).collect()
        };
        let checks: Vec<Option<bool>> = {
            let env = Env::of(self);
            hits.iter().map(|h| h.cmd.is_checked(&env)).collect()
        };
        if pal.selected >= hits.len() {
            pal.selected = hits.len().saturating_sub(1);
        }
        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if down && !hits.is_empty() {
            pal.selected = (pal.selected + 1) % hits.len();
        }
        if up && !hits.is_empty() {
            pal.selected = (pal.selected + hits.len() - 1) % hits.len();
        }
        let mut chosen: Option<usize> = if enter && avail.get(pal.selected).copied().unwrap_or(false) { Some(pal.selected) } else { None };
        let mut close = esc;
        let screen = ctx.content_rect();
        let width = 600.0f32.min(screen.width() - 32.0);
        let pos = pos2(screen.center().x - width / 2.0, screen.top() + 60.0);
        let max_h = (screen.height() - 160.0).clamp(120.0, MAX_ROWS as f32 * 30.0);
        let area = egui::Area::new(Id::unique("kladd-command-palette"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(11)
                    .inner_margin(Margin::same(8))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(width - 16.0);
                        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                        let field = ui.add(
                            TextEdit::singleline(&mut pal.query)
                                .hint_text(t("Skriv ett kommando"))
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(10, 7)),
                        );
                        if pal.focus {
                            field.request_focus();
                            pal.focus = false;
                        }
                        if field.changed() {
                            pal.selected = 0;
                        }
                        ui.add_space(4.0);
                        if hits.is_empty() {
                            ui.label(RichText::new(t("Inga träffar")).size(12.5).color(p.muted));
                        }
                        ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                            for (row, hit) in hits.iter().enumerate() {
                                let on = avail[row];
                                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                                if row == pal.selected {
                                    ui.painter().rect_filled(rect, 6.0, p.selection);
                                } else if resp.hovered() && on {
                                    ui.painter().rect_filled(rect, 6.0, p.surface_hover);
                                }
                                let color = if on { p.text } else { p.faint };
                                let font = FontId::new(13.5, FontFamily::Proportional);
                                let small = FontId::new(12.0, FontFamily::Proportional);
                                let mut x = rect.left() + 10.0;
                                if checks[row] == Some(true) {
                                    let c = pos2(x + 5.0, rect.center().y);
                                    let s = Stroke::new(1.5, p.accent);
                                    ui.painter().line_segment([c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0)], s);
                                    ui.painter().line_segment([c + vec2(-1.0, 3.0), c + vec2(4.5, -3.5)], s);
                                }
                                x += 18.0;
                                let label = hit.cmd.label();
                                let galley = ui.painter().layout_no_wrap(label.to_owned(), font, color);
                                let lw = galley.size().x;
                                ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
                                if hit.other_lang {
                                    let other = if lang == Lang::Sv { Lang::En } else { Lang::Sv };
                                    ui.painter().text(
                                        pos2(x + lw + 8.0, rect.center().y),
                                        Align2::LEFT_CENTER,
                                        hit.cmd.label_in(other),
                                        small.clone(),
                                        p.faint,
                                    );
                                }
                                // Kategori och kortkommando till höger.
                                let mut right = rect.right() - 10.0;
                                if !hit.cmd.shortcut.is_empty() {
                                    let sc = crate::keys::shortcut(hit.cmd.shortcut);
                                    let r = ui.painter().text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, sc, small.clone(), if on { p.muted } else { p.faint });
                                    right = r.left() - 14.0;
                                }
                                ui.painter().text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, hit.cmd.cat.label(), small, p.faint);
                                if resp.clicked() && on {
                                    chosen = Some(row);
                                }
                                if row == pal.selected && (down || up) {
                                    ui.scroll_to_rect(rect, None);
                                }
                            }
                        });
                    });
            });
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| !area.response.rect.contains(pos));
        if clicked_outside {
            close = true;
        }
        if let Some(row) = chosen {
            let cmd = hits[row].cmd;
            // Paletten stängs före kommandot, så att ett kommando som öppnar en dialog får fokus.
            self.focus_editor = true;
            self.run_command(ctx, cmd);
        } else if close {
            self.focus_editor = true;
        } else {
            self.cmd.palette = Some(pal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Settings;

    fn ids(h: &[Hit]) -> Vec<&'static str> {
        h.iter().map(|h| h.cmd.id).collect()
    }

    fn env(s: &Settings) -> Env<'_> {
        Env { has_doc: true, markdown: true, ..Env::empty(s) }
    }

    #[test]
    fn subsequence_is_required() {
        assert!(score("spra", "Spara").is_some());
        assert!(score("xyz", "Spara").is_none());
        assert!(score("aaaa", "Spara").is_none());
        assert_eq!(score("", "Spara"), Some(0));
        assert!(score("spara som här", "Spara").is_none());
    }

    #[test]
    fn folds_case_and_diacritics() {
        assert!(score("sok", "Sök och ersätt").is_some());
        assert!(score("SÖK", "sok").is_some());
        assert!(score("ersatt", "Sök och ersätt").is_some());
    }

    #[test]
    fn exact_beats_prefix_beats_scattered() {
        let exact = score("spara", "Spara").unwrap();
        let prefix = score("spara", "Spara som…").unwrap();
        let scattered = score("spara", "Sök och ersätt: pappersarbete").unwrap_or(i32::MIN);
        assert!(exact > prefix, "{exact} > {prefix}");
        assert!(prefix > scattered, "{prefix} > {scattered}");
    }

    #[test]
    fn word_starts_and_runs_beat_gaps() {
        // "sif" ska hellre träffa "Sök i filer" (ordstarter) än "Spara i fil" glest utspritt.
        let a = score("sif", "Sök i filer…").unwrap();
        let b = score("sif", "Skriv ut sidans information").unwrap();
        assert!(a > b, "{a} > {b}");
        assert!(score("fil", "Sök i filer…").unwrap() > score("fil", "Skriv ut: ferdig i lagret").unwrap_or(i32::MIN));
    }

    #[test]
    fn ranks_commands_and_matches_both_languages() {
        let s = Settings::default();
        let e = env(&s);
        let sv = rank("spara", commands::ALL, &[], &e, Lang::Sv);
        assert_eq!(sv[0].cmd.id, "file.save", "exakt träff först: {:?}", ids(&sv));
        assert!(ids(&sv).contains(&"file.save_as"));
        // Engelska ord hittar svenska kommandon även när gränssnittet är svenskt.
        let en_words = rank("find in files", commands::ALL, &[], &e, Lang::Sv);
        assert_eq!(en_words[0].cmd.id, "edit.find_files");
        assert!(en_words[0].other_lang);
        // Och tvärtom.
        let sv_words = rank("sok i filer", commands::ALL, &[], &e, Lang::En);
        assert_eq!(sv_words[0].cmd.id, "edit.find_files");
    }

    #[test]
    fn empty_query_lists_recent_first_then_registry_order() {
        let s = Settings::default();
        let e = env(&s);
        let recent = vec!["view.theme".to_owned(), "file.print".to_owned()];
        let hits = rank("", commands::ALL, &recent, &e, Lang::Sv);
        assert_eq!(hits.len(), commands::ALL.len());
        assert_eq!(hits[0].cmd.id, "view.theme");
        assert_eq!(hits[1].cmd.id, "file.print");
        assert_eq!(hits[2].cmd.id, commands::ALL[0].id);
    }

    use crate::app::commands::{Category, Command};

    fn yes(_: &Env) -> bool {
        true
    }

    fn no(_: &Env) -> bool {
        false
    }

    fn nop(_: &mut KladdApp, _: &egui::Context) {}

    static SAME: [Command; 3] = [
        Command::new("t.a", "Alfa ett", Category::Tools, "", no, nop),
        Command::new("t.b", "Alfa två", Category::Tools, "", yes, nop),
        Command::new("t.c", "Alfa tre", Category::Tools, "", yes, nop),
    ];

    #[test]
    fn recent_commands_get_a_bonus_in_a_search() {
        let s = Settings::default();
        let e = env(&s);
        let plain = rank("alfa", &SAME, &[], &e, Lang::Sv);
        assert_eq!(ids(&plain), ["t.b", "t.c", "t.a"], "i registrets ordning när poängen är lika");
        let recent = vec!["t.c".to_owned()];
        let with = rank("alfa", &SAME, &recent, &e, Lang::Sv);
        assert_eq!(ids(&with), ["t.c", "t.b", "t.a"]);
    }

    #[test]
    fn unavailable_commands_sink() {
        let s = Settings::default();
        let e = env(&s);
        // "Alfa ett" står först i listan men går inte att köra: den hamnar sist.
        let hits = rank("alfa", &SAME, &[], &e, Lang::Sv);
        assert_eq!(hits.last().unwrap().cmd.id, "t.a");
        // Utan fråga ligger den kvar på sin plats (registrets ordning) men visas nedtonad.
        let all = rank("", &SAME, &[], &e, Lang::Sv);
        assert_eq!(ids(&all), ["t.a", "t.b", "t.c"]);
        assert!(!all[0].cmd.available(&e));
    }
}
