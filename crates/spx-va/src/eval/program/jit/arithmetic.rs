//! Strict floating-point primals and chain-rule factors for native register instructions.
//! Provenance: original lowering of this crate's Math::apply and scaled-partial semantics.

use super::{Math, lower::Lower};
use cranelift_codegen::ir::{InstBuilder, Value, condcodes::FloatCC};

// Each shim specializes Math::apply at Rust compile time. Unused tuple elements
// disappear, while libm selection and exceptional values match the interpreter.
macro_rules! shims {
    ($op:ident) => {{
        extern "C" fn primal(x: f64, y: f64) -> f64 {
            Math::$op.apply(x, y).0
        }
        extern "C" fn left(x: f64, y: f64) -> f64 {
            Math::$op.apply(x, y).1
        }
        extern "C" fn right(x: f64, y: f64) -> f64 {
            Math::$op.apply(x, y).2
        }
        [primal, left, right]
    }};
}
type Shim = extern "C" fn(f64, f64) -> f64;

impl Lower<'_, '_> {
    pub(super) fn math(
        &mut self,
        m: Math,
        x: Value,
        y: Value,
        left: bool,
        right: bool,
    ) -> (Value, Value, Value) {
        use Math::*;
        let z = self.num(0.);
        let one = self.num(1.);
        let minus = self.num(-1.);
        let fns: [Shim; 3] = match m {
            Add => return (self.fb.ins().fadd(x, y), one, one),
            Sub => return (self.fb.ins().fsub(x, y), one, minus),
            Mul => return (self.fb.ins().fmul(x, y), y, x),
            Div => {
                let v = self.fb.ins().fdiv(x, y);
                let a = self.fb.ins().fdiv(one, y);
                let square = self.fb.ins().fmul(y, y);
                let neg = self.fb.ins().fneg(x);
                let b = self.fb.ins().fdiv(neg, square);
                return (v, a, b);
            }
            Neg => return (self.fb.ins().fneg(x), minus, z),
            Lt | Le | Gt | Ge | Eq | Ne | Bool | Not => {
                let cc = match m {
                    Lt => FloatCC::LessThan,
                    Le => FloatCC::LessThanOrEqual,
                    Gt => FloatCC::GreaterThan,
                    Ge => FloatCC::GreaterThanOrEqual,
                    Eq | Not => FloatCC::Equal,
                    _ => FloatCC::NotEqual,
                };
                let test = self
                    .fb
                    .ins()
                    .fcmp(cc, x, if matches!(m, Bool | Not) { z } else { y });
                return (self.number_bool(test), z, z);
            }
            Abs => {
                let pos = self.fb.ins().fcmp(FloatCC::GreaterThan, x, z);
                let neg = self.fb.ins().fcmp(FloatCC::LessThan, x, z);
                let sa = self.fb.ins().select(neg, minus, z);
                let sa = self.fb.ins().select(pos, one, sa);
                return (self.fb.ins().fabs(x), sa, z);
            }
            Exp => shims!(Exp),
            Ln => shims!(Ln),
            Sqrt => shims!(Sqrt),
            Pow => shims!(Pow),
            Floor => return (self.fb.ins().floor(x), z, z),
            Ceil => return (self.fb.ins().ceil(x), z, z),
            Trunc => return (self.fb.ins().trunc(x), z, z),
            Min => shims!(Min),
            Max => shims!(Max),
            Limexp => shims!(Limexp),
            Log => shims!(Log),
            Sin => shims!(Sin),
            Cos => shims!(Cos),
            Tan => shims!(Tan),
            Asin => shims!(Asin),
            Acos => shims!(Acos),
            Atan => shims!(Atan),
            Sinh => shims!(Sinh),
            Cosh => shims!(Cosh),
            Tanh => shims!(Tanh),
            Asinh => shims!(Asinh),
            Acosh => shims!(Acosh),
            Atanh => shims!(Atanh),
            Hypot => shims!(Hypot),
            Atan2 => shims!(Atan2),
        };
        let v = self.call(fns[0] as usize, &[x, y]);
        if !left && !right {
            return (v, z, z);
        }
        let a = if !left {
            z
        } else {
            match m {
                Exp => v,
                Ln => self.fb.ins().fdiv(one, x),
                Sqrt => {
                    let half = self.num(0.5);
                    self.fb.ins().fdiv(half, v)
                }
                Floor | Ceil | Trunc => z,
                _ => self.call(fns[1] as usize, &[x, y]),
            }
        };
        let b = if right && matches!(m, Pow | Hypot | Atan2 | Min | Max) {
            self.call(fns[2] as usize, &[x, y])
        } else {
            z
        };
        (v, a, b)
    }
}
