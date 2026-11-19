//! Coupled winding flux and hysteretic controlled conductances.
//! Provenance: original MNA formulation from Ho, Ruehli & Brennan (1975) and
//! the ngspice user manual descriptions of coupled inductors and switches.

use crate::{DeviceGroup, EvalCtx, Node, Slots, StampBuilder, StateRef, volt};
use sansho_diag::Diagnostic;

/// A symmetric off-diagonal pair in the branch flux matrix.
#[derive(Debug)]
pub struct Coupling {
    pub name: String,
    pub windings: [String; 2],
    pub mutual: f64,
    pub branches: [usize; 2],
}

impl DeviceGroup for Coupling {
    fn type_name(&self) -> &'static str {
        "coupling"
    }
    fn len(&self) -> usize {
        1
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for (idx, name) in self.branches.iter_mut().zip(&self.windings) {
            *idx = b.branch(name).ok_or_else(|| {
                Diagnostic::error("E-COUPLING-L", format!("unknown winding {name}"))
            })?;
        }
        let [a, z] = self.branches.map(Some);
        b.c(a, z);
        b.c(z, a);
        b.q(a);
        b.q(z);
        Ok(())
    }
    fn eval(&self, _: &EvalCtx, x: &[f64], _: StateRef<'_>, out: &mut Slots) {
        let [a, b] = self.branches;
        out.c.fill(-self.mutual);
        out.q
            .copy_from_slice(&[-self.mutual * x[b], -self.mutual * x[a]]);
    }
}

/// The control input is read only after the continuous equations settle.
#[derive(Debug)]
pub enum Control {
    Voltage(Node, Node),
    Current(String, Node),
}

#[derive(Debug)]
pub struct Switch {
    pub span: sansho_diag::Span,
    pub name: String,
    pub p: Node,
    pub n: Node,
    pub control: Control,
    pub threshold: f64,
    pub width: f64,
    pub gon: f64,
    pub goff: f64,
    pub initially_on: bool,
}

impl Switch {
    fn input(&self, x: &[f64]) -> f64 {
        match self.control {
            Control::Voltage(p, n) => volt(x, p) - volt(x, n),
            Control::Current(_, i) => volt(x, i),
        }
    }
    fn on(&self, state: f64) -> bool {
        state > 0. || (state == 0. && self.initially_on)
    }
    fn boundary(&self, on: bool) -> f64 {
        if on {
            self.threshold - self.width
        } else {
            self.threshold + self.width
        }
    }
}

impl DeviceGroup for Switch {
    fn instance_span(&self) -> Option<sansho_diag::Span> {
        Some(self.span)
    }
    fn type_name(&self) -> &'static str {
        "switch"
    }
    fn len(&self) -> usize {
        1
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        1
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        if let Control::Current(name, k) = &mut self.control {
            *k = Some(b.branch(name).ok_or_else(|| {
                Diagnostic::error(
                    "E-SWITCH-MODEL",
                    format!("{} references unknown control branch {name}", self.name),
                )
            })?);
        }
        b.resistor(self.p, self.n);
        Ok(())
    }
    fn eval(&self, _: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        let g = if self.on(st.current[0]) {
            self.gon
        } else {
            self.goff
        };
        out.resistor(&mut Default::default(), g, volt(x, self.p), volt(x, self.n));
    }
    fn settle(&self, x: &[f64], state: &mut [f64]) -> bool {
        let on = self.on(state[0]);
        let u = self.input(x);
        let next = if on {
            u >= self.boundary(true)
        } else {
            u > self.boundary(false)
        };
        state[0] = if next { 1. } else { -1. };
        on != next
    }
    fn crossing(&self, old: &[f64], new: &[f64], accepted: &[f64], current: &[f64]) -> Option<f64> {
        let on = self.on(accepted[0]);
        if on == self.on(current[0]) {
            return None;
        }
        let a = self.input(old);
        let b = self.input(new);
        let f = (self.boundary(on) - a) / (b - a);
        Some(if f.is_finite() { f.clamp(0., 1.) } else { 1. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winding_flux_and_cross_partial_use_the_other_branch() {
        let mut u = crate::Unknowns::default();
        let b = u.alloc_branch("lb");
        let a = u.alloc_branch("la");
        let mut k = Coupling {
            name: "k1".into(),
            windings: ["la".into(), "lb".into()],
            mutual: -0.125,
            branches: [0; 2],
        };
        let mut stamps = StampBuilder::new(&u);
        k.setup(&mut stamps).unwrap();
        let layout = stamps.finish();
        assert_eq!(layout.c, [(Some(a), Some(b)), (Some(b), Some(a))]);
        assert!(layout.g.is_empty() && layout.f.is_empty());
        let mut out = Slots::for_layout(&layout);
        k.eval(
            &EvalCtx::dc(),
            &[-2., 3.],
            StateRef {
                accepted: &[],
                current: &mut [],
            },
            &mut out,
        );
        assert_eq!(out.q, [-0.25, 0.375]);
        assert_eq!(out.c, [0.125, 0.125]);
    }

    #[test]
    fn switch_keeps_accepted_state_until_a_trial_is_committed() {
        let s = Switch {
            span: sansho_diag::Span::new(1, 1),
            name: "s".into(),
            p: Some(1),
            n: None,
            control: Control::Voltage(Some(0), None),
            threshold: 1.,
            width: 0.25,
            gon: 1.,
            goff: 1e-6,
            initially_on: false,
        };
        let accepted = [-1.];
        let mut trial = accepted;
        assert!(!s.settle(&[1.25], &mut trial));
        assert!(s.settle(&[1.5], &mut trial));
        assert_eq!(s.crossing(&[1.0], &[1.5], &accepted, &trial), Some(0.5));
        assert!(!s.settle(&[0.75], &mut trial));
        assert!(s.settle(&[0.7], &mut trial));
        assert_eq!(accepted, [-1.]);
    }
}
