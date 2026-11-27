//! Diagrammodellen: `c:chartSpace` → typer, serier (ur de cachade värdena `c:numCache`/`c:strCache`),
//! axlar, förklaring, titel och manuella layouter. Bara läsning; ritningen görs i `chart_draw.rs`.
//!
//! Modellen lånar noderna ur diagramdelens träd (`'n`), så formateringen (`c:spPr`, `c:txPr`) kan
//! lösas upp med samma fyllnings- och textkod som formerna.

use super::xml::Node;

/// Diagramtyp för en plott (ett `c:*Chart`-element i `c:plotArea`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Stapel (`col` = lodräta staplar, annars liggande).
    Bar { horizontal: bool },
    Line,
    Area,
    /// Cirkel/ring: `hole` är hålets storlek som andel av radien (0 = cirkel).
    Pie { hole: f32, three_d: bool },
    Scatter,
}

/// Gruppering för stapel/linje/yta.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grouping {
    Clustered,
    Stacked,
    Percent,
}

/// En markör (`c:marker`).
#[derive(Clone, Debug)]
pub struct Marker<'n> {
    pub symbol: String,
    pub size: f32,
    pub sppr: Option<&'n Node>,
}

/// En datapunkt med egen formatering (`c:dPt`).
#[derive(Clone, Debug)]
pub struct DataPoint<'n> {
    pub idx: usize,
    pub sppr: Option<&'n Node>,
    pub explosion: Option<f32>,
}

/// Dataetiketter (`c:dLbls`/`c:dLbl`).
#[derive(Clone, Debug, Default)]
pub struct Labels<'n> {
    pub show_val: bool,
    pub show_cat: bool,
    pub show_ser: bool,
    pub show_pct: bool,
    pub pos: Option<String>,
    pub sppr: Option<&'n Node>,
    pub txpr: Option<&'n Node>,
    pub num_fmt: Option<String>,
    pub separator: Option<String>,
    /// Egen text (`c:tx/c:rich`); fälten (kategorinamn, procent …) fylls i vid ritningen.
    pub rich: Option<&'n Node>,
    pub deleted: bool,
}

impl Labels<'_> {
    pub fn any(&self) -> bool {
        !self.deleted && (self.show_val || self.show_cat || self.show_ser || self.show_pct || self.rich.is_some())
    }
}

/// En serie.
#[derive(Clone, Debug)]
pub struct Series<'n> {
    pub idx: usize,
    pub order: usize,
    pub name: String,
    pub cats: Vec<String>,
    /// Kategorierna som tal (punktdiagrammets x-värden).
    pub xvals: Vec<Option<f64>>,
    pub vals: Vec<Option<f64>>,
    pub format: String,
    pub sppr: Option<&'n Node>,
    pub marker: Option<Marker<'n>>,
    pub points: Vec<DataPoint<'n>>,
    pub explosion: f32,
    pub smooth: bool,
    pub labels: Labels<'n>,
    /// Etiketter för enskilda punkter (`c:dLbl`).
    pub point_labels: Vec<(usize, Labels<'n>)>,
}

impl<'n> Series<'n> {
    pub fn point(&self, i: usize) -> Option<&DataPoint<'n>> {
        self.points.iter().find(|p| p.idx == i)
    }

    pub fn labels_for(&self, i: usize) -> &Labels<'n> {
        self.point_labels.iter().find(|(k, _)| *k == i).map_or(&self.labels, |(_, l)| l)
    }
}

/// En plott: en diagramtyp med sina serier.
#[derive(Clone, Debug)]
pub struct Plot<'n> {
    pub kind: Kind,
    pub grouping: Grouping,
    pub vary_colors: bool,
    pub series: Vec<Series<'n>>,
    /// Mellanrum mellan kategorier i procent av en stapels bredd (standard 150).
    pub gap: f32,
    /// Överlappning mellan staplar i procent (negativt = mellanrum).
    pub overlap: f32,
    /// Första sektorns vinkel i grader (medurs från klockan 12).
    pub first_angle: f32,
    /// Linjediagram: markörer som standard (`c:marker val="1"`).
    pub markers: bool,
    pub ax_ids: Vec<String>,
    /// 3D-typ (`c:bar3DChart`, `c:line3DChart`, `c:area3DChart`; cirkeln har det i `Kind::Pie`).
    pub three_d: bool,
    /// Staplarnas form i 3D (`c:shape`: `box`, `cylinder`, `cone`, `coneToMax`, `pyramid`, `pyramidToMax`).
    pub shape: String,
    /// Mellanrum mellan serierna på djupet i procent (`c:gapDepth`, standard 150).
    pub gap_depth: f32,
}

/// En axel.
#[derive(Clone, Debug)]
pub struct Axis<'n> {
    pub id: String,
    /// `catAx`, `valAx`, `dateAx`, `serAx`.
    pub kind: String,
    pub pos: String,
    pub deleted: bool,
    pub reversed: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub major: Option<f64>,
    pub num_fmt: Option<String>,
    pub source_linked: bool,
    pub major_grid: Option<Option<&'n Node>>,
    pub minor_grid: Option<Option<&'n Node>>,
    pub tick_lbl_pos: String,
    pub sppr: Option<&'n Node>,
    pub txpr: Option<&'n Node>,
    pub title: Option<Title<'n>>,
    /// `between` (kategorierna mitt i sina band) eller `midCat` (på strecken).
    pub cross_between: Option<String>,
    pub crosses: String,
}

/// Manuell layout (`c:manualLayout`), andelar av diagrammets yta.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Manual {
    pub inner: bool,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub w: Option<f32>,
    pub h: Option<f32>,
    /// `xMode`/`yMode` = `factor`: x och y är förskjutningar från standardläget.
    pub x_factor: bool,
    pub y_factor: bool,
}

/// En titel.
#[derive(Clone, Debug)]
pub struct Title<'n> {
    /// `c:tx/c:rich` (stycken) eller ingen (automatisk text).
    pub rich: Option<&'n Node>,
    /// Text ur `c:tx/c:strRef`.
    pub text: Option<String>,
    pub txpr: Option<&'n Node>,
    pub sppr: Option<&'n Node>,
    pub layout: Option<Manual>,
    pub overlay: bool,
}

/// Förklaringen.
#[derive(Clone, Debug)]
pub struct Legend<'n> {
    pub pos: String,
    pub layout: Option<Manual>,
    pub overlay: bool,
    pub sppr: Option<&'n Node>,
    pub txpr: Option<&'n Node>,
    /// Borttagna poster (`c:legendEntry/c:delete`).
    pub deleted: Vec<usize>,
}

/// `c:view3D`. Saknade värden är `None` (standardvärdena beror på diagramtypen).
#[derive(Clone, Copy, Debug, Default)]
pub struct View3D {
    /// Lutning i grader (`rotX`).
    pub rot_x: Option<f32>,
    /// Vridning i grader (`rotY`).
    pub rot_y: Option<f32>,
    /// Djup i procent av basen (`depthPercent`).
    pub depth: f32,
    /// Höjd i procent av basen (`hPercent`).
    pub h_percent: Option<f32>,
    /// Rätvinkliga axlar (`rAngAx`): parallellprojektion med lutning.
    pub right_angle: Option<bool>,
    /// Perspektiv (`perspective`, 0–240; 30 som standard).
    pub perspective: Option<f32>,
}

/// Golv och väggar i ett 3D-diagram (`c:floor`, `c:sideWall`, `c:backWall`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Walls<'n> {
    pub floor: Option<&'n Node>,
    pub side: Option<&'n Node>,
    pub back: Option<&'n Node>,
}

/// Hela diagrammet.
#[derive(Clone, Debug)]
pub struct Chart<'n> {
    pub style: i64,
    pub title: Option<Title<'n>>,
    pub plots: Vec<Plot<'n>>,
    pub axes: Vec<Axis<'n>>,
    pub legend: Option<Legend<'n>>,
    pub plot_layout: Option<Manual>,
    pub space_sppr: Option<&'n Node>,
    pub space_txpr: Option<&'n Node>,
    pub plot_sppr: Option<&'n Node>,
    pub view3d: Option<View3D>,
    /// Golvets och väggarnas `c:spPr`.
    pub walls: Walls<'n>,
}

fn val<'a>(n: &'a Node, child: &str) -> Option<&'a str> {
    n.child(child).and_then(|c| c.attr("val"))
}

fn flag(n: &Node, child: &str, default: bool) -> bool {
    match n.child(child) {
        Some(c) => c.attr("val").is_none_or(|v| v == "1" || v == "true"),
        None => default,
    }
}

fn num(n: &Node, child: &str) -> Option<f64> {
    n.child(child).and_then(|c| c.attr_f64("val")).filter(|v| v.is_finite())
}

/// Punkterna i en cache (`c:numCache`/`c:strCache`/`c:numLit`/`c:strLit`/`c:multiLvlStrCache`) som text.
fn cache_strings(n: &Node) -> (Vec<String>, Option<String>) {
    let cache = ["c:numRef", "c:strRef", "c:multiLvlStrRef"]
        .iter()
        .find_map(|r| n.child(r))
        .and_then(|r| ["c:numCache", "c:strCache", "c:multiLvlStrCache"].iter().find_map(|c| r.child(c)))
        .or_else(|| ["c:numLit", "c:strLit"].iter().find_map(|c| n.child(c)));
    let Some(cache) = cache else { return (Vec::new(), None) };
    // Flernivåkategorier: den innersta nivån (första `c:lvl`).
    let pts_parent = cache.child("c:lvl").unwrap_or(cache);
    let count = cache.child("c:ptCount").and_then(|c| c.attr_i64("val")).unwrap_or(0).clamp(0, 100_000) as usize;
    let mut out = vec![String::new(); count];
    for pt in pts_parent.children_named("c:pt") {
        let i = pt.attr_i64("idx").unwrap_or(-1);
        if i < 0 || i >= 100_000 {
            continue;
        }
        let i = i as usize;
        if i >= out.len() {
            out.resize(i + 1, String::new());
        }
        out[i] = pt.child("c:v").map(|v| v.text.clone()).unwrap_or_default();
    }
    let fmt = cache.child("c:formatCode").map(|f| f.text.clone()).filter(|s| !s.is_empty());
    (out, fmt)
}

fn parse_num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn manual(n: Option<&Node>) -> Option<Manual> {
    let m = n?.child("c:manualLayout")?;
    let f = |k: &str| num(m, k).map(|v| v as f32);
    Some(Manual {
        inner: val(m, "c:layoutTarget") == Some("inner"),
        x: f("c:x"),
        y: f("c:y"),
        w: f("c:w"),
        h: f("c:h"),
        x_factor: val(m, "c:xMode") == Some("factor"),
        y_factor: val(m, "c:yMode") == Some("factor"),
    })
}

fn title(n: &Node) -> Title<'_> {
    let tx = n.child("c:tx");
    Title {
        rich: tx.and_then(|t| t.child("c:rich")),
        text: tx.filter(|t| t.child("c:rich").is_none()).map(|t| cache_strings(t).0.join(" ")).filter(|s| !s.is_empty()),
        txpr: n.child("c:txPr"),
        sppr: n.child("c:spPr"),
        layout: manual(n.child("c:layout")),
        overlay: flag(n, "c:overlay", false),
    }
}

fn labels(n: &Node) -> Labels<'_> {
    let rich = n.path(&["c:tx", "c:rich"]);
    Labels {
        show_val: flag(n, "c:showVal", false),
        show_cat: flag(n, "c:showCatName", false),
        show_ser: flag(n, "c:showSerName", false),
        show_pct: flag(n, "c:showPercent", false),
        pos: val(n, "c:dLblPos").map(str::to_owned),
        sppr: n.child("c:spPr"),
        txpr: n.child("c:txPr"),
        num_fmt: n.child("c:numFmt").filter(|f| f.attr("sourceLinked") != Some("1")).and_then(|f| f.attr("formatCode")).map(str::to_owned),
        separator: n.child("c:separator").map(|s| s.text.clone()),
        rich,
        deleted: flag(n, "c:delete", false),
    }
}

/// Ärver de inställningar som en punkts etikett inte anger från seriens.
fn merge_labels<'n>(own: &'n Node, base: &Labels<'n>) -> Labels<'n> {
    let mut l = labels(own);
    if own.child("c:showVal").is_none() {
        l.show_val = base.show_val;
        l.show_cat = base.show_cat;
        l.show_ser = base.show_ser;
        l.show_pct = base.show_pct;
    }
    l.pos = l.pos.or_else(|| base.pos.clone());
    l.sppr = l.sppr.or(base.sppr);
    l.txpr = l.txpr.or(base.txpr);
    l.num_fmt = l.num_fmt.or_else(|| base.num_fmt.clone());
    l.separator = l.separator.or_else(|| base.separator.clone());
    l
}

fn series<'n>(s: &'n Node, plot_labels: &Labels<'n>) -> Series<'n> {
    let idx = s.child("c:idx").and_then(|c| c.attr_i64("val")).unwrap_or(0).clamp(0, 10_000) as usize;
    let order = s.child("c:order").and_then(|c| c.attr_i64("val")).unwrap_or(idx as i64).clamp(0, 10_000) as usize;
    let name = s.child("c:tx").map(|t| {
        let (v, _) = cache_strings(t);
        if v.is_empty() { t.child("c:v").map(|v| v.text.clone()).unwrap_or_default() } else { v.join(" ") }
    });
    let cat_node = s.child("c:cat").or_else(|| s.child("c:xVal"));
    let (cats, cat_fmt) = cat_node.map(cache_strings).unwrap_or_default();
    let val_node = s.child("c:val").or_else(|| s.child("c:yVal"));
    let (vals_s, fmt) = val_node.map(cache_strings).unwrap_or_default();
    // Kategorier som är tal med talformat (t.ex. datum) visas formaterade.
    let cats_fmt: Vec<String> = match &cat_fmt {
        Some(f) if !f.eq_ignore_ascii_case("general") => cats.iter().map(|c| parse_num(c).map_or_else(|| c.clone(), |v| super::chart_fmt::format(v, f))).collect(),
        _ => cats.clone(),
    };
    let base = match s.child("c:dLbls") {
        Some(d) => merge_labels(d, plot_labels),
        None => plot_labels.clone(),
    };
    let point_labels = s.child("c:dLbls").map_or_else(Vec::new, |d| {
        d.children_named("c:dLbl").filter_map(|l| Some((l.child("c:idx")?.attr_i64("val")?.clamp(0, 100_000) as usize, merge_labels(l, &base)))).collect()
    });
    Series {
        idx,
        order,
        name: name.unwrap_or_else(|| format!("{} {}", crate::i18n::t("Serie"), idx + 1)),
        xvals: cats.iter().map(|c| parse_num(c)).collect(),
        cats: cats_fmt,
        vals: vals_s.iter().map(|v| parse_num(v)).collect(),
        format: fmt.unwrap_or_else(|| "General".into()),
        sppr: s.child("c:spPr"),
        marker: s.child("c:marker").map(|m| Marker {
            symbol: val(m, "c:symbol").unwrap_or("auto").to_owned(),
            size: num(m, "c:size").unwrap_or(5.0).clamp(2.0, 72.0) as f32,
            sppr: m.child("c:spPr"),
        }),
        points: s
            .children_named("c:dPt")
            .map(|p| DataPoint {
                idx: p.child("c:idx").and_then(|c| c.attr_i64("val")).unwrap_or(0).clamp(0, 100_000) as usize,
                sppr: p.child("c:spPr"),
                explosion: num(p, "c:explosion").map(|v| v as f32),
            })
            .collect(),
        explosion: num(s, "c:explosion").unwrap_or(0.0) as f32,
        smooth: flag(s, "c:smooth", false),
        labels: base,
        point_labels,
    }
}

fn axis(a: &Node) -> Axis<'_> {
    let scaling = a.child("c:scaling");
    let nf = a.child("c:numFmt");
    Axis {
        id: val(a, "c:axId").unwrap_or("").to_owned(),
        kind: a.local().to_owned(),
        pos: val(a, "c:axPos").unwrap_or("b").to_owned(),
        deleted: flag(a, "c:delete", false),
        reversed: scaling.and_then(|s| val(s, "c:orientation")) == Some("maxMin"),
        min: scaling.and_then(|s| num(s, "c:min")),
        max: scaling.and_then(|s| num(s, "c:max")),
        major: num(a, "c:majorUnit").filter(|v| *v > 0.0),
        num_fmt: nf.and_then(|f| f.attr("formatCode")).map(str::to_owned),
        source_linked: nf.is_none_or(|f| f.attr("sourceLinked") == Some("1")),
        major_grid: a.child("c:majorGridlines").map(|g| g.child("c:spPr")),
        minor_grid: a.child("c:minorGridlines").map(|g| g.child("c:spPr")),
        tick_lbl_pos: val(a, "c:tickLblPos").unwrap_or("nextTo").to_owned(),
        sppr: a.child("c:spPr"),
        txpr: a.child("c:txPr"),
        title: a.child("c:title").map(title),
        cross_between: val(a, "c:crossBetween").map(str::to_owned),
        crosses: val(a, "c:crosses").unwrap_or("autoZero").to_owned(),
    }
}

fn plot(n: &Node) -> Option<Plot<'_>> {
    let kind = match n.local() {
        "barChart" | "bar3DChart" => Kind::Bar { horizontal: val(n, "c:barDir") == Some("bar") },
        "lineChart" | "line3DChart" | "stockChart" => Kind::Line,
        "areaChart" | "area3DChart" => Kind::Area,
        "pieChart" | "ofPieChart" => Kind::Pie { hole: 0.0, three_d: false },
        "pie3DChart" => Kind::Pie { hole: 0.0, three_d: true },
        "doughnutChart" => Kind::Pie { hole: num(n, "c:holeSize").unwrap_or(50.0).clamp(10.0, 90.0) as f32 / 100.0, three_d: false },
        "scatterChart" | "bubbleChart" => Kind::Scatter,
        _ => return None,
    };
    let grouping = match val(n, "c:grouping") {
        Some("stacked") => Grouping::Stacked,
        Some("percentStacked") => Grouping::Percent,
        _ => Grouping::Clustered,
    };
    let plot_labels = n.child("c:dLbls").map(labels).unwrap_or_default();
    let mut series: Vec<Series<'_>> = n.children_named("c:ser").map(|s| series(s, &plot_labels)).collect();
    series.sort_by_key(|s| s.order);
    let pie = matches!(kind, Kind::Pie { .. });
    Some(Plot {
        kind,
        grouping,
        vary_colors: flag(n, "c:varyColors", pie),
        series,
        gap: num(n, "c:gapWidth").unwrap_or(150.0).clamp(0.0, 500.0) as f32,
        overlap: num(n, "c:overlap").unwrap_or(if grouping == Grouping::Clustered { 0.0 } else { 100.0 }).clamp(-100.0, 100.0) as f32,
        first_angle: num(n, "c:firstSliceAng").unwrap_or(0.0) as f32,
        markers: flag(n, "c:marker", true),
        ax_ids: n.children_named("c:axId").filter_map(|a| a.attr("val")).map(str::to_owned).collect(),
        three_d: matches!(n.local(), "bar3DChart" | "line3DChart" | "area3DChart"),
        shape: val(n, "c:shape").unwrap_or("box").to_owned(),
        gap_depth: num(n, "c:gapDepth").unwrap_or(150.0).clamp(0.0, 500.0) as f32,
    })
}

/// Läser `c:chartSpace`. `None` om det inte finns något ritbart diagram.
pub fn parse(root: &Node) -> Option<Chart<'_>> {
    let chart = root.child("c:chart")?;
    let pa = chart.child("c:plotArea")?;
    let plots: Vec<Plot<'_>> = pa.children.iter().filter_map(plot).collect();
    if plots.is_empty() {
        return None;
    }
    let axes = pa.children.iter().filter(|c| matches!(c.local(), "catAx" | "valAx" | "dateAx" | "serAx")).map(axis).collect();
    let legend = chart.child("c:legend").map(|l| Legend {
        pos: val(l, "c:legendPos").unwrap_or("r").to_owned(),
        layout: manual(l.child("c:layout")),
        overlay: flag(l, "c:overlay", false),
        sppr: l.child("c:spPr"),
        txpr: l.child("c:txPr"),
        deleted: l
            .children_named("c:legendEntry")
            .filter(|e| flag(e, "c:delete", false))
            .filter_map(|e| e.child("c:idx").and_then(|i| i.attr_i64("val")))
            .map(|i| i.clamp(0, 100_000) as usize)
            .collect(),
    });
    Some(Chart {
        style: val(root, "c:style").and_then(|s| s.parse().ok()).unwrap_or(2),
        title: chart.child("c:title").map(title),
        plots,
        axes,
        legend,
        plot_layout: manual(pa.child("c:layout")),
        space_sppr: root.child("c:spPr"),
        space_txpr: root.child("c:txPr"),
        plot_sppr: pa.child("c:spPr"),
        view3d: chart.child("c:view3D").map(|v| View3D {
            rot_x: num(v, "c:rotX").map(|x| x as f32),
            rot_y: num(v, "c:rotY").map(|x| x as f32),
            depth: num(v, "c:depthPercent").unwrap_or(100.0).clamp(20.0, 2000.0) as f32,
            h_percent: num(v, "c:hPercent").map(|x| x.clamp(5.0, 500.0) as f32),
            right_angle: v.child("c:rAngAx").map(|_| flag(v, "c:rAngAx", true)),
            perspective: num(v, "c:perspective").map(|x| x.clamp(0.0, 240.0) as f32),
        }),
        walls: Walls { floor: chart.path(&["c:floor", "c:spPr"]), side: chart.path(&["c:sideWall", "c:spPr"]), back: chart.path(&["c:backWall", "c:spPr"]) },
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Ett minimalt stapeldiagram (två serier, tre kategorier).
    pub const BAR: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/>
<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>
<c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Äpplen</c:v></c:pt></c:strCache></c:strRef></c:tx>
<c:cat><c:strRef><c:strCache><c:ptCount val="3"/><c:pt idx="0"><c:v>A</c:v></c:pt><c:pt idx="1"><c:v>B</c:v></c:pt><c:pt idx="2"><c:v>C</c:v></c:pt></c:strCache></c:strRef></c:cat>
<c:val><c:numRef><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="3"/><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="1"><c:v>2</c:v></c:pt><c:pt idx="2"><c:v>4</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>
<c:ser><c:idx val="1"/><c:order val="1"/><c:tx><c:v>Päron</c:v></c:tx><c:spPr><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></c:spPr>
<c:val><c:numLit><c:ptCount val="3"/><c:pt idx="0"><c:v>3</c:v></c:pt><c:pt idx="2"><c:v>1.5</c:v></c:pt></c:numLit></c:val></c:ser>
<c:gapWidth val="100"/><c:axId val="1"/><c:axId val="2"/></c:barChart>
<c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/></c:catAx>
<c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:numFmt formatCode="0.0" sourceLinked="0"/><c:crossAx val="1"/></c:valAx>
</c:plotArea><c:legend><c:legendPos val="b"/></c:legend></c:chart></c:chartSpace>"#;

    #[test]
    fn laser_serier_ur_cachen() {
        let root = crate::pptx::xml::parse(BAR.as_bytes()).unwrap();
        let c = parse(&root).expect("diagram");
        assert_eq!(c.plots.len(), 1);
        let p = &c.plots[0];
        assert_eq!(p.kind, Kind::Bar { horizontal: false });
        assert_eq!(p.grouping, Grouping::Clustered);
        assert_eq!(p.gap, 100.0);
        assert_eq!(p.series[0].name, "Äpplen");
        assert_eq!(p.series[0].cats, ["A", "B", "C"]);
        assert_eq!(p.series[0].vals, [Some(1.0), Some(2.0), Some(4.0)]);
        // Saknad punkt i en literal är tom.
        assert_eq!(p.series[1].name, "Päron");
        assert_eq!(p.series[1].vals, [Some(3.0), None, Some(1.5)]);
        assert!(p.series[1].sppr.is_some());
        assert_eq!(c.axes.len(), 2);
        assert_eq!(c.axes[1].num_fmt.as_deref(), Some("0.0"));
        assert!(!c.axes[1].source_linked);
        assert!(c.axes[1].major_grid.is_some());
        assert_eq!(c.legend.as_ref().unwrap().pos, "b");
    }

    #[test]
    fn okand_typ_ger_inget() {
        let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:radarChart/></c:plotArea></c:chart></c:chartSpace>"#;
        let root = crate::pptx::xml::parse(xml.as_bytes()).unwrap();
        assert!(parse(&root).is_none());
    }
}
