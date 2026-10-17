//! Custom fluid mixtures: shared DTOs + validation (run in the browser
//! before saving AND on the server as `mixture-invalid` violations).
//! Pure and wasm-safe: the mass-to-mole conversion receives the CoolProp
//! molar masses as an argument, this crate never links `pnex-coolprop`.
//!
//! Stored JSONB shape (`fluid_mixtures.composition`): the entered basis
//! (`basis`) + the components as entered + the mole fractions converted on
//! save (`mole_fractions`, same order) + the mixture molar mass
//! (`molar_mass`, kg/mol). Rendering and CoolProp specs
//! (`"Propane[0.5]&Ethane[0.5]"`) read the mole values directly.

use serde::{Deserialize, Serialize};

/// Basis of the entered fractions. CoolProp computes in mole fractions; a
/// mass entry is converted on save with the CoolProp molar masses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum MixtureBasis {
    #[default]
    Mole,
    Mass,
}

impl MixtureBasis {
    pub fn as_str(&self) -> &'static str {
        match self {
            MixtureBasis::Mole => "mole",
            MixtureBasis::Mass => "mass",
        }
    }
}

/// One component of the mixture: a pure CoolProp fluid name + its fraction
/// in the mixture basis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MixtureComponent {
    pub fluid: String,
    pub fraction: f64,
}

/// Composition of a mixture, as stored in JSONB.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FluidMixtureComposition {
    pub basis: MixtureBasis,
    pub components: Vec<MixtureComponent>,
    /// Mole fractions (same order as `components`), converted on save;
    /// equal to the entered fractions when `basis` is `mole`.
    #[serde(default)]
    pub mole_fractions: Vec<f64>,
    /// Molar mass of the mixture (kg/mol), computed on save.
    #[serde(default)]
    pub molar_mass: Option<f64>,
}

/// Validation violation of a mixture: machine `code` (resolved by the UI as
/// `err-<code>` with `args`), canonical English `message` as fallback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixtureViolation {
    pub field: Option<String>,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

impl MixtureViolation {
    pub fn new(field: Option<&str>, code: &str, message: impl Into<String>) -> Self {
        Self {
            field: field.map(str::to_string),
            code: code.to_string(),
            message: message.into(),
            args: None,
        }
    }

    fn with_args(mut self, args: serde_json::Value) -> Self {
        self.args = Some(args);
        self
    }
}

/// Tolerance on the sum of the fractions.
pub const FRACTION_SUM_TOL: f64 = 1e-6;
/// Upper bound on components (a CoolProp mixture stays small).
pub const MAX_COMPONENTS: usize = 20;

/// API answer for a mixture: identity + stored composition (with the
/// converted `mole_fractions`) + ready-to-use inline CoolProp spec
/// (`"Propane[0.5]&Ethane[0.5]"`), `None` when the composition is not
/// converted yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FluidMixture {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub spec: Option<String>,
    pub composition: FluidMixtureComposition,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Create/replace body of a mixture (the server fills
/// `mole_fractions`/`molar_mass` after conversion).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FluidMixtureInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub composition: FluidMixtureComposition,
}

impl FluidMixtureComposition {
    /// Structural validation shared by the browser and the server. The
    /// CoolProp instantiation check (real fluids) is server-side only
    /// (`pnex_coolprop::validate_fluid_spec`).
    pub fn validate(&self) -> Vec<MixtureViolation> {
        let mut v = Vec::new();
        if self.components.is_empty() {
            v.push(MixtureViolation::new(
                Some("composition"),
                "mixture-components-missing",
                "at least one component is required",
            ));
            return v;
        }
        if self.components.len() > MAX_COMPONENTS {
            v.push(
                MixtureViolation::new(
                    Some("composition"),
                    "mixture-too-many-components",
                    format!("at most {MAX_COMPONENTS} components"),
                )
                .with_args(serde_json::json!({ "max": MAX_COMPONENTS.to_string() })),
            );
        }
        for (i, c) in self.components.iter().enumerate() {
            let index = serde_json::json!({ "index": (i + 1).to_string() });
            if c.fluid.trim().is_empty() {
                v.push(
                    MixtureViolation::new(
                        Some("composition"),
                        "mixture-fluid-missing",
                        format!("component {}: fluid name required", i + 1),
                    )
                    .with_args(index.clone()),
                );
            }
            if !c.fraction.is_finite() || c.fraction <= 0.0 {
                v.push(
                    MixtureViolation::new(
                        Some("composition"),
                        "mixture-fraction-invalid",
                        format!("component {}: fraction must be > 0", i + 1),
                    )
                    .with_args(index),
                );
            }
        }
        let sum: f64 = self.components.iter().map(|c| c.fraction).sum();
        if !v.iter().any(|x| x.code == "mixture-fraction-invalid")
            && (sum - 1.0).abs() > FRACTION_SUM_TOL
        {
            v.push(
                MixtureViolation::new(
                    Some("composition"),
                    "mixture-fraction-sum",
                    format!("the fractions must add up to 1 (got {sum:.6})"),
                )
                .with_args(serde_json::json!({ "sum": format!("{sum:.6}") })),
            );
        }
        // mole_fractions: same length as components (present when the
        // server validated and converted the composition).
        if !self.mole_fractions.is_empty() && self.mole_fractions.len() != self.components.len() {
            v.push(MixtureViolation::new(
                Some("composition"),
                "mixture-mole-fractions-length",
                "mole_fractions must have the same length as components",
            ));
        }
        v
    }

    /// Spec CoolProp inline depuis les fractions molaires stockées
    /// (`"Propane[0.5]&Ethane[0.5]"`, sans préfixe de backend — HEOS est
    /// le défaut). Erreur si les fractions molaires ne sont pas encore
    /// calculées (composition non convertie).
    pub fn to_coolprop_spec(&self) -> Result<String, String> {
        if self.mole_fractions.len() != self.components.len() {
            return Err("mole fractions not computed (composition not converted)".into());
        }
        let parts: Vec<String> = self
            .components
            .iter()
            .zip(&self.mole_fractions)
            .map(|(c, x)| format!("{}[{}]", c.fluid.trim(), x))
            .collect();
        Ok(parts.join("&"))
    }

    /// Conversion massique→molaire : x_i = (w_i/M_i) / Σ(w_j/M_j).
    /// `molar_masses` en kg/mol, même ordre que `components`. Pure et
    /// testable — la lecture CoolProp des masses molaires est côté serveur.
    pub fn to_mole_fractions(&self, molar_masses: &[f64]) -> Result<Vec<f64>, String> {
        if molar_masses.len() != self.components.len() {
            return Err("molar masses: one per component expected".into());
        }
        let mols: Vec<f64> = self
            .components
            .iter()
            .zip(molar_masses)
            .map(|(c, m)| {
                if !m.is_finite() || *m <= 0.0 {
                    return Err(format!("invalid molar mass for {}", c.fluid.trim()));
                }
                Ok(c.fraction / m)
            })
            .collect::<Result<_, _>>()?;
        let total: f64 = mols.iter().sum();
        if !total.is_finite() || total <= 0.0 {
            return Err("zero or invalid mole sum".into());
        }
        Ok(mols.iter().map(|n| n / total).collect())
    }

    /// Masse molaire du mélange en kg/mol depuis des masses molaires de
    /// composants (kg/mol) : M = Σx_i·M_i en base molaire,
    /// M = 1/Σ(w_i/M_i) en base massique.
    pub fn mixture_molar_mass(&self, molar_masses: &[f64]) -> Result<f64, String> {
        if molar_masses.len() != self.components.len() {
            return Err("molar masses: one per component expected".into());
        }
        match self.basis {
            MixtureBasis::Mole => Ok(self
                .components
                .iter()
                .zip(molar_masses)
                .map(|(c, m)| c.fraction * m)
                .sum()),
            MixtureBasis::Mass => {
                let inv: f64 = self
                    .components
                    .iter()
                    .zip(molar_masses)
                    .map(|(c, m)| c.fraction / m)
                    .sum();
                if !inv.is_finite() || inv <= 0.0 {
                    return Err("zero or invalid mole sum".into());
                }
                Ok(1.0 / inv)
            }
        }
    }
}

/// Règle : la base est uniforme par mélange — c'est structurellement vrai
/// ici (`basis` unique sur la composition), la conversion massique est
/// faite à la sauvegarde (`mole_fractions` toujours présentes en base).
#[cfg(test)]
mod tests {
    use super::*;

    fn mole(components: &[(&str, f64)]) -> FluidMixtureComposition {
        let components: Vec<MixtureComponent> = components
            .iter()
            .map(|(f, x)| MixtureComponent {
                fluid: f.to_string(),
                fraction: *x,
            })
            .collect();
        FluidMixtureComposition {
            basis: MixtureBasis::Mole,
            mole_fractions: components.iter().map(|c| c.fraction).collect(),
            components,
            molar_mass: None,
        }
    }

    #[test]
    fn validation_rejete_les_compositions_invalides() {
        let mut m = mole(&[("Propane", 0.5), ("Ethane", 0.5)]);
        assert!(m.validate().is_empty());

        m.components[0].fraction = 0.7;
        m.mole_fractions = vec![0.7, 0.5];
        assert_eq!(m.validate()[0].code, "mixture-fraction-sum");

        m.components[1].fluid = "  ".into();
        assert!(m
            .validate()
            .iter()
            .any(|v| v.code == "mixture-fluid-missing"));

        m.components.clear();
        assert_eq!(m.validate()[0].code, "mixture-components-missing");
    }

    #[test]
    fn conversion_massique_vers_molaire() {
        // 50 % massique eau (M=0.018) / 50 % éthanol (M=0.046) :
        // moles 27.78/10.87 → x_eau ≈ 0.7187.
        let m = FluidMixtureComposition {
            basis: MixtureBasis::Mass,
            components: vec![
                MixtureComponent {
                    fluid: "Water".into(),
                    fraction: 0.5,
                },
                MixtureComponent {
                    fluid: "Ethanol".into(),
                    fraction: 0.5,
                },
            ],
            mole_fractions: vec![],
            molar_mass: None,
        };
        let x = m.to_mole_fractions(&[0.01801528, 0.04607]).unwrap();
        assert!((x[0] - 0.7187).abs() < 1e-3, "x_eau = {x:?}");
        assert!((x[0] + x[1] - 1.0).abs() < 1e-12);

        let mm = m.mixture_molar_mass(&[0.01801528, 0.04607]).unwrap();
        assert!((mm - (1.0 / (0.5 / 0.01801528 + 0.5 / 0.04607))).abs() < 1e-12);
    }

    #[test]
    fn spec_coolprop_depuis_fractions_molaires() {
        let m = mole(&[("Propane", 0.5), ("Ethane", 0.5)]);
        assert_eq!(m.to_coolprop_spec().unwrap(), "Propane[0.5]&Ethane[0.5]");

        let mut non_converti = m.clone();
        non_converti.mole_fractions.clear();
        assert!(non_converti.to_coolprop_spec().is_err());
    }
}
