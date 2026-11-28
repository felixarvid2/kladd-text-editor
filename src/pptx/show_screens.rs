//! Skärmarna för bildspelet: var publikens helskärm och föredragsvyn hamnar när det finns flera skärmar.
//!
//! egui/winit kan sätta ett fönster i helskärm på skärm nummer N (`ViewportCommand::SetMonitor`) men ger
//! ingen skärmlista. På Linux/X11 läses därför skärmarna med RandR (x11rb, ren Rust) i samma ordning som winit
//! räknar dem (kretsarna med en utgång och en storlek). På andra system saknas listan (tom): bildspelet
//! beter sig då som förut, och *Byt skärm* och skärmvalet visas inte. Rent val av skärm provas här.
//!
//! `KLADD_SCREENS="x,y,b,h;x,y,b,h"` ersätter listan (för att pröva menyn och placeringen utan flera skärmar).

/// En skärm i bildpunkter i skrivbordets koordinater.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Skärmarna i winits ordning. Tom om de inte går att läsa.
pub fn list() -> Vec<Screen> {
    if let Ok(s) = std::env::var("KLADD_SCREENS") {
        return parse(&s);
    }
    platform()
}

/// `x,y,b,h;x,y,b,h` → skärmar (felaktiga poster hoppas över).
pub fn parse(s: &str) -> Vec<Screen> {
    s.split(';')
        .filter_map(|r| {
            let v: Vec<i32> = r.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            (v.len() == 4 && v[2] > 0 && v[3] > 0).then(|| Screen { x: v[0], y: v[1], w: v[2], h: v[3] })
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
fn platform() -> Vec<Screen> {
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt;
    let read = || -> Option<Vec<Screen>> {
        let (conn, num) = x11rb::rust_connection::RustConnection::connect(None).ok()?;
        let root = conn.setup().roots.get(num)?.root;
        let res = conn.randr_get_screen_resources_current(root).ok()?.reply().ok()?;
        let cookies: Vec<_> = res.crtcs.iter().filter_map(|&c| conn.randr_get_crtc_info(c, x11rb::CURRENT_TIME).ok()).collect();
        // Samma urval som winit: kretsar utan storlek eller utgång räknas inte.
        Some(
            cookies
                .into_iter()
                .filter_map(|c| c.reply().ok())
                .filter(|c| c.width > 0 && c.height > 0 && !c.outputs.is_empty())
                .map(|c| Screen { x: c.x as i32, y: c.y as i32, w: c.width as i32, h: c.height as i32 })
                .collect(),
        )
    };
    read().unwrap_or_default()
}

#[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "android"))))]
fn platform() -> Vec<Screen> {
    Vec::new()
}

/// Skärmen som innehåller punkten.
pub fn index_at(screens: &[Screen], x: f32, y: f32) -> Option<usize> {
    screens.iter().position(|s| x >= s.x as f32 && y >= s.y as f32 && x < (s.x + s.w) as f32 && y < (s.y + s.h) as f32)
}

/// Skärmen publiken visas på. `n` skärmar, Kladd-fönstret på `home`, `pref` = valet i *Visa*-menyn, `swap` =
/// skärmen som *Byt skärm* valt, `presenter` = föredragsvyn är på. `None` = som förut (aktuell skärm).
pub fn audience(n: usize, home: usize, pref: Option<usize>, swap: Option<usize>, presenter: bool) -> Option<usize> {
    if n < 2 {
        return None;
    }
    if let Some(s) = swap.filter(|&s| s < n) {
        return Some(s);
    }
    if let Some(p) = pref.filter(|&p| p < n) {
        return Some(p);
    }
    // Utan föredragsvy stannar bildspelet där fönstret är; med den går publiken till nästa skärm.
    presenter.then(|| (home + 1) % n)
}

/// Skärmen föredragsvyn hamnar på: Kladd-fönstrets egen, annars skärmen före publikens.
pub fn presenter(n: usize, home: usize, audience: usize) -> usize {
    if home != audience && home < n { home } else { (audience + n - 1) % n }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skärmlista_ur_text_och_träff() {
        let s = parse("0,0,1920,1080; 1920,0,1280,720;fel;1,2,3");
        assert_eq!(s.len(), 2);
        assert_eq!(index_at(&s, 100.0, 100.0), Some(0));
        assert_eq!(index_at(&s, 2000.0, 10.0), Some(1));
        assert_eq!(index_at(&s, 5000.0, 10.0), None);
        assert!(parse("").is_empty());
    }

    #[test]
    fn publikens_skärm_och_byte() {
        // En skärm: som förut.
        assert_eq!(audience(1, 0, Some(0), None, true), None);
        assert_eq!(audience(0, 0, None, None, true), None);
        // Två skärmar: automatiskt går publiken till den andra, men bara med föredragsvy.
        assert_eq!(audience(2, 0, None, None, true), Some(1));
        assert_eq!(audience(2, 1, None, None, true), Some(0));
        assert_eq!(audience(2, 0, None, None, false), None);
        // Valet i menyn gäller även utan föredragsvy; ett val utanför listan ignoreras.
        assert_eq!(audience(3, 0, Some(2), None, false), Some(2));
        assert_eq!(audience(2, 0, Some(5), None, true), Some(1));
        // Byt skärm går före valet.
        assert_eq!(audience(3, 0, Some(2), Some(1), true), Some(1));
        // Föredragsvyn på fönstrets skärm, annars skärmen före publikens.
        assert_eq!(presenter(2, 0, 1), 0);
        assert_eq!(presenter(2, 0, 0), 1);
        assert_eq!(presenter(3, 1, 1), 0);
    }
}
