//! Plot namespaces and conversion between batch vectors and simulation output.
//! Provenance: original implementation from the ngspice user manual and Sansho output types.

use super::{Result, error, expr::Vector};
use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity, Variable};
use sansho_sparse::c64 as C;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug)]
pub(super) struct Plot {
    pub name: String,
    pub analysis: Option<AnalysisResult>,
    pub locals: BTreeMap<String, Vector>,
    pub grid: Option<(f64, f64, f64)>,
}
impl Plot {
    pub fn scale(&self) -> Option<Arc<[f64]>> {
        self.analysis
            .as_ref()
            .filter(|a| a.scale.is_some())
            .map(|a| a.scale_values.as_slice().into())
    }
    pub fn get(&self, name: &str) -> Option<Vector> {
        if let Some(v) = self.locals.get(name) {
            return Some(v.clone());
        }
        let a = self.analysis.as_ref()?;
        let mut v = if a
            .scale
            .as_ref()
            .is_some_and(|s| s.name.eq_ignore_ascii_case(name))
        {
            Vector::real(a.scale_values.iter().copied())
        } else if let Some(column) = a.real(name) {
            Vector::real(column.iter().copied())
        } else {
            Vector {
                values: a
                    .complex(name)?
                    .iter()
                    .map(|&(re, im)| C::new(re, im))
                    .collect(),
                complex: true,
                scale: None,
            }
        };
        v.scale = self.scale();
        Some(v)
    }
    pub fn remove(&mut self, name: &str) -> Result<()> {
        let local = self.locals.remove(name);
        if let Some(a) = &mut self.analysis
            && let Some(i) = a.index_of(name)
        {
            a.variables.remove(i);
            match &mut a.data {
                Data::Real { columns } => {
                    columns.remove(i);
                }
                Data::Complex { columns } => {
                    columns.remove(i);
                }
            }
            return Ok(());
        }
        if local.is_some() {
            Ok(())
        } else {
            Err(error(format!("unknown or protected vector {name}")))
        }
    }
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .analysis
            .iter()
            .flat_map(|a| a.variables.iter().map(|v| v.name.clone()))
            .collect();
        names.extend(
            self.locals
                .keys()
                .filter(|k| {
                    self.analysis
                        .as_ref()
                        .is_none_or(|a| a.index_of(k).is_none())
                })
                .cloned(),
        );
        names
    }
    pub fn result(&self, selected: &[(String, Vector)]) -> Result<AnalysisResult> {
        let a = self.analysis.as_ref();
        let n = a.map_or_else(
            || {
                selected
                    .iter()
                    .map(|(_, v)| v.values.len())
                    .max()
                    .unwrap_or(0)
            },
            AnalysisResult::len,
        );
        let scale = a.and_then(|a| a.scale.as_ref());
        let mut variables = vec![];
        let mut cols = vec![];
        let mut complex = false;
        for (name, v) in selected {
            if scale.is_some_and(|s| s.name == *name) {
                continue;
            }
            if v.values.len() != n && v.values.len() != 1 {
                return Err(error(format!(
                    "{name} has a different length from the plot scale"
                )));
            }
            let quantity = a
                .and_then(|a| a.index_of(name).map(|i| a.variables[i].quantity))
                .unwrap_or(Quantity::Scalar);
            variables.push(Variable {
                name: name.clone(),
                quantity,
            });
            let column = if v.values.len() == 1 {
                vec![v.values[0]; n]
            } else {
                v.values.clone()
            };
            complex |= v.complex;
            cols.push(column);
        }
        let data = if complex {
            Data::Complex {
                columns: cols
                    .into_iter()
                    .map(|v| v.into_iter().map(|z| (z.re, z.im)).collect())
                    .collect(),
            }
        } else {
            Data::Real {
                columns: cols
                    .into_iter()
                    .map(|v| v.into_iter().map(|z| z.re).collect())
                    .collect(),
            }
        };
        Ok(AnalysisResult {
            events: a.map(|plot| plot.events.clone()).unwrap_or_default(),
            kind: a.map_or(AnalysisKind::Vectors, |a| a.kind),
            scale: scale.cloned(),
            scale_values: a.map(|a| a.scale_values.clone()).unwrap_or_default(),
            variables,
            data,
        })
    }
    pub fn snapshot(&self) -> Result<AnalysisResult> {
        if let Some(a) = &self.analysis
            && self.locals.is_empty()
        {
            return Ok(a.clone());
        }
        let n = self.analysis.as_ref().map_or_else(
            || {
                self.locals
                    .values()
                    .map(|v| v.values.len())
                    .max()
                    .unwrap_or(0)
            },
            AnalysisResult::len,
        );
        let selected = self
            .names()
            .into_iter()
            .filter_map(|name| {
                let v = self.get(&name)?;
                (v.values.len() == n || v.values.len() == 1).then_some((name, v))
            })
            .collect::<Vec<_>>();
        self.result(&selected)
    }
}

pub(super) fn prefix(kind: AnalysisKind) -> &'static str {
    match kind {
        AnalysisKind::Vectors => "unknown",
        AnalysisKind::Tran => "tran",
        AnalysisKind::Op => "op",
        AnalysisKind::Ac => "ac",
        AnalysisKind::Dc => "dc",
        AnalysisKind::Sp => "sp",
        AnalysisKind::Sens => "sens",
        AnalysisKind::Four => "four",
        AnalysisKind::Pz => "pz",
        AnalysisKind::Noise | AnalysisKind::IntegratedNoise => "noise",
        AnalysisKind::Tf => "tf",
        _ => "disto",
    }
}
