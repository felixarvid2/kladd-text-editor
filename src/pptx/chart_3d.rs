//! 3D för diagram: kameran ur `c:view3D`, projektionen, belysningen och målarordningen (ytor ritas
//! bakifrån och fram). Rena funktioner utan ritning; `chart_3d_draw.rs` gör om ytorna till visningslistan.
//!
//! Världens koordinater: x åt höger (kategorierna), y uppåt (värdena), z bakåt (bort från betraktaren,
//! serierna på djupet). Kamerans koordinater: x åt höger, y uppåt, z mot betraktaren.
//!
//! - **Rätvinkliga axlar** (`rAngAx = 1`, standard för stapel/linje/yta): framsidan ritas oförvrängd och
//!   djupet läggs på snett (parallellprojektion med lutning). En enhet bakåt flyttar punkten
//!   (sin `rotY`, sin `rotX` · cos `rotY`) på skärmen, samma riktning som djupaxeln får vid en riktig
//!   vridning, så att bilden liknar Excels.
//! - **Annars** vrids världen `rotY` runt den lodräta axeln och sedan `rotX` runt den vågräta, och
//!   `perspective` (0–240, standard 30) ger ett synfält på halva värdet i grader (som PowerPoint).
//!
//! **Belysningen** är en riktad ljuskälla plus omgivningsljus. Konstanterna är anpassade (minsta
//! kvadrat) till sidornas färger i två 3D-cirklar i PowerPoints PDF-exporter (`rotX` 30 och 50): sidan
//! rakt fram har 55 % respektive 38 % av toppens ljushet, vänsterkanten 77 %, högerkanten 39 %.

/// En punkt eller riktning i 3D.
pub type V3 = [f32; 3];

/// Omgivningsljusets andel (anpassat, se modulens beskrivning).
pub const AMBIENT: f32 = 0.35;
/// Ljusets riktning i kamerans koordinater: 40° till vänster om rakt upp, 36° ut från skärmens plan.
pub const LIGHT: V3 = [-0.5200, 0.6198, 0.5878];

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-9 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// Ljusheten för en yta med normalen `n` relativt en yta med normalen `reference` (båda i kamerans
/// koordinater): 1 = samma som referensytan, som ritas i seriens färg.
pub fn shade(n: V3, reference: V3) -> f32 {
    let d = 1.0 - AMBIENT;
    let lit = |v: V3| AMBIENT + d * dot(norm(v), LIGHT).max(0.0);
    lit(n) / lit(reference).max(1e-3)
}

/// Kameran.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub rot_x: f32,
    pub rot_y: f32,
    pub right_angle: bool,
    /// Perspektiv: ögats avstånd från mitten i världens enheter (`None` = parallellprojektion).
    pub eye: Option<f32>,
}

impl Camera {
    /// Kameran för en vy (`rot_x`/`rot_y` i grader, `perspective` 0–240) och scenens storlek `size`
    /// (största måttet i världens enheter, för perspektivet).
    pub fn new(rot_x: f32, rot_y: f32, right_angle: bool, perspective: f32, size: f32) -> Camera {
        let fov = (perspective / 2.0).clamp(0.0, 120.0);
        let eye = (!right_angle && fov > 0.5).then(|| size / (fov.to_radians() / 2.0).tan().max(1e-3));
        Camera { rot_x, rot_y, right_angle, eye }
    }

    /// Skärmförskjutningen (x, y uppåt) för en enhet bakåt vid rätvinkliga axlar.
    pub fn depth_dir(&self) -> (f32, f32) {
        let (sy, cy) = self.rot_y.to_radians().sin_cos();
        (sy, self.rot_x.to_radians().sin() * cy)
    }

    /// En punkt (världens koordinater, relativt scenens mitt) i kamerans koordinater.
    pub fn view(&self, p: V3) -> V3 {
        if self.right_angle {
            let (dx, dy) = self.depth_dir();
            return [p[0] + p[2] * dx, p[1] + p[2] * dy, -p[2]];
        }
        let (sy, cy) = self.rot_y.to_radians().sin_cos();
        let (sx, cx) = self.rot_x.to_radians().sin_cos();
        let x1 = p[0] * cy + p[2] * sy;
        let z1 = -p[0] * sy + p[2] * cy;
        let y2 = p[1] * cx + z1 * sx;
        let z2 = -p[1] * sx + z1 * cx;
        [x1, y2, -z2]
    }

    /// En riktning (normal) i kamerans koordinater. Vid rätvinkliga axlar används den riktiga vridningen,
    /// så att ytorna skuggas som i en riktig vy.
    pub fn view_dir(&self, n: V3) -> V3 {
        let c = Camera { right_angle: false, eye: None, ..*self };
        let o = c.view([0.0; 3]);
        norm(sub(c.view(n), o))
    }

    /// Projektion: (x, y uppåt, djup mot betraktaren). Med perspektiv krymper det som ligger längre bort.
    pub fn project(&self, p: V3) -> (f32, f32, f32) {
        let v = self.view(p);
        match self.eye {
            Some(e) => {
                let k = e / (e - v[2]).max(e * 0.05);
                (v[0] * k, v[1] * k, v[2])
            }
            None => (v[0], v[1], v[2]),
        }
    }

    /// Är en plan yta med normalen `n` (världens koordinater) vänd mot betraktaren?
    pub fn faces_viewer(&self, n: V3) -> bool {
        self.view_dir(n)[2] > 1e-4
    }
}

/// En yta att rita: hörnen på skärmen (y nedåt, i anroparens koordinater), djupet (större = närmare)
/// och ljusheten.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Vilket föremål ytan hör till (cirkelsektor, stapel, serie …).
    pub owner: usize,
    pub kind: FaceKind,
    pub pts: Vec<(f32, f32)>,
    pub depth: f32,
    pub shade: f32,
    /// Ljushet som varierar vågrätt (cirkelns sidor): (x, ljushet) i stigande x.
    pub shade_x: Vec<(f32, f32)>,
}

/// Slags yta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceKind {
    Top,
    Side,
    /// Snittyta (en cirkelsektors raka sida).
    Cut,
    Front,
    Back,
    Bottom,
    Wall,
    Floor,
}

/// Sorterar ytorna bakifrån och fram (målaralgoritmen; stabil för lika djup).
pub fn painter_sort(faces: &mut [Face]) {
    faces.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap_or(std::cmp::Ordering::Equal));
}

// ---------------------------------------------------------------------------------------------
// Cirkel

/// En sektor: startvinkel och storlek i grader (medurs från klockan tolv sett ovanifrån, där klockan
/// tolv är längst bort) och utskjutningen som andel av radien.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slice {
    pub start: f32,
    pub sweep: f32,
    pub explosion: f32,
}

/// Geometrin för en 3D-cirkel: mitten av toppens ellips (`cx`, `cy`, skärmen med y nedåt), radien,
/// tjockleken (världens enheter) och lutningen `rot_x`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pie {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub thickness: f32,
    pub rot_x: f32,
}

impl Pie {
    fn cam(&self) -> Camera {
        Camera { rot_x: self.rot_x, rot_y: 0.0, right_angle: false, eye: None }
    }

    /// Punkt på skärmen och djup för vinkeln `a` (grader), avståndet `rho` från mitten, höjden `y`
    /// (0 = toppen, negativt nedåt) och utskjutningen (`ox`, `oz`).
    pub fn point(&self, a: f32, rho: f32, y: f32, off: (f32, f32)) -> (f32, f32, f32) {
        let t = a.to_radians();
        let p = [rho * t.sin() + off.0, y, rho * t.cos() + off.1];
        let (x, yu, d) = self.cam().project(p);
        (self.cx + x, self.cy - yu, d)
    }

    /// Utskjutningen för en sektor (världens x och z).
    pub fn offset(&self, s: &Slice) -> (f32, f32) {
        let m = (s.start + s.sweep / 2.0).to_radians();
        (s.explosion * self.r * m.sin(), s.explosion * self.r * m.cos())
    }

    /// Normalen (kamerans koordinater) för en vågrät riktning med vinkeln `a`.
    fn horizontal(&self, a: f32) -> V3 {
        let t = a.to_radians();
        self.cam().view_dir([t.sin(), 0.0, t.cos()])
    }

    fn up(&self) -> V3 {
        self.cam().view_dir([0.0, 1.0, 0.0])
    }

    /// Alla synliga ytor (toppar, sidor, snittytor) i ritordning.
    pub fn faces(&self, slices: &[Slice]) -> Vec<Face> {
        let n = slices.len();
        let up = self.up();
        let mut back: Vec<Face> = Vec::new();
        let mut tops: Vec<Face> = Vec::new();
        let steps = |sw: f32| ((sw.abs() / 3.0).ceil() as usize).clamp(2, 180);
        for (i, s) in slices.iter().enumerate() {
            if s.sweep <= 0.0 {
                continue;
            }
            let off = self.offset(s);
            let (a0, a1) = (s.start, s.start + s.sweep);
            // Toppen.
            let k = steps(s.sweep);
            let mut pts = Vec::with_capacity(k + 2);
            let full = s.sweep >= 359.99;
            if !full {
                let c = self.point(0.0, 0.0, 0.0, off);
                pts.push((c.0, c.1));
            }
            let mut dsum = 0.0;
            for j in 0..=k {
                let p = self.point(a0 + s.sweep * j as f32 / k as f32, self.r, 0.0, off);
                dsum += p.2;
                pts.push((p.0, p.1));
            }
            tops.push(Face { owner: i, kind: FaceKind::Top, pts, depth: dsum / (k + 1) as f32, shade: 1.0, shade_x: Vec::new() });
            // Sidan: de delar av bågen som vetter mot betraktaren (klockan 3 till 9 genom 6).
            for (b0, b1) in front_ranges(a0, a1) {
                let k = steps(b1 - b0);
                let mut top = Vec::with_capacity(k + 1);
                let mut shade_x = Vec::with_capacity(k + 1);
                let mut dsum = 0.0;
                for j in 0..=k {
                    let a = b0 + (b1 - b0) * j as f32 / k as f32;
                    let p = self.point(a, self.r, 0.0, off);
                    dsum += p.2;
                    top.push((p.0, p.1));
                    shade_x.push((p.0, shade(self.horizontal(a), up)));
                }
                let mut pts = top.clone();
                for j in (0..=k).rev() {
                    let a = b0 + (b1 - b0) * j as f32 / k as f32;
                    let p = self.point(a, self.r, -self.thickness, off);
                    pts.push((p.0, p.1));
                }
                shade_x.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                let sh = shade_x.iter().map(|s| s.1).sum::<f32>() / shade_x.len().max(1) as f32;
                back.push(Face { owner: i, kind: FaceKind::Side, pts, depth: dsum / (k + 1) as f32, shade: sh, shade_x });
            }
            // Snittytorna syns bara där sektorn är skild från grannen (utskjuten).
            if !full && n > 1 {
                let prev = &slices[(i + n - 1) % n];
                let next = &slices[(i + 1) % n];
                for (a, normal_a, other) in [(a0, a0 - 90.0, prev), (a1, a1 + 90.0, next)] {
                    if s.explosion <= 0.0 && other.explosion <= 0.0 {
                        continue;
                    }
                    let nv = self.horizontal(normal_a);
                    if nv[2] <= 1e-4 {
                        continue;
                    }
                    let c0 = self.point(0.0, 0.0, 0.0, off);
                    let r0 = self.point(a, self.r, 0.0, off);
                    let r1 = self.point(a, self.r, -self.thickness, off);
                    let c1 = self.point(0.0, 0.0, -self.thickness, off);
                    let depth = (c0.2 + r0.2 + r1.2 + c1.2) / 4.0;
                    back.push(Face { owner: i, kind: FaceKind::Cut, pts: vec![(c0.0, c0.1), (r0.0, r0.1), (r1.0, r1.1), (c1.0, c1.1)], depth, shade: shade(nv, up), shade_x: Vec::new() });
                }
            }
        }
        painter_sort(&mut back);
        painter_sort(&mut tops);
        back.extend(tops);
        back
    }
}

/// Delarna av vinkelintervallet [a0, a1] där bågens normal vetter mot betraktaren (90°..270° plus
/// hela varv).
pub fn front_ranges(a0: f32, a1: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let k0 = ((a0 - 270.0) / 360.0).floor() as i32 - 1;
    let k1 = ((a1 - 90.0) / 360.0).ceil() as i32 + 1;
    for k in k0..=k1 {
        let lo = 90.0 + 360.0 * k as f32;
        let hi = 270.0 + 360.0 * k as f32;
        let (s, e) = (a0.max(lo), a1.min(hi));
        if e - s > 1e-3 {
            out.push((s, e));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Lådor, cylindrar, koner och pyramider (staplar)

/// Formen på en 3D-stapel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarShape {
    Box,
    Cylinder,
    /// Kon/pyramid: spetsen i stapelns topp, eller vid axelns största värde (`to_max`).
    Cone { to_max: bool },
    Pyramid { to_max: bool },
}

impl BarShape {
    pub fn parse(s: &str) -> BarShape {
        match s {
            "cylinder" => BarShape::Cylinder,
            "cone" => BarShape::Cone { to_max: false },
            "coneToMax" => BarShape::Cone { to_max: true },
            "pyramid" => BarShape::Pyramid { to_max: false },
            "pyramidToMax" => BarShape::Pyramid { to_max: true },
            _ => BarShape::Box,
        }
    }
}

/// En plan polygon i världen.
#[derive(Clone, Debug)]
pub struct Poly {
    pub kind: FaceKind,
    pub pts: Vec<V3>,
}

impl Poly {
    /// Normalen enligt hörnens ordning (moturs sett utifrån, i världens högerhänta system med z bakåt
    /// blir det medurs; se `outward`).
    fn normal(&self) -> V3 {
        let n = self.pts.len();
        let mut acc = [0.0; 3];
        for i in 0..n {
            let a = self.pts[i];
            let b = self.pts[(i + 1) % n];
            acc = [acc[0] + (a[1] - b[1]) * (a[2] + b[2]), acc[1] + (a[2] - b[2]) * (a[0] + b[0]), acc[2] + (a[0] - b[0]) * (a[1] + b[1])];
        }
        norm(acc)
    }

    fn center(&self) -> V3 {
        let n = self.pts.len().max(1) as f32;
        let s = self.pts.iter().fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
        [s[0] / n, s[1] / n, s[2] / n]
    }
}

/// Ytorna (med utåtriktade normaler) för en stapel mellan `lo` och `hi` (hörn; `lo` har de minsta
/// koordinaterna). `vertical` = lodrät stapel (värdet längs y), annars längs x. `tip` är spetsens
/// läge längs värdeaxeln för koner och pyramider mot största värdet.
pub fn bar_polys(shape: BarShape, lo: V3, hi: V3, vertical: bool, negative: bool, tip: Option<f32>) -> Vec<Poly> {
    // Arbeta i ett lokalt system där stapeln växer längs "upp"; byt axlar för liggande staplar.
    let map = |u: f32, v: f32, w: f32| -> V3 { if vertical { [u, v, w] } else { [v, u, w] } };
    let (u0, u1, w0, w1) = if vertical { (lo[0], hi[0], lo[2], hi[2]) } else { (lo[1], hi[1], lo[2], hi[2]) };
    let (mut v0, mut v1) = if vertical { (lo[1], hi[1]) } else { (lo[0], hi[0]) };
    if negative {
        std::mem::swap(&mut v0, &mut v1);
    }
    let (cu, cw) = ((u0 + u1) / 2.0, (w0 + w1) / 2.0);
    let (ru, rw) = ((u1 - u0) / 2.0, (w1 - w0) / 2.0);
    let polys: Vec<Poly> = match shape {
        BarShape::Box => {
            let ring = [(u0, w0), (u1, w0), (u1, w1), (u0, w1)];
            prism(&ring, v0, v1, &map)
        }
        BarShape::Cylinder => {
            let ring: Vec<(f32, f32)> = (0..36).map(|k| {
                let t = (k as f32 / 36.0) * std::f32::consts::TAU;
                (cu + ru * t.cos(), cw - rw * t.sin())
            }).collect();
            prism(&ring, v0, v1, &map)
        }
        BarShape::Cone { to_max } | BarShape::Pyramid { to_max } => {
            let ring: Vec<(f32, f32)> = if matches!(shape, BarShape::Pyramid { .. }) {
                vec![(u0, w0), (u1, w0), (u1, w1), (u0, w1)]
            } else {
                (0..36).map(|k| {
                    let t = (k as f32 / 36.0) * std::f32::consts::TAU;
                    (cu + ru * t.cos(), cw - rw * t.sin())
                }).collect()
            };
            let apex = if to_max { tip.unwrap_or(v1) } else { v1 };
            // Mot största värdet: stapeln är en stympad kon/pyramid som slutar vid värdet.
            let f = if to_max && (apex - v0).abs() > 1e-6 { ((v1 - v0) / (apex - v0)).clamp(0.0, 1.0) } else { 1.0 };
            if f >= 0.999 {
                let mut out = vec![Poly { kind: FaceKind::Bottom, pts: ring.iter().rev().map(|&(u, w)| map(u, v0, w)).collect() }];
                let a = map(cu, apex, cw);
                for i in 0..ring.len() {
                    let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
                    out.push(Poly { kind: FaceKind::Side, pts: vec![map(p.0, v0, p.1), map(q.0, v0, q.1), a] });
                }
                out
            } else {
                let top: Vec<(f32, f32)> = ring.iter().map(|&(u, w)| (cu + (u - cu) * (1.0 - f), cw + (w - cw) * (1.0 - f))).collect();
                frustum(&ring, &top, v0, v1, &map)
            }
        }
    };
    // Normalerna ska peka utåt från stapelns mitt.
    let mid = map(cu, (v0 + v1) / 2.0, cw);
    polys
        .into_iter()
        .map(|mut p| {
            if dot(p.normal(), sub(p.center(), mid)) < 0.0 {
                p.pts.reverse();
            }
            p
        })
        .collect()
}

fn prism(ring: &[(f32, f32)], v0: f32, v1: f32, map: &dyn Fn(f32, f32, f32) -> V3) -> Vec<Poly> {
    frustum(ring, ring, v0, v1, map)
}

fn frustum(bottom: &[(f32, f32)], top: &[(f32, f32)], v0: f32, v1: f32, map: &dyn Fn(f32, f32, f32) -> V3) -> Vec<Poly> {
    let n = bottom.len();
    let mut out = vec![
        Poly { kind: FaceKind::Bottom, pts: bottom.iter().map(|&(u, w)| map(u, v0, w)).collect() },
        Poly { kind: FaceKind::Top, pts: top.iter().map(|&(u, w)| map(u, v1, w)).collect() },
    ];
    for i in 0..n {
        let j = (i + 1) % n;
        let kind = if n == 4 && i == 0 { FaceKind::Front } else if n == 4 && i == 2 { FaceKind::Back } else { FaceKind::Side };
        out.push(Poly { kind, pts: vec![map(bottom[i].0, v0, bottom[i].1), map(bottom[j].0, v0, bottom[j].1), map(top[j].0, v1, top[j].1), map(top[i].0, v1, top[i].1)] });
    }
    out
}

/// Projicerar ett föremåls ytor: bara de som vetter mot betraktaren, skuggade relativt framsidan.
/// `to_screen` gör om kamerans (x, y uppåt) till skärmen.
pub fn project_polys(cam: &Camera, polys: &[Poly], owner: usize, to_screen: &dyn Fn(f32, f32) -> (f32, f32)) -> Vec<Face> {
    let front = cam.view_dir([0.0, 0.0, -1.0]);
    let mut out = Vec::new();
    for p in polys {
        let n = p.normal();
        if !cam.faces_viewer(n) {
            continue;
        }
        let mut pts = Vec::with_capacity(p.pts.len());
        let mut d = 0.0;
        for q in &p.pts {
            let (x, y, z) = cam.project(*q);
            d += z;
            pts.push(to_screen(x, y));
        }
        let depth = d / p.pts.len().max(1) as f32;
        out.push(Face { owner, kind: p.kind, pts, depth, shade: shade(cam.view_dir(n), front), shade_x: Vec::new() });
    }
    out
}

/// Ett föremåls djup (dess mitts djup i kameran), för att rita föremålen bakifrån och fram.
pub fn object_depth(cam: &Camera, polys: &[Poly]) -> f32 {
    let n = polys.len().max(1) as f32;
    let c = polys.iter().map(|p| p.center()).fold([0.0; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
    cam.view(c)[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn ratvinklig_projektion_flyttar_djupet_snett() {
        // rotX 15, rotY 20: en enhet bakåt → (sin 20°, sin 15° · cos 20°) uppåt till höger.
        let cam = Camera::new(15.0, 20.0, true, 30.0, 1.0);
        assert!(cam.eye.is_none());
        let (x, y, z) = cam.project([1.0, 2.0, 0.0]);
        assert!(near(x, 1.0) && near(y, 2.0) && near(z, 0.0), "framsidan oförvrängd");
        let (x, y, _) = cam.project([0.0, 0.0, 1.0]);
        assert!(near(x, 20f32.to_radians().sin()) && near(y, 15f32.to_radians().sin() * 20f32.to_radians().cos()), "{x} {y}");
        // Lådans åtta hörn: framsidans fyra på sina platser, baksidans förskjutna lika mycket.
        let (dx, dy) = cam.depth_dir();
        for (px, py, pz) in [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (1.0, 1.0, 0.0), (0.0, 0.0, 0.5), (1.0, 1.0, 0.5)] {
            let (x, y, _) = cam.project([px, py, pz]);
            assert!(near(x, px + pz * dx) && near(y, py + pz * dy));
        }
    }

    #[test]
    fn vriden_vy_och_perspektiv() {
        // Rakt framifrån utan vridning: punkten oförändrad, djupet negativt bakåt.
        let cam = Camera::new(0.0, 0.0, false, 0.0, 1.0);
        assert_eq!(cam.project([0.3, 0.4, 0.5]), (0.3, 0.4, -0.5));
        // rotY 90 (sett från höger): en punkt till höger hamnar närmast, en punkt bakom hamnar till höger.
        let cam = Camera::new(0.0, 90.0, false, 0.0, 1.0);
        let (x, _, z) = cam.project([1.0, 0.0, 0.0]);
        assert!(near(x, 0.0) && near(z, 1.0), "{x} {z}");
        let (x, _, _) = cam.project([0.0, 0.0, 1.0]);
        assert!(near(x, 1.0));
        // rotX 90 (rakt ovanifrån): en punkt bakom hamnar högst upp.
        let cam = Camera::new(90.0, 0.0, false, 0.0, 1.0);
        let (_, y, _) = cam.project([0.0, 0.0, 1.0]);
        assert!(near(y, 1.0));
        // Perspektiv: det som ligger närmare blir större.
        let cam = Camera::new(0.0, 0.0, false, 60.0, 1.0);
        let near_x = cam.project([1.0, 0.0, -0.5]).0;
        let far_x = cam.project([1.0, 0.0, 0.5]).0;
        assert!(near_x > 1.0 && far_x < 1.0, "{near_x} {far_x}");
    }

    #[test]
    fn belysning_som_powerpoints_cirklar() {
        // Sidan rakt fram relativt toppen vid rotX 30 och 50 (uppmätt 0,55 och 0,38).
        for (rx, want) in [(30.0, 0.55), (50.0, 0.38)] {
            let p = Pie { cx: 0.0, cy: 0.0, r: 1.0, thickness: 0.2, rot_x: rx };
            let s = shade(p.horizontal(180.0), p.up());
            assert!((s - want).abs() < 0.03, "rotX {rx}: {s}");
        }
        // Vänster sida ljusare än höger.
        let p = Pie { cx: 0.0, cy: 0.0, r: 1.0, thickness: 0.2, rot_x: 30.0 };
        assert!(shade(p.horizontal(240.0), p.up()) > shade(p.horizontal(120.0), p.up()));
    }

    #[test]
    fn cirkelns_ytor_i_malarordning() {
        let p = Pie { cx: 100.0, cy: 100.0, r: 50.0, thickness: 10.0, rot_x: 30.0 };
        // Toppens ellips: klockan tolv ligger r · sin 30° ovanför mitten, klockan sex lika långt under.
        let (x, y, _) = p.point(0.0, 50.0, 0.0, (0.0, 0.0));
        assert!(near(x, 100.0) && near(y, 75.0), "{x} {y}");
        let (_, y, _) = p.point(180.0, 50.0, -10.0, (0.0, 0.0));
        assert!(near(y, 125.0 + 10.0 * 30f32.to_radians().cos()));
        let slices = [Slice { start: 0.0, sweep: 90.0, explosion: 0.1 }, Slice { start: 90.0, sweep: 270.0, explosion: 0.0 }];
        let faces = p.faces(&slices);
        // Topparna sist, sidor och snittytor före; inom varje grupp bakifrån och fram.
        let first_top = faces.iter().position(|f| f.kind == FaceKind::Top).unwrap();
        assert!(faces[first_top..].iter().all(|f| f.kind == FaceKind::Top));
        assert_eq!(faces.iter().filter(|f| f.kind == FaceKind::Top).count(), 2);
        for w in faces[..first_top].windows(2) {
            assert!(w[0].depth <= w[1].depth);
        }
        // Den första sektorn (klockan 12–3) har ingen synlig sida, den andra (3–12 via 6) har det.
        assert!(!faces.iter().any(|f| f.kind == FaceKind::Side && f.owner == 0));
        assert!(faces.iter().any(|f| f.kind == FaceKind::Side && f.owner == 1));
        // Den utskjutna sektorns snittyta vid klockan tre vetter mot betraktaren.
        assert!(faces.iter().any(|f| f.kind == FaceKind::Cut && f.owner == 0));
        // Sidan blir mörkare åt höger.
        let side = faces.iter().find(|f| f.kind == FaceKind::Side).unwrap();
        assert!(side.shade_x.first().unwrap().1 > side.shade_x.last().unwrap().1);
    }

    #[test]
    fn framsidans_vinkelintervall() {
        assert_eq!(front_ranges(0.0, 90.0), vec![]);
        assert_eq!(front_ranges(0.0, 360.0), vec![(90.0, 270.0)]);
        assert_eq!(front_ranges(200.0, 500.0), vec![(200.0, 270.0), (450.0, 500.0)]);
    }

    #[test]
    fn lada_med_synliga_sidor_och_ordning() {
        let cam = Camera::new(15.0, 20.0, true, 30.0, 1.0);
        let polys = bar_polys(BarShape::Box, [0.0, 0.0, 0.0], [1.0, 2.0, 1.0], true, false, None);
        assert_eq!(polys.len(), 6);
        let faces = project_polys(&cam, &polys, 0, &|x, y| (x, -y));
        // Framsidan, toppen och högersidan syns; toppen ljusare och sidan mörkare än framsidan.
        let kinds: Vec<FaceKind> = faces.iter().map(|f| f.kind).collect();
        assert_eq!(faces.len(), 3, "{kinds:?}");
        let get = |k: FaceKind| faces.iter().find(|f| f.kind == k).unwrap();
        assert!(near(get(FaceKind::Front).shade, 1.0));
        assert!(get(FaceKind::Top).shade > 1.0);
        assert!(get(FaceKind::Side).shade < 1.0);
        // En stapel längre bak ritas före en längre fram.
        let back = bar_polys(BarShape::Box, [0.0, 0.0, 2.0], [1.0, 1.0, 3.0], true, false, None);
        assert!(object_depth(&cam, &back) < object_depth(&cam, &polys));
        // Normalerna pekar utåt även för negativa och liggande staplar och för koner.
        for (shape, vertical, neg) in [(BarShape::Box, true, true), (BarShape::Cylinder, false, false), (BarShape::Cone { to_max: false }, true, false), (BarShape::Pyramid { to_max: true }, true, false)] {
            let ps = bar_polys(shape, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], vertical, neg, Some(2.0));
            let c = [0.5, 0.5, 0.5];
            for p in &ps {
                assert!(dot(p.normal(), sub(p.center(), c)) >= -1e-4, "{shape:?} {:?}", p.kind);
            }
        }
    }
}
