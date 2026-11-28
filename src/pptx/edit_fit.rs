//! Autoanpassning av text under redigering: när texten i en form med `a:normAutofit` växer eller
//! krymper räknas `fontScale`/`lnSpcReduction` om (PowerPoints stege: först 10 % och 20 % mindre
//! radavstånd, sedan 92,5 %, 85 % … 25 % av teckenstorleken med 20 % mindre radavstånd); i en form
//! med `a:spAutoFit` följer formens höjd texten. Textmotorn är den som ritar bilden, så samma
//! utläggning avgör vad som ryms. Värdena räknas bara om när texten ändras, aldrig när filen
//! öppnas, sparas eller formen flyttas.

use super::edit::Txn;
use super::edit_ops::{self, Xf};
use super::edit_text::{AutoFit, TextModel};
use super::edit_xml::XElem;
use super::model::{Deck, EMU_PER_PT};
use super::xml::Node;

/// Stegen i PowerPoints autoanpassning, från minst till mest krympning: (teckenskala, radavståndsminskning).
pub const LADDER: &[(f32, f32)] = &[
    (1.0, 0.0),
    (1.0, 0.1),
    (1.0, 0.2),
    (0.925, 0.2),
    (0.85, 0.2),
    (0.775, 0.2),
    (0.7, 0.2),
    (0.625, 0.2),
    (0.55, 0.2),
    (0.475, 0.2),
    (0.4, 0.2),
    (0.325, 0.2),
    (0.25, 0.2),
];

/// Spelrum vid jämförelsen mellan textens höjd och rutans (punkter).
const SLACK: f32 = 0.5;

/// Det som ska skrivas efter en textändring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    /// Nya värden för `a:normAutofit`.
    Norm { scale: f32, red: f32 },
    /// Ny placering och höjd för en form med `a:spAutoFit`.
    Shape { y: i64, cy: i64 },
}

impl TextModel {
    /// Vad autoanpassningen ska bli för den här texten i formen `sp` (bild `slide`), eller `None`
    /// om inget ska ändras. `xf` är formens placering och `before_h` textens höjd före ändringen
    /// (för att krympa en `spAutoFit`-form lika mycket som texten krympte).
    pub fn autofit(&self, deck: &Deck, slide: usize, sp: &Node, xf: Xf, before_h: Option<f32>) -> Option<Fit> {
        let base = self.carets_fit(deck, slide, sp, None)?;
        if !base.fit_ok {
            return None;
        }
        match base.autofit {
            AutoFit::Norm { scale, red } => {
                let fits = |s: f32, r: f32| self.carets_fit(deck, slide, sp, Some((s, r))).is_some_and(|c| c.text_h <= c.avail_h + SLACK);
                let cur_fits = base.text_h <= base.avail_h + SLACK;
                // Första steget som är minst lika kraftigt som det nuvarande.
                let at = LADDER.iter().position(|&(s, r)| s <= scale + 1e-3 && r >= red - 1e-3).unwrap_or(LADDER.len() - 1);
                let exact = (LADDER[at].0 - scale).abs() < 1e-3 && (LADDER[at].1 - red).abs() < 1e-3;
                // Att något steg ryms gäller också alla stegen efter det (monotont): ett prov av steget före
                // det nuvarande räcker oftast, och sökningen mot mjukare steg är binär.
                let pick = if cur_fits {
                    if at == 0 || !fits(LADDER[at - 1].0, LADDER[at - 1].1) {
                        None
                    } else {
                        let (mut lo, mut hi) = (0, at - 1);
                        while lo < hi {
                            let mid = (lo + hi) / 2;
                            if fits(LADDER[mid].0, LADDER[mid].1) {
                                hi = mid;
                            } else {
                                lo = mid + 1;
                            }
                        }
                        Some(LADDER[lo])
                    }
                } else {
                    let from = if exact { at + 1 } else { at };
                    Some((from..LADDER.len()).map(|i| LADDER[i]).find(|&(s, r)| fits(s, r)).unwrap_or(LADDER[LADDER.len() - 1]))
                };
                let (s, r) = pick?;
                ((s - scale).abs() > 1e-3 || (r - red).abs() > 1e-3).then_some(Fit::Norm { scale: s, red: r })
            }
            AutoFit::Shape => {
                let need = base.text_h + base.ins_tb.0 + base.ins_tb.1;
                let cur = xf.cy as f32 / EMU_PER_PT as f32;
                let new = if cur < need - SLACK {
                    need
                } else if let Some(b) = before_h.filter(|b| base.text_h < b - SLACK) {
                    (cur - (b - base.text_h)).max(need)
                } else {
                    return None;
                };
                if (new - cur).abs() < SLACK {
                    return None;
                }
                let cy = (new as f64 * EMU_PER_PT).round() as i64;
                let d = xf.cy - cy;
                let y = match base.anchor {
                    // Vridna former behåller sin övre kant (förskjutningen skulle behöva vridas med).
                    _ if xf.rot != 0 => xf.y,
                    "ctr" => xf.y + d / 2,
                    "b" => xf.y + d,
                    _ => xf.y,
                };
                Some(Fit::Shape { y, cy })
            }
            AutoFit::None => None,
        }
    }
}

/// Skriver textmodellen och autoanpassningen i samma ändring (ett ångra-steg).
pub fn set_text_fit(txn: &mut Txn, slide: usize, index: usize, id: u32, model: &TextModel, fit: Option<Fit>, xf: Xf) -> Result<(), String> {
    edit_ops::set_text(txn, slide, index, id, model)?;
    let Some(fit) = fit else { return Ok(()) };
    edit_ops::with_shape(txn, slide, index, id, &mut |e, doc| match fit {
        Fit::Norm { scale, red } => write_norm(e, doc, scale, red),
        Fit::Shape { y, cy } => edit_ops::write_xf(e, doc, Xf { y, cy, ..xf }),
    })
}

/// Sätter `fontScale`/`lnSpcReduction` på formens egen `a:normAutofit` (skapas efter `a:prstTxWarp`
/// och före `a:scene3d` om formen ärvde den).
pub(super) fn write_norm(e: &mut XElem, doc: &super::edit_xml::XDoc, scale: f32, red: f32) -> Result<(), String> {
    let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
    let tag = if a.is_empty() { "normAutofit".to_owned() } else { format!("{a}:normAutofit") };
    let body = e.child_mut("p:txBody").and_then(|b| b.child_mut("a:bodyPr")).ok_or("bodyPr")?;
    if body.child("a:normAutofit").is_none() {
        body.remove_children("a:noAutofit");
        body.remove_children("a:spAutoFit");
        let el = doc.fragment(&format!("<{tag}/>")).into_iter().next().ok_or("normAutofit")?;
        body.insert_before(el, &["a:scene3d", "a:sp3d", "a:flatTx", "a:extLst"]);
    }
    let fit = body.child_mut("a:normAutofit").ok_or("normAutofit")?;
    let pct = |v: f32| ((v * 100_000.0).round() as i64).to_string();
    if scale < 0.999 {
        fit.set_attr("fontScale", &pct(scale));
    } else {
        fit.remove_attr("fontScale");
    }
    if red > 0.001 {
        fit.set_attr("lnSpcReduction", &pct(red));
    } else {
        fit.remove_attr("lnSpcReduction");
    }
    Ok(())
}

/// Skriver bara autoanpassningen (efter att formen ändrat storlek med handtagen).
pub fn set_fit_only(txn: &mut Txn, slide: usize, index: usize, id: u32, fit: Fit, xf: Xf) -> Result<(), String> {
    edit_ops::with_shape(txn, slide, index, id, &mut |e, doc| match fit {
        Fit::Norm { scale, red } => write_norm(e, doc, scale, red),
        Fit::Shape { y, cy } => edit_ops::write_xf(e, doc, Xf { y, cy, ..xf }),
    })
}
