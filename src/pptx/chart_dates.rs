//! Datumaxlar (`c:dateAx`) som i PowerPoint: kategorierna är datum (serienummer) och axeln är en
//! tidsskala från det tidigaste till det senaste datumet i basenheten (`c:baseTimeUnit`: dagar, månader,
//! år), inte en kategori per cachad punkt. Saknade dagar blir tomma platser, och cachens `ptCount` (som
//! kan vara större än antalet punkter) spelar ingen roll. Etiketterna står på var `c:majorUnit`:e plats
//! (i `c:majorTimeUnit`), i axelns talformat eller cachens (`sourceLinked`), med Excels korta datumformat
//! i diagrammets språk. En `c:catAx` är alltid en textaxel (Office skriver `dateAx` när axeln är en
//! datumaxel). Undermodul till `chart_draw.rs`.
//!
//! Platser utan värde (dagar som inte finns i datan) överbryggas av linjer (`blanks = span`).
//!
//! `remap` gör om diagrammet till ett vanligt kategoridiagram med en kategori per tidsenhet, så att
//! resten av ritningen (staplar, linjer, etiketter, trendlinjer) är oförändrad.

use super::super::chart_fmt;
use super::super::chart_model::{Axis, Chart, Kind};

/// Tidsenheten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Days,
    Months,
    Years,
}

impl Unit {
    fn parse(s: Option<&str>) -> Unit {
        match s {
            Some("months") => Unit::Months,
            Some("years") => Unit::Years,
            _ => Unit::Days,
        }
    }

    /// Serienummer → platsnummer i enheten.
    pub fn key(self, serial: f64) -> i64 {
        match self {
            Unit::Days => serial.floor() as i64,
            Unit::Months => {
                let (y, m, _, _) = chart_fmt::civil(serial);
                y * 12 + m as i64 - 1
            }
            Unit::Years => chart_fmt::civil(serial).0,
        }
    }

    /// Platsnummer → serienummer (första dagen i perioden).
    pub fn serial(self, key: i64) -> f64 {
        match self {
            Unit::Days => key as f64,
            Unit::Months => serial_of(key.div_euclid(12), key.rem_euclid(12) as u32 + 1, 1),
            Unit::Years => serial_of(key, 1, 1),
        }
    }

    /// Ungefärligt antal enheter av `self` i en enhet `other` (för `majorTimeUnit`).
    fn per(self, other: Unit) -> f64 {
        let days = |u: Unit| match u {
            Unit::Days => 1.0,
            Unit::Months => 30.4375,
            Unit::Years => 365.25,
        };
        days(other) / days(self)
    }
}

/// Datum → serienummer (dagar sedan 1899-12-30).
fn serial_of(y: i64, m: u32, d: u32) -> f64 {
    let (y, m) = if m <= 2 { (y - 1, m as i64 + 9) } else { (y, m as i64 - 3) };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * m + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_1970 = era * 146097 + doe - 719468;
    (days_1970 + 25569) as f64
}

/// Ett diagram med datumaxel omgjort till en kategori per tidsenhet, och steget mellan etiketterna
/// (`None` = automatiskt). `None` om första plottens kategoriaxel inte är en datumaxel med datum.
pub(super) fn remap<'n>(c: &Chart<'n>) -> Option<(Chart<'n>, Option<usize>)> {
    let first = c.plots.first()?;
    if matches!(first.kind, Kind::Scatter | Kind::Pie { .. } | Kind::Radar { .. }) {
        return None;
    }
    let ax: &Axis<'_> = first.ax_ids.iter().filter_map(|id| c.axes.iter().find(|a| a.id == *id)).find(|a| a.kind == "dateAx")?;
    let unit = Unit::parse(ax.base_time_unit.as_deref());
    let keys: Vec<i64> = c.plots.iter().flat_map(|p| p.series.iter()).flat_map(|s| s.xvals.iter().flatten()).map(|v| unit.key(*v)).collect();
    let (lo, hi) = (*keys.iter().min()?, *keys.iter().max()?);
    let n = (hi - lo + 1).clamp(1, 20_000) as usize;
    // Talformatet: axelns, eller cachens när det är länkat; Excels korta datum i diagrammets språk.
    let code = match (&ax.num_fmt, ax.source_linked) {
        (Some(f), false) => f.clone(),
        _ => c.plots.iter().flat_map(|p| p.series.iter()).find_map(|s| s.cat_format.clone()).or_else(|| ax.num_fmt.clone()).unwrap_or_else(|| "d.m.yyyy".into()),
    };
    let code = chart_fmt::localized_short_date(&code, c.lang.as_deref());
    let labels: Vec<String> = (0..n).map(|k| chart_fmt::format(unit.serial(lo + k as i64), &code)).collect();
    let mut out = c.clone();
    // Dagar utan värde är inga tomma celler: linjen går över dem (som i PowerPoint).
    out.blanks = "span".into();
    for p in &mut out.plots {
        for s in &mut p.series {
            let slot = |i: usize| -> Option<usize> {
                let v = s.xvals.get(i).copied().flatten()?;
                let k = unit.key(v) - lo;
                (0..n as i64).contains(&k).then_some(k as usize)
            };
            let map_vec = |v: &Vec<Option<f64>>| -> Vec<Option<f64>> {
                let mut o = vec![None; n];
                for (i, x) in v.iter().enumerate() {
                    if let (Some(k), Some(x)) = (slot(i), x) {
                        o[k] = Some(*x);
                    }
                }
                o
            };
            let vals = map_vec(&s.vals);
            let err: Vec<(Vec<Option<f64>>, Vec<Option<f64>>)> = s.err_bars.iter().map(|e| (map_vec(&e.plus), map_vec(&e.minus))).collect();
            let points = s.points.iter().filter_map(|d| Some(super::super::chart_model::DataPoint { idx: slot(d.idx)?, ..d.clone() })).collect();
            let point_labels = s.point_labels.iter().filter_map(|(i, l)| Some((slot(*i)?, l.clone()))).collect();
            s.vals = vals;
            for (e, (plus, minus)) in s.err_bars.iter_mut().zip(err) {
                e.plus = plus;
                e.minus = minus;
            }
            s.points = points;
            s.point_labels = point_labels;
            s.cats = labels.clone();
            s.xvals = (0..n).map(|k| Some(unit.serial(lo + k as i64))).collect();
        }
    }
    let step = ax.major.map(|m| {
        let mu = Unit::parse(ax.major_time_unit.as_deref().or(ax.base_time_unit.as_deref()));
        (m * unit.per(mu)).round().max(1.0) as usize
    });
    Some((out, step))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enheter_och_serienummer() {
        // 44076 = 2020-09-02.
        assert_eq!(chart_fmt::civil(44076.0).0, 2020);
        assert_eq!(serial_of(2020, 9, 2), 44076.0);
        assert_eq!(Unit::Days.key(44076.7), 44076);
        let m = Unit::Months.key(44076.0);
        assert_eq!(Unit::Months.serial(m), serial_of(2020, 9, 1));
        assert_eq!(Unit::Months.key(serial_of(2021, 1, 31)) - m, 4);
        assert_eq!(Unit::Years.serial(Unit::Years.key(44076.0)), serial_of(2020, 1, 1));
        assert_eq!((3.0 * Unit::Days.per(Unit::Days)) as usize, 3);
        assert!((Unit::Months.per(Unit::Years) - 12.0).abs() < 1e-9);
    }
}
