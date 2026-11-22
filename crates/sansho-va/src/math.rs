//! Scalar math and local chain-rule factors shared by both interpreters.
//! Provenance: original work from elementary calculus and the Accellera Verilog-AMS LRM.

#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum Math {
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Exp,
    Ln,
    Sqrt,
    Abs,
    Pow,
    Floor,
    Ceil,
    Min,
    Max,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    Not,
    Trunc,
    Bool,
    Limexp,
    Log,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Asinh,
    Acosh,
    Atanh,
    Hypot,
    Atan2,
}

impl Math {
    pub(crate) fn differentiable(self) -> bool {
        !matches!(
            self,
            Self::Lt
                | Self::Le
                | Self::Gt
                | Self::Ge
                | Self::Eq
                | Self::Ne
                | Self::Bool
                | Self::Not
        )
    }
    pub(crate) fn named(name: &str) -> Option<Self> {
        use Math::*;
        Some(match name {
            "+" => Add,
            "-" => Sub,
            "*" => Mul,
            "/" => Div,
            "exp" => Exp,
            "ln" => Ln,
            "sqrt" => Sqrt,
            "abs" => Abs,
            "pow" | "**" => Pow,
            "floor" => Floor,
            "ceil" => Ceil,
            "min" => Min,
            "max" => Max,
            "<" => Lt,
            "<=" => Le,
            ">" => Gt,
            ">=" => Ge,
            "==" | "===" => Eq,
            "!=" | "!==" => Ne,
            "!" => Not,
            "limexp" => Limexp,
            "log" => Log,
            "sin" => Sin,
            "cos" => Cos,
            "tan" => Tan,
            "asin" => Asin,
            "acos" => Acos,
            "atan" => Atan,
            "sinh" => Sinh,
            "cosh" => Cosh,
            "tanh" => Tanh,
            "asinh" => Asinh,
            "acosh" => Acosh,
            "atanh" => Atanh,
            "hypot" => Hypot,
            "atan2" => Atan2,
            _ => return None,
        })
    }
    /// Primal and the two local chain-rule factors, in source evaluation order.
    #[inline]
    pub(crate) fn apply(self, x: f64, y: f64) -> (f64, f64, f64) {
        use Math::*;
        match self {
            Add => (x + y, 1., 1.),
            Sub => (x - y, 1., -1.),
            Mul => (x * y, y, x),
            Div => (x / y, 1. / y, -x / (y * y)),
            Neg => (-x, -1., 0.),
            Exp => {
                let v = x.exp();
                (v, v, 0.)
            }
            Ln => (x.ln(), 1. / x, 0.),
            Sqrt => {
                let v = x.sqrt();
                (v, 0.5 / v, 0.)
            }
            Abs => (
                x.abs(),
                if x > 0. {
                    1.
                } else if x < 0. {
                    -1.
                } else {
                    0.
                },
                0.,
            ),
            Pow => {
                let v = x.powf(y);
                (v, if y == 0. { 0. } else { y * x.powf(y - 1.) }, v * x.ln())
            }
            Floor => (x.floor(), 0., 0.),
            Ceil => (x.ceil(), 0., 0.),
            Min | Max => {
                if (self == Min && x <= y) || (self == Max && x >= y) {
                    (x, 1., 0.)
                } else {
                    (y, 0., 1.)
                }
            }
            Lt => (f64::from(x < y), 0., 0.),
            Le => (f64::from(x <= y), 0., 0.),
            Gt => (f64::from(x > y), 0., 0.),
            Ge => (f64::from(x >= y), 0., 0.),
            Eq => (f64::from(x == y), 0., 0.),
            Ne => (f64::from(x != y), 0., 0.),
            Not => (f64::from(x == 0.), 0., 0.),
            Bool => (f64::from(x != 0.), 0., 0.),
            Trunc => (x.trunc(), 0., 0.),
            Limexp => {
                if x > 80. {
                    let t = 80f64.exp();
                    (t * (1. + (x - 80.)), t, 0.)
                } else {
                    let v = x.exp();
                    (v, v, 0.)
                }
            }
            Log => (x.log10(), 1. / (x * std::f64::consts::LN_10), 0.),
            Sin => (x.sin(), x.cos(), 0.),
            Cos => (x.cos(), -x.sin(), 0.),
            Tan => (x.tan(), 1. / x.cos().powi(2), 0.),
            Asin => (x.asin(), 1. / (1. - x * x).sqrt(), 0.),
            Acos => (x.acos(), -1. / (1. - x * x).sqrt(), 0.),
            Atan => (x.atan(), 1. / (1. + x * x), 0.),
            Sinh => (x.sinh(), x.cosh(), 0.),
            Cosh => (x.cosh(), x.sinh(), 0.),
            Tanh => (x.tanh(), 1. - x.tanh().powi(2), 0.),
            Asinh => (x.asinh(), 1. / (1. + x * x).sqrt(), 0.),
            Acosh => (x.acosh(), 1. / (x * x - 1.).sqrt(), 0.),
            Atanh => (x.atanh(), 1. / (1. - x * x), 0.),
            Hypot => {
                let v = x.hypot(y);
                if v == 0. {
                    (v, 0., 0.)
                } else {
                    (v, x / v, y / v)
                }
            }
            Atan2 => {
                let d = x * x + y * y;
                (x.atan2(y), y / d, -x / d)
            }
        }
    }
}
