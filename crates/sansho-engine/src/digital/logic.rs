//! Digital node values and multiple-driver resolution.
//! Provenance: XSPICE twelve-state node notation and elementary multi-valued logic.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Level {
    Low,
    High,
    #[default]
    Unknown,
}
use Level::{High, Low, Unknown};
impl Level {
    pub(super) fn invert(self) -> Self {
        match self {
            Low => High,
            High => Low,
            Unknown => Unknown,
        }
    }
    pub(super) fn and(self, b: Self) -> Self {
        match (self, b) {
            (Low, _) | (_, Low) => Low,
            (High, High) => High,
            _ => Unknown,
        }
    }
    pub(super) fn or(self, b: Self) -> Self {
        self.invert().and(b.invert()).invert()
    }
    pub(super) fn xor(self, b: Self) -> Self {
        match self {
            Low => b,
            High => b.invert(),
            Unknown => Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Drive {
    Strong,
    Resistive,
    #[default]
    HiZ,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) struct Value(pub Level, pub Drive);
impl Value {
    pub(super) fn decode(code: u8) -> Self {
        Self(
            match code % 3 {
                0 => Low,
                1 => High,
                _ => Unknown,
            },
            match code / 3 {
                0 => Drive::Strong,
                1 => Drive::Resistive,
                2 => Drive::HiZ,
                _ => Drive::Unknown,
            },
        )
    }
    pub(crate) fn symbol(self) -> &'static str {
        const SYMBOLS: [[&str; 4]; 3] = [
            ["0s", "0r", "0z", "0u"],
            ["1s", "1r", "1z", "1u"],
            ["Us", "Ur", "Uz", "Uu"],
        ];
        SYMBOLS[self.0 as usize][self.1 as usize]
    }
}

/// Reduce the entire set at once. Equal strongest drivers must agree. An uncertain
/// drive can contend at any strength, but cannot weaken a definite strong driver.
pub(super) fn resolve(values: impl Iterator<Item = Value>) -> Value {
    let mut levels = [None; 4];
    for Value(level, drive) in values {
        let slot = &mut levels[drive as usize];
        *slot = Some(match *slot {
            Some(v) if v != level => Unknown,
            _ => level,
        });
    }
    let strongest = [Drive::Strong, Drive::Resistive, Drive::HiZ]
        .into_iter()
        .find(|&d| levels[d as usize].is_some());
    let uncertain = levels[Drive::Unknown as usize];
    match (strongest, uncertain) {
        (Some(d), None) => Value(levels[d as usize].unwrap_or(Unknown), d),
        (Some(d), Some(u)) => Value(
            if levels[d as usize] == Some(u) {
                u
            } else {
                Unknown
            },
            if d == Drive::Strong {
                d
            } else {
                Drive::Unknown
            },
        ),
        (None, Some(u)) => Value(u, Drive::Unknown),
        (None, None) => Value::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolution_is_independent_of_driver_order() {
        for i in 0..12 {
            for j in 0..12 {
                let a = Value::decode(i);
                let b = Value::decode(j);
                assert_eq!(resolve([a, b].into_iter()), resolve([b, a].into_iter()));
                assert_eq!(resolve([a].into_iter()), a);
            }
        }
        let strong = Value(Low, Drive::Strong);
        assert_eq!(
            resolve([Value(High, Drive::Resistive), strong].into_iter()),
            strong
        );
        assert_eq!(
            resolve([Value(High, Drive::Strong), strong].into_iter()),
            Value(Unknown, Drive::Strong)
        );
    }
}
