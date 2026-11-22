//! CIDER geometry and silicon material binding.
//! Provenance: ngspice 44 manual §§26.1–26.19 and Abramowitz & Stegun 7.1.26.

#[path = "plane.rs"]
mod plane;
pub(super) use plane::{Plane, Surface, SurfaceTrap};
use sansho_diag::Diagnostic;
use sansho_netlist::{ModelCard, parse_number};
use std::collections::BTreeMap;

pub(super) fn invalid(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-CIDER-MODEL", msg)
}

#[derive(Debug, Default)]
pub(super) struct Fields {
    values: BTreeMap<String, f64>,
    domains: Vec<u32>,
}

impl Fields {
    // The universal silicon set is active before the first Init. AR and CT
    // select the same concentration law with different initialization data.
    fn bulk(carrier: usize, minority: usize, initialized_model: Option<f64>) -> Self {
        let values = match (carrier, minority, initialized_model) {
            (0, 0, Some(0.)) => [92., 1360., 1.3e17, 0.91],
            (0, 0, _) => [88., 1340., 1.26e17, 0.88],
            (1, 1, _) => [130., 500., 8e17, 1.25],
            (0, _, _) => [232., 1412., 1.26e17, 0.88],
            (_, _, _) => [54.3, 461.3, 2.35e17, 0.88],
        };
        let mut fields = Fields::default();
        for (key, value) in ["mumin", "mumax", "ntref", "ntexp"].into_iter().zip(values) {
            fields.values.insert(key.into(), value);
        }
        fields
            .values
            .insert("concmodel".into(), initialized_model.unwrap_or(1.));
        fields
    }
    pub(super) fn get(&self, name: &str, default: f64) -> f64 {
        self.values.get(name).copied().unwrap_or(default)
    }
    fn flag(&self, name: &str) -> bool {
        self.get(name, 0.) != 0.
    }
    fn positive(&self, name: &str, default: f64) -> Result<f64, Diagnostic> {
        let x = self.get(name, default);
        (x > 0. && x.is_finite())
            .then_some(x)
            .ok_or_else(|| invalid(format!("{name} must be positive and finite")))
    }
}

// Each group has one canonical spelling followed by interchangeable input spellings.
fn keyword<'a>(word: &str, groups: &'a str) -> Option<&'a str> {
    let mut matching = groups
        .split_whitespace()
        .filter(|group| group.split('|').any(|k| k.starts_with(word)));
    let exact = groups
        .split_whitespace()
        .find(|group| group.split('|').any(|k| k == word));
    exact
        .or_else(|| {
            let first = matching.next()?;
            matching.next().is_none().then_some(first)
        })
        .and_then(|s| s.split('|').next())
}

fn fields(words: &[String], numbers: &str, flags: &str) -> Result<Fields, Diagnostic> {
    let mut f = Fields::default();
    let mut i = 1;
    while let Some(word) = words.get(i) {
        i += 1;
        let raw = word.trim_start_matches('^');
        let eq = words.get(i).is_some_and(|w| w == "=");
        i += usize::from(eq);
        let (key, value) = if let Some(key) = keyword(raw, flags) {
            if eq {
                return Err(invalid(format!("expected a flag for {key}")));
            }
            (key, f64::from(!word.starts_with('^')))
        } else if let Some(key) = keyword(raw, numbers) {
            if word.starts_with('^') {
                return Err(invalid("only flags may be negated"));
            }
            if key == "domains" {
                let start = i;
                while let Some(v) = words.get(i).and_then(|w| parse_number(w)) {
                    f.domains.push(identifier(v, "domains")?);
                    i += 1;
                }
                if i == start {
                    return Err(invalid("empty doping domain list"));
                }
                continue;
            }
            let raw_value = words.get(i).map(String::as_str);
            let value = match key {
                "concmodel" | "fieldmodel" => match raw_value {
                    Some("ar") => Some(1.),
                    Some("ct") => Some(0.),
                    Some("sg") if key == "fieldmodel" => Some(2.),
                    _ => {
                        return Err(Diagnostic::error(
                            "E-UNSUPPORTED",
                            format!("unsupported {key} {}", raw_value.unwrap_or("")),
                        ));
                    }
                },
                _ => raw_value.and_then(parse_number),
            }
            .filter(|v| v.is_finite())
            .ok_or_else(|| invalid(format!("expected a finite value for {word}")))?;
            i += 1;
            (key, value)
        } else {
            return Err(Diagnostic::error(
                "E-UNSUPPORTED",
                format!("unsupported or ambiguous {} parameter {word}", words[0]),
            ));
        };
        f.values.insert(key.into(), value);
    }
    Ok(f)
}

const MAX_NODES: usize = 4096;
fn mesh_interval(x: &mut Vec<f64>, f: &Fields) -> Result<(), Diagnostic> {
    let width = f.values.get("width");
    if width.is_some() == f.values.contains_key("location") {
        return Err(invalid("mesh needs either width or location"));
    }
    if x.is_empty() && width.is_none() {
        if f.get("number", 1.) != 1. {
            return Err(invalid("first mesh index must be one"));
        }
        x.push(f.get("location", 0.));
        return Ok(());
    }
    if x.is_empty() {
        x.push(0.)
    }
    let start = x[x.len() - 1];
    let stop = width.map_or_else(|| f.get("location", start), |w| start + w);
    let length = stop - start;
    if length <= 0. || !length.is_finite() {
        return Err(invalid("mesh positions must increase"));
    }
    let capacity = MAX_NODES - x.len();
    if let Some(&num) = f.values.get("number") {
        let count = num - if width.is_some() { 0. } else { x.len() as f64 };
        if count.fract() != 0. || count < 1. || count > capacity as f64 || f.values.len() != 2 {
            return Err(invalid(
                "invalid uniform mesh interval or node limit exceeded",
            ));
        }
        for j in 1..=count as usize {
            x.push(start + length * (j as f64 / count))
        }
    } else {
        let ratio = f.positive("ratio", 1.)?;
        let a = f.positive("h.start", f64::MAX)?;
        let b = f.positive("h.end", f64::MAX)?;
        let max = f.positive("h.max", length)?;
        if ratio < 1. || a.min(b) == f64::MAX {
            return Err(invalid("graded mesh needs an end spacing and ratio >= 1"));
        }
        let space = |i: usize, n: usize| {
            max.min(a * ratio.powi(i as i32))
                .min(b * ratio.powi((n - 1 - i) as i32))
        };
        let extent = |n| (0..n).map(|i| space(i, n)).sum::<f64>();
        if capacity == 0 || extent(capacity) < length {
            return Err(invalid("mesh exceeds 4096 nodes"));
        }
        let (mut lo, mut hi) = (1, capacity);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if extent(mid) < length {
                lo = mid + 1
            } else {
                hi = mid
            }
        }
        let scale = length / extent(lo);
        let mut pos = start;
        for i in 0..lo {
            pos += scale * space(i, lo);
            x.push(pos)
        }
    }
    if let Some(end) = x.last_mut() {
        *end = stop
    }
    Ok(())
}

fn bounds(f: &Fields, x: &[f64], point: bool, y_axis: bool) -> Result<(f64, f64), Diagnostic> {
    let bound = |coord, idx| -> Result<Option<f64>, Diagnostic> {
        match (f.values.get(coord), f.values.get(idx)) {
            (Some(_), Some(_)) => Err(invalid("duplicate mesh bound")),
            (_, Some(&i)) if i.fract() != 0. || i < 1. || i > x.len() as f64 => {
                Err(invalid("mesh index out of range"))
            }
            (_, Some(&i)) => Ok(Some(x[i as usize - 1])),
            (v, None) => Ok(v.map(|&value| {
                // A width-built mesh endpoint can differ from the equivalent
                // literal coordinate by one rounding unit (e.g. 0.08 + 0.72).
                [x[0], x[x.len() - 1]]
                    .into_iter()
                    .find(|end| {
                        (value - end).abs() <= 8. * f64::EPSILON * value.abs().max(end.abs())
                    })
                    .unwrap_or(value)
            })),
        }
    };
    let [lo, hi] = if y_axis {
        [bound("y.low", "iy.low")?, bound("y.high", "iy.high")?]
    } else {
        [bound("x.low", "ix.low")?, bound("x.high", "ix.high")?]
    };
    let lo = lo.unwrap_or(if point { hi.unwrap_or(x[0]) } else { x[0] });
    let hi = hi.unwrap_or(if point { lo } else { x[x.len() - 1] });
    if hi < lo {
        return Err(invalid("reversed spatial bounds"));
    }
    Ok((lo, hi))
}

// Abramowitz & Stegun 7.1.26, maximum absolute error 1.5e-7.
fn erfc(x: f64) -> f64 {
    let t = 1. / (1. + 0.3275911 * x.abs());
    let y = t
        * (0.254829592
            + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))))
        * (-x * x).exp();
    if x < 0. { 2. - y } else { y }
}

#[derive(Debug)]
pub(super) struct Structure {
    pub x: Vec<f64>,
    pub plane: Option<Plane>,
    pub frequency: f64,
    pub report_material: bool,
    /// Carrier (electron, hole) and scattering class (majority, minority).
    pub mobility: [[Fields; 2]; 2],
    pub surface_params: [[f64; 3]; 2],
    pub net: Vec<f64>,
    pub impurities: Vec<f64>,
    material: Fields,
    switches: Fields,
    options: Fields,
}
impl Structure {
    pub fn material(&self, k: &str, default: f64) -> f64 {
        self.material.get(k, default)
    }
    pub fn enabled(&self, k: &str) -> bool {
        self.switches.flag(k)
    }
    pub fn option(&self, k: &str, default: f64) -> f64 {
        self.options.get(k, default)
    }

    pub fn bind(card: &ModelCard, warnings: &mut Vec<Diagnostic>) -> Result<Self, Diagnostic> {
        let planar =
            card.kind == "numos" || card.params.iter().any(|(k, v)| k == "level" && *v == 2.);
        let mut x = Vec::new();
        let mut y = Vec::new();
        let mut electrodes = Vec::new();
        let mut interfaces = Vec::new();
        let mut contacts = Vec::new();
        let mut materials = BTreeMap::new();
        let mut domains = Vec::new();
        let mut profiles = Vec::new();
        let mut mobility_cards = Vec::new();
        let mut switches = Fields::default();
        let mut options = Fields::default();
        let mut frequency = 1.;
        let mut report_material = false;
        const BOX: &str = "x.low x.high ix.low ix.high y.low y.high iy.low iy.high";
        const LINE_CARDS: &str = "x.mesh material domain|region doping mobility models options method output comment title end";
        const PLANE_CARDS: &str = "x.mesh y.mesh electrode contact interface|boundary material domain|region doping mobility models options method output comment title end";
        for words in &card.cider {
            let Some(name) = words.first() else { continue };
            let warn = |warnings: &mut Vec<Diagnostic>| {
                warnings.push(
                    Diagnostic::warning(
                        "W-MODEL-IGNORED",
                        format!("{}: ignored {}", card.name, words.join(" ")),
                    )
                    .with_span(card.span),
                )
            };
            match keyword(name, if planar { PLANE_CARDS } else { LINE_CARDS }) {
                Some("end") => break,
                Some("title" | "comment") => {}
                _ if name.starts_with(['*', '#', '$']) => {}
                Some(axis @ ("x.mesh" | "y.mesh")) if planar || axis == "x.mesh" => {
                    let mesh = fields(
                        words,
                        "location width number|node h.start|h1 h.end|h2 h.max|h3 ratio",
                        "",
                    )?;
                    mesh_interval(if axis == "x.mesh" { &mut x } else { &mut y }, &mesh)?;
                }
                Some("electrode") if planar => {
                    electrodes.push(fields(words, &format!("number {BOX}"), "")?)
                }
                Some("contact") if planar => {
                    contacts.push(fields(words, "number work-function|workfunction", "")?)
                }
                Some("interface") if planar => interfaces.push(fields(
                    words,
                    &format!("domain neighbor qf sn sp layer.width {BOX}"),
                    "",
                )?),
                Some("domain") => domains.push(fields(
                    words,
                    if planar {
                        "number material x.low x.high ix.low ix.high y.low y.high iy.low iy.high"
                    } else {
                        "number material x.low x.high ix.low ix.high"
                    },
                    "",
                )?),
                Some("material") => {
                    let f = fields(
                        words,
                        "number affinity permittivity nc nv eg deg.dt eg.tref deg.dn deg.dp eg.nref eg.pref tn tp cn cp srh.nref srh.pref",
                        if planar {
                            "silicon|si|semiconductor oxide|sio2|insulator"
                        } else {
                            "silicon|si|semiconductor"
                        },
                    )?;
                    let id = id(&f, "number")?;
                    materials
                        .entry(id)
                        .or_insert_with(Fields::default)
                        .values
                        .extend(f.values);
                }
                Some("doping") => {
                    const PROFILE: &str = "uniform gaussian|gauss erfc exponential linear n.type|donor|phosphorus|arsenic|antimony p.type|acceptor|boron x.axis";
                    if planar {
                        profiles.push(fields(words, &format!("domains {BOX} conc|peak.conc location|range char.length ratio.lat"), &format!("{PROFILE} y.axis lat.rotate lat.unif|lat.uniform lat.lin|lat.linear lat.gauss|lat.gaussian lat.erfc lat.exp|lat.exponential"))?);
                    } else {
                        profiles.push(fields(words, "domains x.low x.high ix.low ix.high conc|peak.conc location|range char.length", PROFILE)?);
                    }
                }
                Some("mobility") => {
                    mobility_cards.push(fields(
                        words,
                        if planar { "material mumax mumin ntref ntexp vsat concmodel fieldmodel mus ec.a ec.b" } else { "material mumax mumin ntref ntexp vsat concmodel fieldmodel" },
                        "electron hole majority minority init",
                    )?);
                }
                Some("models") => {
                    let flags = fields(
                        words,
                        "",
                        if planar {
                            "bgn srh conctau auger avalanche tempmob concmob fieldmob surfmob transmob"
                        } else {
                            "bgn srh conctau auger avalanche tempmob concmob fieldmob"
                        },
                    )?;
                    switches.values.extend(flags.values);
                }
                Some("options") => {
                    let f = fields(
                        words,
                        if planar {
                            "defw defa defl tnom"
                        } else {
                            "defa base.area base.length base.depth tnom"
                        },
                        if planar {
                            "diode bipolar|bjt resistor capacitor|moscap mosfet"
                        } else {
                            "diode bipolar|bjt resistor"
                        },
                    )?;
                    options.values.extend(f.values);
                }
                Some("method") => {
                    let mut args = words[1..].iter().map(String::as_str).filter(|w| *w != "=");
                    while let Some(key) = args.next() {
                        match keyword(key, "frequency ac.analysis|acanalysis") {
                            Some("frequency") => {
                                frequency = args
                                    .next()
                                    .and_then(parse_number)
                                    .filter(|v| v.is_finite() && *v > 0.)
                                    .ok_or_else(|| invalid("invalid method frequency"))?;
                            }
                            Some("ac.analysis") if args.next() == Some("direct") => {}
                            _ => {
                                warn(warnings);
                                break;
                            }
                        }
                    }
                }
                Some("output") => {
                    let mut unsupported = false;
                    for word in &words[1..] {
                        if keyword(word.trim_start_matches('^'), "material").is_some() {
                            report_material = !word.starts_with('^');
                        } else {
                            unsupported = true;
                        }
                    }
                    if unsupported {
                        warn(warnings);
                    }
                }
                _ => {
                    return Err(Diagnostic::error(
                        "E-UNSUPPORTED",
                        format!("unsupported CIDER card {name}"),
                    ));
                }
            }
        }
        let ports = match card.kind.as_str() {
            "numos" => 4,
            "nbjt" => 3,
            _ => 2,
        };
        let (plane, domain, selected) = if planar {
            let plane = Plane::bind(
                &x,
                &y,
                &materials,
                &domains,
                &electrodes,
                &contacts,
                &interfaces,
                ports,
            )?;
            let silicon = plane.silicon_material;
            (Some(plane), Vec::new(), silicon)
        } else {
            let (domain, selected) = line_domains(&x, domains)?;
            (None, domain, selected)
        };
        let material = match selected {
            Some(m) => materials
                .remove(&m)
                .ok_or_else(|| invalid("undefined silicon material"))?,
            None if planar => Fields::default(),
            None => return Err(invalid("undefined silicon material")),
        };
        let mut mobility: [[Fields; 2]; 2] =
            std::array::from_fn(|k| std::array::from_fn(|class| Fields::bulk(k, class, None)));
        // [MUS, theta_A, theta_B] from the 300.15 K silicon material report.
        let mut surface_params = [[991., 2.67e-6, 4.18e-14], [240., 3.07e-6, 0.]];
        for f in mobility_cards {
            if Some(id(&f, "material")?) != selected {
                return Err(invalid("mobility material does not match domain"));
            }
            if f.flag("hole") && f.flag("electron") || f.flag("majority") && f.flag("minority") {
                return Err(invalid("conflicting carrier classes on mobility card"));
            }
            for (k, classes) in mobility.iter_mut().enumerate() {
                if k == 0 && f.flag("hole") || k == 1 && f.flag("electron") {
                    continue;
                }
                surface_params[k][0] = f.positive("mus", surface_params[k][0])?;
                for (i, key) in [(1, "ec.a"), (2, "ec.b")] {
                    if f.values.contains_key(key) {
                        let inverse = 1. / f.positive(key, 0.)?;
                        surface_params[k][i] = if i == 2 { inverse * inverse } else { inverse };
                    }
                }
                for (minority, params) in classes.iter_mut().enumerate() {
                    if minority == 0 && f.flag("minority") || minority == 1 && f.flag("majority") {
                        continue;
                    }
                    if f.flag("init") {
                        let model = f.get("concmodel", params.get("concmodel", 1.));
                        let field = params.get("fieldmodel", 0.);
                        *params = Fields::bulk(k, minority, Some(model));
                        params.values.insert("fieldmodel".into(), field);
                    }
                    params.values.extend(f.values.clone());
                }
            }
        }
        let mut net = vec![0.; plane.as_ref().map_or(x.len(), |p| p.vertices.len())];
        let mut impurities = net.clone();
        for f in profiles {
            let shape = ["uniform", "gaussian", "erfc", "exponential", "linear"]
                .into_iter()
                .filter(|s| f.flag(s))
                .collect::<Vec<_>>();
            let [shape] = shape.as_slice() else {
                return Err(invalid("specify one analytic doping profile"));
            };
            let conc = f.positive("conc", 0.)?;
            let len = f.positive("char.length", 1.)?;
            let peak = f.get("location", 0.);
            if f.flag("n.type") && f.flag("p.type") {
                return Err(invalid("conflicting dopant polarity"));
            }
            if f.domains.iter().any(|id| {
                !plane.as_ref().map_or_else(
                    || domain.contains(&Some(*id)),
                    |p| p.domain_ids.contains(id),
                )
            }) {
                return Err(invalid("unknown doping domain"));
            }
            let point = *shape != "uniform";
            let bx = bounds(&f, &x, point, false)?;
            let by = if planar {
                bounds(&f, &y, point, true)?
            } else {
                (0., 0.)
            };
            let along_x = !planar || f.flag("x.axis");
            if along_x && f.flag("y.axis") {
                return Err(invalid("conflicting primary doping axes"));
            }
            let mut lateral = ["rotate", "unif", "lin", "gauss", "erfc", "exp"]
                .into_iter()
                .filter(|k| f.flag(&format!("lat.{k}")));
            let lat = lateral
                .next()
                .unwrap_or(if planar { shape } else { "rotate" });
            if lateral.next().is_some() {
                return Err(invalid("conflicting lateral doping profiles"));
            }
            let ratio = f.positive("ratio.lat", 1.)?;
            let distance = |v: f64, (lo, hi): (f64, f64)| (lo - v).max(v - hi).max(0.);
            for i in 0..net.len() {
                let weight = if let Some(p) = &plane {
                    p.vertices[i].domain_fraction(&f.domains)
                } else {
                    f64::from(
                        f.domains.is_empty() || domain[i].is_some_and(|id| f.domains.contains(&id)),
                    )
                };
                if weight == 0. {
                    continue;
                }
                let dx = distance(x[i % x.len()], bx);
                let dy = if planar {
                    distance(y[i / x.len()], by)
                } else {
                    0.
                };
                let (d, l) = if along_x {
                    (dx, dy / ratio)
                } else {
                    (dy, dx / ratio)
                };
                let c = if lat == "rotate" {
                    profile(shape, d.hypot(l), peak, len)
                } else {
                    let lateral_factor = match lat {
                        _ if !planar => profile(lat, l, peak, len),
                        "uniform" | "unif" => f64::from(l <= peak),
                        "linear" | "lin" => 1.,
                        "erfc" => erfc(((l - peak) / len).abs()),
                        _ => profile(lat, l, peak, len),
                    };
                    profile(shape, d, peak, len) * lateral_factor
                } * conc
                    * weight;
                impurities[i] += c;
                net[i] += if f.flag("p.type") { -c } else { c };
            }
        }
        for v in &mut x {
            *v *= 1e-4
        }
        Ok(Self {
            x,
            plane,
            frequency,
            report_material,
            mobility,
            surface_params,
            net,
            impurities,
            material,
            switches,
            options,
        })
    }
}

fn id(f: &Fields, key: &str) -> Result<u32, Diagnostic> {
    identifier(f.get(key, 0.), key)
}

fn identifier(i: f64, key: &str) -> Result<u32, Diagnostic> {
    if !(1. ..=u32::MAX as f64).contains(&i) || i.fract() != 0. {
        return Err(invalid(format!("invalid {key} identifier")));
    }
    Ok(i as u32)
}

fn profile(shape: &str, distance: f64, peak: f64, length: f64) -> f64 {
    let z = (distance - peak) / length;
    match shape {
        "uniform" | "unif" => f64::from(distance == 0.),
        "gaussian" | "gauss" => (-z * z).exp(),
        "erfc" => erfc(z),
        "exponential" | "exp" => (-z.abs()).exp(),
        _ => (1. - z.abs()).max(0.),
    }
}

fn line_domains(
    x: &[f64],
    domains: Vec<Fields>,
) -> Result<(Vec<Option<u32>>, Option<u32>), Diagnostic> {
    if x.len() < 3 || x.windows(2).any(|w| w[1] <= w[0]) {
        return Err(invalid("need at least three distinct mesh points"));
    }
    let mut domain = vec![None; x.len()];
    let mut selected = None;
    let mut coverage = Vec::new();
    for f in domains {
        let index = id(&f, "number")?;
        let mat = id(&f, "material")?;
        if selected.replace(mat).is_some_and(|n| n != mat) {
            return Err(Diagnostic::error(
                "E-UNSUPPORTED",
                "CIDER heterogeneous materials",
            ));
        }
        let (lo, hi) = bounds(&f, x, false, false)?;
        coverage.push((lo, hi));
        for (&pos, d) in x.iter().zip(&mut domain) {
            if pos >= lo && pos <= hi {
                *d = Some(index)
            }
        }
    }
    coverage.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut reached = x[0];
    for (lo, hi) in coverage {
        if lo > reached {
            break;
        }
        reached = reached.max(hi);
    }
    if reached < x[x.len() - 1] || domain.contains(&None) {
        return Err(invalid("domain cards must cover the mesh"));
    }
    Ok((domain, selected))
}
