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
pub(super) type Shim = extern "C" fn(f64, f64) -> f64;

macro_rules! math_table {
    ($($op:ident),+) => {
        #[derive(Clone, Copy)]
        #[repr(usize)]
        enum Row { $($op),+ }
        pub(super) static TABLE: &[[Shim; 3]] = &[$(shims!($op)),+];
    }
}
math_table!(
    Exp, Ln, Pow, Min, Max, Limexp, Log, Sin, Cos, Tan, Asin, Acos, Atan, Sinh, Cosh, Tanh, Asinh,
    Acosh, Atanh, Hypot, Atan2
);

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
        let row = match m {
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
                let sa = self.select(neg, minus, z);
                let sa = self.select(pos, one, sa);
                return (self.fb.ins().fabs(x), sa, z);
            }
            Exp => Row::Exp,
            Ln => Row::Ln,
            Sqrt => {
                let v = self.fb.ins().sqrt(x);
                let half = self.num(0.5);
                let a = self.fb.ins().fdiv(half, v);
                return (v, a, z);
            }
            Pow => Row::Pow,
            Floor => return (self.fb.ins().floor(x), z, z),
            Ceil => return (self.fb.ins().ceil(x), z, z),
            Trunc if self.predicate(x).is_some() => return (x, z, z),
            Trunc => return (self.fb.ins().trunc(x), z, z),
            Min => Row::Min,
            Max => Row::Max,
            Limexp => Row::Limexp,
            Log => Row::Log,
            Sin => Row::Sin,
            Cos => Row::Cos,
            Tan => Row::Tan,
            Asin => Row::Asin,
            Acos => Row::Acos,
            Atan => Row::Atan,
            Sinh => Row::Sinh,
            Cosh => Row::Cosh,
            Tanh => Row::Tanh,
            Asinh => Row::Asinh,
            Acosh => Row::Acosh,
            Atanh => Row::Atanh,
            Hypot => Row::Hypot,
            Atan2 => Row::Atan2,
        };
        let v = self.math_call(row as usize, 0, x, y);
        if !left && !right {
            return (v, z, z);
        }
        let a = if !left {
            z
        } else {
            match m {
                Exp => v,
                Ln => self.fb.ins().fdiv(one, x),
                Tanh => {
                    let square = self.fb.ins().fmul(v, v);
                    self.fb.ins().fsub(one, square)
                }
                _ => self.math_call(row as usize, 1, x, y),
            }
        };
        let b = if right && m == Pow {
            let ln = self.math_call(Row::Ln as usize, 0, x, z);
            self.fb.ins().fmul(v, ln)
        } else if right && matches!(m, Hypot | Atan2 | Min | Max) {
            self.math_call(row as usize, 2, x, y)
        } else {
            z
        };
        (v, a, b)
    }
}
