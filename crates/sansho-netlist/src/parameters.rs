//! Flattened parameter bindings for a fixed-topology statistical batch.
//! Provenance: original work from SPICE parameter scoping conventions.

use crate::{Diagnostic, Element, ElementKind, Expr, Line, ModelCard, Netlist, Sample, Span};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct BoundLine {
    pub index: usize,
    pub line: Line,
    pub fields: Vec<(usize, Expr)>,
}

impl BoundLine {
    fn sample(&self, draws: &crate::statistics::Draws) -> Result<Line, Vec<Diagnostic>> {
        let mut line = self.line.clone();
        for (i, expr) in &self.fields {
            line.tokens[*i].text = value(expr, draws, line.tokens[*i].span)?.to_string();
        }
        Ok(line)
    }
}

type Fields = Vec<(usize, Expr, Span)>;
const BOUNDS: [&str; 5] = ["level", "lmin", "lmax", "wmin", "wmax"];

pub(crate) fn in_bin(
    [level, lmin, lmax, wmin, wmax]: [f64; 5],
    length: f64,
    width: f64,
    fingers: f64,
) -> bool {
    let w = if matches!(level, 10. | 58. | 54.) {
        width / fingers
    } else {
        width
    };
    let tol = 1. - 1e-9;
    length >= lmin * tol && length < lmax * tol && w >= wmin * tol && w < wmax * tol
}

#[derive(Debug)]
struct Candidate {
    bounds: [Expr; 5],
    values: Result<(ModelCard, Fields), Diagnostic>,
}

#[derive(Debug)]
pub(crate) struct ModelBinding {
    model: usize,
    element: usize,
    fields: Fields,
    candidates: Vec<Candidate>,
    scale: f64,
}

struct CardBinder<'a> {
    instance: &'a str,
    params: &'a [(String, f64)],
    overrides: HashMap<String, Expr>,
}

impl CardBinder<'_> {
    fn expressions(
        &self,
        card: &ModelCard,
        source: Option<&(usize, &crate::preproc::ModelScope, &Line)>,
        only_bounds: bool,
    ) -> Result<(ModelCard, Fields), Diagnostic> {
        let mut result = card.clone();
        let mut fields = vec![];
        let Some((_, scope, line)) = source else {
            return Ok((result, fields));
        };
        let bind = scope.binder(self.params, self.overrides.clone(), self.instance);
        result.params.clear();
        for kv in line.tokens[3..].as_chunks::<3>().0 {
            let name = kv[0].lower();
            let token = &kv[2];
            if only_bounds && !BOUNDS.contains(&name.as_str())
                || name == "version" && crate::parser::dotted_version(&token.text)
            {
                continue;
            }
            let v = if crate::expr::is_number_literal(&token.text) {
                crate::parse_number(&token.text).ok_or_else(|| "invalid model number".to_string())
            } else {
                bind(token).and_then(|expr| {
                    let v = crate::expr::sample(&expr, None)?;
                    if !matches!(expr, Expr::Num(_)) {
                        fields.push((result.params.len(), expr, token.span))
                    }
                    Ok(v)
                })
            }
            .and_then(|v| {
                if v.is_finite() {
                    Ok(v)
                } else {
                    Err("nonfinite model value".into())
                }
            })
            .map_err(|msg| {
                Diagnostic::error(
                    "E-MC-PARAM",
                    format!("{}: {}.{name}: {msg}", self.instance, card.name),
                )
                .with_span(token.span)
            })?;
            result.params.push((name, v));
        }
        Ok((result, fields))
    }
}

/// Retain expressions only for live cards and bins reachable with sampled geometry.
pub(crate) fn bind_models(
    elements: &mut [Element],
    cards: &mut Vec<ModelCard>,
    source: &crate::parser::DynamicCards<'_>,
    lines: &[BoundLine],
    families: &HashMap<usize, Vec<usize>>,
    scale: f64,
) -> Result<Vec<ModelBinding>, Vec<Diagnostic>> {
    let names: HashMap<_, _> = cards
        .iter()
        .enumerate()
        .map(|(i, m)| (m.name.as_str(), i))
        .collect();
    let lines: HashMap<_, _> = lines.iter().map(|b| (b.index, b)).collect();
    let mut live = Vec::new();
    let mut bindings = Vec::new();
    let mut used = HashSet::new();
    let mut literal_aliases = HashMap::<usize, String>::new();
    for (element, e) in elements.iter_mut().enumerate() {
        let mut empty = vec![];
        let (model, params) = match &mut e.kind {
            ElementKind::Resistor {
                model: Some(model),
                params,
                ..
            }
            | ElementKind::Inductor {
                model: Some(model),
                params,
                ..
            }
            | ElementKind::Capacitor {
                model: Some(model),
                params,
                ..
            } => (model, params),
            ElementKind::Ltra { model, .. } | ElementKind::Switch { model, .. } => {
                (model, &mut empty)
            }
            ElementKind::Mosfet { model, params, .. }
            | ElementKind::Va { model, params, .. }
            | ElementKind::Bjt { model, params, .. }
            | ElementKind::Jfet { model, params, .. }
            | ElementKind::Mesfet { model, params, .. }
            | ElementKind::Vdmos { model, params, .. }
            | ElementKind::Kspice { model, params, .. }
            | ElementKind::Urc { model, params, .. }
            | ElementKind::Diode { model, params, .. } => (model, params),
            _ => continue,
        };
        let idx = *names.get(model.as_str()).ok_or_else(|| {
            vec![
                Diagnostic::error("E-PARSE-MODEL", format!("unknown model {model}"))
                    .with_span(e.span),
            ]
        })?;
        let overrides = lines
            .get(&element)
            .into_iter()
            .flat_map(|b| {
                b.fields.iter().filter_map(|(i, expr)| {
                    let k = i.checked_sub(2)?;
                    b.line.tokens[k + 1]
                        .is_eq()
                        .then(|| (b.line.tokens[k].lower(), expr.clone()))
                })
            })
            .collect::<HashMap<_, _>>();
        let varying_size = ["w", "l", "nf"].iter().any(|k| overrides.contains_key(*k));
        let binder = CardBinder {
            instance: &e.name,
            params,
            overrides,
        };
        let (mut nominal, fields) = binder
            .expressions(&cards[idx], source.get(&idx), false)
            .map_err(|e| vec![e])?;
        let mut bins = Vec::new();
        let mut varying = varying_size;
        if let Some(family) = families.get(&element) {
            for &i in family {
                let (card, exprs) = binder
                    .expressions(&cards[i], source.get(&i), true)
                    .map_err(|e| vec![e])?;
                let mut bounds = [1., 0., f64::INFINITY, 0., f64::INFINITY].map(Expr::Num);
                for (k, (name, v)) in card.params.iter().enumerate() {
                    if let Some(b) = BOUNDS.iter().position(|n| n == name) {
                        bounds[b] = exprs
                            .iter()
                            .find(|(f, _, _)| *f == k)
                            .map_or(Expr::Num(*v), |(_, e, _)| e.clone());
                    }
                }
                varying |= !exprs.is_empty();
                bins.push((i, bounds));
                if !varying && i == idx {
                    break;
                }
            }
        }
        let candidates = if varying {
            bins.into_iter()
                .map(|(i, bounds)| Candidate {
                    bounds,
                    values: binder.expressions(&cards[i], source.get(&i), false),
                })
                .collect()
        } else {
            vec![]
        };
        // Constant literal cards can be shared. Distinct bindings have stable private names.
        let literal = !source.contains_key(&idx) && candidates.is_empty();
        if literal && let Some(alias) = literal_aliases.get(&idx) {
            model.clone_from(alias);
            continue;
        }
        if !used.insert(model.clone()) {
            let mut suffix = element;
            loop {
                let alias = format!("{model}__sansho_{suffix}");
                if !names.contains_key(alias.as_str()) && used.insert(alias.clone()) {
                    *model = alias;
                    break;
                }
                suffix += 1;
            }
        }
        if literal {
            literal_aliases.insert(idx, model.clone());
        }
        nominal.name = model.clone();
        if !fields.is_empty() || !candidates.is_empty() {
            bindings.push(ModelBinding {
                model: live.len(),
                element,
                fields,
                candidates,
                scale,
            });
        }
        live.push(nominal);
    }
    *cards = live;
    Ok(bindings)
}

/// A flat deck and the numeric fields which depend on statistical functions.
#[derive(Debug)]
pub struct ParameterPlan {
    pub(crate) nominal: Netlist,
    pub(crate) elements: Vec<BoundLine>,
    pub(crate) directives: Vec<BoundLine>,
    pub(crate) models: Vec<ModelBinding>,
}

impl ParameterPlan {
    /// Primitive names whose numeric parameters or selected model values vary.
    pub fn varying_instances(&self) -> std::collections::HashSet<&str> {
        let models: std::collections::HashSet<_> = self
            .models
            .iter()
            .map(|f| self.nominal.models[f.model].name.as_str())
            .collect();
        self.elements
            .iter()
            .map(|p| self.nominal.elements[p.index].name.as_str())
            .chain(
                self.nominal
                    .elements
                    .iter()
                    .filter(|e| e.kind.model().is_some_and(|m| models.contains(m)))
                    .map(|e| e.name.as_str()),
            )
            .collect()
    }

    /// Fixed connectivity and nominal bindings.
    pub fn nominal(&self) -> &Netlist {
        &self.nominal
    }

    /// Bind one statistical instance without reading files or expanding subcircuits.
    pub fn sample(&self, run: Sample) -> Result<Netlist, Vec<Diagnostic>> {
        let draws = crate::statistics::Draws::new(run);
        let mut nl = self.nominal.clone();
        for p in &self.elements {
            let line = p.sample(&draws)?;
            let element = &mut nl.elements[p.index];
            let model = element.kind.model().map(str::to_owned);
            let mut sampled = crate::parser::sample_element(&line, &nl)?;
            match &mut sampled.kind {
                ElementKind::Resistor { model: m, .. }
                | ElementKind::Capacitor { model: m, .. }
                | ElementKind::Inductor { model: m, .. } => *m = model,
                ElementKind::Switch { model: m, .. }
                | ElementKind::Mosfet { model: m, .. }
                | ElementKind::Bjt { model: m, .. }
                | ElementKind::Va { model: m, .. }
                | ElementKind::Jfet { model: m, .. }
                | ElementKind::Mesfet { model: m, .. }
                | ElementKind::Vdmos { model: m, .. }
                | ElementKind::Kspice { model: m, .. }
                | ElementKind::Urc { model: m, .. }
                | ElementKind::Ltra { model: m, .. }
                | ElementKind::Diode { model: m, .. } => {
                    if let Some(model) = model {
                        *m = model
                    }
                }
                _ => {}
            }
            nl.elements[p.index] = sampled;
        }
        for directive in &self.directives {
            crate::parser::sample_directive(&directive.sample(&draws)?, directive.index, &mut nl)?;
        }
        for binding in &self.models {
            let model = &mut nl.models[binding.model];
            let mut fields = &binding.fields;
            if !binding.candidates.is_empty()
                && let ElementKind::Mosfet { params, .. } = &nl.elements[binding.element].kind
            {
                let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|p| p.1);
                let mut selected = None;
                for candidate in &binding.candidates {
                    let mut bounds = [0.; 5];
                    for (v, expr) in bounds.iter_mut().zip(&candidate.bounds) {
                        *v = match expr {
                            Expr::Num(x) => *x,
                            _ => value(expr, &draws, model.span)?,
                        };
                    }
                    if in_bin(
                        bounds,
                        get("l").unwrap_or(0.) * binding.scale,
                        get("w").unwrap_or(0.) * binding.scale,
                        get("nf").unwrap_or(1.),
                    ) {
                        selected = Some(&candidate.values);
                        break;
                    }
                }
                let (card, chosen) = selected
                    .ok_or_else(|| {
                        vec![
                            Diagnostic::error(
                                "E-PARSE-MODEL",
                                "no geometry bin covers this statistical instance",
                            )
                            .with_span(nl.elements[binding.element].span),
                        ]
                    })?
                    .as_ref()
                    .map_err(|d| vec![d.clone()])?;
                model.params.clone_from(&card.params);
                model.version.clone_from(&card.version);
                model.kind.clone_from(&card.kind);
                fields = chosen;
            }
            for (index, expr, span) in fields {
                model.params[*index].1 = value(expr, &draws, *span)?
            }
        }
        Ok(nl)
    }
}

fn value(e: &Expr, run: &crate::statistics::Draws, span: Span) -> Result<f64, Vec<Diagnostic>> {
    crate::expr::sample(e, Some(run))
        .and_then(|v| {
            if v.is_finite() {
                Ok(v)
            } else {
                Err("non-finite statistical parameter".into())
            }
        })
        .map_err(|msg| vec![Diagnostic::error("E-MC-PARAM", msg).with_span(span)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoFiles, prepare_parameters};

    #[test]
    fn sampled_geometry_reselects_bins_and_measurement_levels() {
        let text = "bins\n.model n.1 nmos level=1 wmax=2 vto={w}\n.model n.2 nmos level=1 wmin=2 vto={w+1}\n.param width=limit(2,0.5)\nM1 a a 0 0 n w={width} l=1\nR1 a 0 1k\n.dc v1 0 {width} 0.1\n.meas dc at_w find v(a) at={width}\n.end";
        let plan = prepare_parameters(text, &NoFiles).unwrap();
        let mut both = [false; 2];
        for index in 0..20 {
            let nl = plan.sample(Sample { seed: 0, index }).unwrap();
            let crate::AnalysisCard::Dc {
                inner: crate::DcSweep { stop, .. },
                ..
            } = &nl.analyses[0]
            else {
                unreachable!()
            };
            let vto = nl.models[0]
                .params
                .iter()
                .find(|(k, _)| k == "vto")
                .unwrap()
                .1;
            let upper = *stop > 2.;
            both[usize::from(upper)] = true;
            assert_eq!(vto, stop + f64::from(upper));
        }
        assert_eq!(both, [true, true]);
    }

    #[test]
    fn process_model_and_instance_draw_scopes() {
        let text = "scope\n.param p=agauss(2,1,3)\n.func u(a) {aunif(a,1)}\n.subckt cell a params: x=1\n.param y={u(x)}\nR1 a 0 {p}\nR2 a 0 {y}\nR3 a 0 {y}\n.ends\nXleft a cell x={u(2)}\nXright b cell x={u(2)}\n.model m nmos level=1 vto={u(p)} kp={p}\nM1 a a 0 0 m\nM2 b b 0 0 m\nRdiff b 0 {10+u(1)-u(1)}\n.end";
        let plan = prepare_parameters(text, &NoFiles).unwrap();
        let deck = plan
            .sample(Sample {
                seed: 111,
                index: 4,
            })
            .unwrap();
        let values: Vec<_> = deck
            .elements
            .iter()
            .filter_map(|e| match e.kind {
                ElementKind::Resistor { value, .. } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(values[0], values[3]);
        assert_eq!(values[1], values[2]);
        assert_eq!(values[4], values[5]);
        assert_ne!(values[1], values[4]);
        assert_ne!(values[6], 10.);
        assert_eq!(deck.models.len(), 2);
        assert_ne!(deck.models[0].params[1].1, deck.models[1].params[1].1);
        for card in &deck.models {
            assert_eq!(card.params[2].1, values[0]);
        }
    }
}
