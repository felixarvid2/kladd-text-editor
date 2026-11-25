//! Textmarkeringen i läsytan. egui låter etiketter markera text men ger ingen åtkomst till vad som är markerat;
//! det enda som kommer ut är `OutputCommand::TextSelectionSettled(text)` när markeringen är klar (musen
//! släppt). Ett insticksprogram läser det ur bildrutans utdata och sparar texten och var pekaren var, så att
//! läsvyn nästa bildruta kan visa menyn för understrykning vid markeringen.

use std::sync::Mutex;

use egui::{Context, FullOutput, OutputCommand, Pos2};

/// Den senast avslutade markeringen: texten och pekarens läge.
static SETTLED: Mutex<Option<Settled>> = Mutex::new(None);

/// En avslutad markering: texten, pekarens läge och om den räknas som vald med avsikt utan att musen följts åt (tester).
pub type Settled = (String, Option<Pos2>, bool);

struct Tap;

impl egui::Plugin for Tap {
    fn debug_name(&self) -> &'static str {
        "kladd-selection-tap"
    }

    fn output_hook(&mut self, ctx: &Context, output: &mut FullOutput) {
        for cmd in &output.platform_output.commands {
            if let OutputCommand::TextSelectionSettled(t) = cmd {
                let pos = ctx.input(|i| i.pointer.latest_pos());
                if let Ok(mut s) = SETTLED.lock() {
                    *s = Some((t.clone(), pos, false));
                }
                ctx.request_repaint();
            }
        }
    }
}

/// Installerar insticksprogrammet (en gång per `Context`).
pub fn install(ctx: &Context) {
    ctx.add_plugin(Tap);
}

/// Tar den senast avslutade markeringen, om det kommit någon sedan sist.
pub fn take() -> Option<Settled> {
    SETTLED.lock().ok().and_then(|mut s| s.take())
}

/// Lämnar en markering som om användaren gjort den (gränssnittstester).
#[cfg(test)]
pub fn inject(text: &str, pos: Pos2) {
    if let Ok(mut s) = SETTLED.lock() {
        *s = Some((text.to_owned(), Some(pos), true));
    }
}
