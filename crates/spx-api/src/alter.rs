//! Parameter changes applied to a netlist (instance values, instance parameters, model
//! parameters and options).
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use serde::{Deserialize, Serialize};
use spx_diag::{Confidence, Diagnostic};
use spx_netlist::{ElementKind, Netlist, OptionCard, Span};

/// One parameter change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// Instance name (`r1`, `m3`), model name, or `.options`.
    pub target: String,
    /// Parameter: `value` (or `r`/`c`/`l`/`dc`/`gain`) for simple elements, an instance
    /// parameter (`w`, `l`, `area`, ...) for devices, a model parameter, or an option name.
    pub param: String,
    /// New value (SI units).
    pub value: f64,
}

/// A change that was applied.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Applied {
    /// Target as given (lower-cased).
    pub target: String,
    /// Parameter (lower-cased).
    pub param: String,
    /// Previous value, if the parameter was set before.
    pub old: Option<f64>,
    /// New value.
    pub new: f64,
}

fn alter_error(msg: String) -> Diagnostic {
    Diagnostic::error("E-API-ALTER", msg)
}

/// Instance parameters accepted per device letter.
fn device_params(kind: &ElementKind) -> &'static [&'static str] {
    match kind {
        ElementKind::Diode { .. } | ElementKind::Bjt { .. } => &["area"],
        ElementKind::Mosfet { .. } => &["l", "w", "ad", "as", "pd", "ps"],
        _ => &[],
    }
}

fn set_param(list: &mut Vec<(String, f64)>, k: &str, v: f64) -> Option<f64> {
    if let Some(e) = list.iter_mut().find(|(n, _)| n == k) {
        let old = e.1;
        e.1 = v;
        Some(old)
    } else {
        list.push((k.to_string(), v));
        None
    }
}

/// Apply one change to `nl`.
pub(crate) fn apply(nl: &mut Netlist, c: &Change) -> Result<Applied, Diagnostic> {
    let target = c.target.trim().to_ascii_lowercase();
    let param = c.param.trim().to_ascii_lowercase();
    let value = c.value;
    if !value.is_finite() {
        return Err(alter_error(format!(
            "value for `{target}.{param}` is not finite"
        )));
    }
    let done = |old| Applied {
        target: target.clone(),
        param: param.clone(),
        old,
        new: value,
    };
    if target == ".options" || target == "options" {
        let old = nl
            .options
            .iter_mut()
            .find(|o| o.name == param)
            .map(|o| o.value.replace(value));
        if old.is_none() {
            nl.options.push(OptionCard {
                name: param.clone(),
                value: Some(value),
                span: Span::new(0, 0),
            });
        }
        return Ok(done(old.flatten()));
    }
    if let Some(e) = nl.elements.iter_mut().find(|e| e.name == target) {
        let allowed = device_params(&e.kind);
        let old = match &mut e.kind {
            ElementKind::Resistor { value: v, .. }
                if matches!(param.as_str(), "value" | "r" | "resistance") =>
            {
                Some(std::mem::replace(v, value))
            }
            ElementKind::Capacitor { value: v, .. }
                if matches!(param.as_str(), "value" | "c" | "capacitance") =>
            {
                Some(std::mem::replace(v, value))
            }
            ElementKind::Inductor { value: v, .. }
                if matches!(param.as_str(), "value" | "l" | "inductance") =>
            {
                Some(std::mem::replace(v, value))
            }
            ElementKind::VoltageSource { source, .. }
            | ElementKind::CurrentSource { source, .. }
                if matches!(param.as_str(), "value" | "dc") =>
            {
                Some(source.dc.replace(value).unwrap_or(0.0))
            }
            ElementKind::VoltageSource { source, .. }
            | ElementKind::CurrentSource { source, .. }
                if matches!(param.as_str(), "ac" | "acmag") =>
            {
                Some(std::mem::replace(&mut source.ac_mag, value))
            }
            ElementKind::Vcvs { gain: g, .. }
            | ElementKind::Cccs { gain: g, .. }
            | ElementKind::Vccs { gm: g, .. }
            | ElementKind::Ccvs { r: g, .. }
                if matches!(param.as_str(), "value" | "gain" | "gm" | "r") =>
            {
                Some(std::mem::replace(g, value))
            }
            ElementKind::Diode { params, .. }
            | ElementKind::Bjt { params, .. }
            | ElementKind::Mosfet { params, .. }
                if allowed.contains(&param.as_str()) =>
            {
                return Ok(done(set_param(params, &param, value)));
            }
            _ => {
                let mut d = alter_error(format!(
                    "instance `{target}` has no alterable parameter `{param}`"
                ));
                if !allowed.is_empty() {
                    d = d.with_suggestion(
                        format!(
                            "use one of: {}; or alter its model `{}`",
                            allowed.join(", "),
                            e.kind.model().unwrap_or("?")
                        ),
                        Confidence::High,
                    );
                } else {
                    d = d.with_suggestion("use `value`", Confidence::High);
                }
                return Err(d);
            }
        };
        return Ok(done(old));
    }
    if let Some(m) = nl.models.iter_mut().find(|m| m.name == target) {
        return Ok(done(set_param(&mut m.params, &param, value)));
    }
    Err(alter_error(format!(
        "`{target}` is neither an instance nor a model of this circuit"
    ))
    .with_suggestion(
        "use an instance name, a model name, or `.options`",
        Confidence::High,
    ))
}
