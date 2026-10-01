//! Mélanges de fluides personnalisés — DTO partagés + validation
//! (exécutée dans le navigateur avant save ET côté serveur en 400
//! `violations`, école `viz.rs`). Pur, wasm-safe : la conversion
//! massique→molaire reçoit les masses molaires CoolProp en argument —
//! ce crate ne lie jamais `pnex-coolprop`.
//!
//! Shape stocké en JSONB (`fluid_mixtures.composition`) : la base saisie
//! (`basis`) + les composants tels qu'entrés + les fractions molaires
//! converties à la sauvegarde (`mole_fractions`, même ordre) + la masse
//! molaire du mélange (`molar_mass`, kg/mol) — le rendu et les specs
//! CoolProp (`"Propane[0.5]&Ethane[0.5]"`) consomment directement les
//! valeurs molaires sans re-conversion.

use serde::{Deserialize, Serialize};

/// Base des fractions saisies. CoolProp calcule nativement en molaires ;
/// une saisie massique est convertie à la sauvegarde via les masses
/// molaires CoolProp.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Un composant du mélange : nom de fluide CoolProp (pure) + fraction
/// dans la base du mélange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixtureComponent {
    pub fluid: String,
    pub fraction: f64,
}

/// Composition d'un mélange, telle que stockée en JSONB.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FluidMixtureComposition {
    pub basis: MixtureBasis,
    pub components: Vec<MixtureComponent>,
    /// Fractions molaires (même ordre que `components`), converties à la
    /// sauvegarde ; identiques aux fractions saisies si `basis == Mole`.
    #[serde(default)]
    pub mole_fractions: Vec<f64>,
    /// Masse molaire du mélange (kg/mol), calculée à la sauvegarde.
    #[serde(default)]
    pub molar_mass: Option<f64>,
}

/// Violation de validation d'un mélange (field = champ du formulaire).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixtureViolation {
    pub field: Option<String>,
    pub code: String,
    pub message: String,
}

impl MixtureViolation {
    pub fn new(field: Option<&str>, code: &str, message: impl Into<String>) -> Self {
        Self {
            field: field.map(str::to_string),
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// Tolérance sur la somme des fractions.
pub const FRACTION_SUM_TOL: f64 = 1e-6;
/// Bornes raisonnables (un mélange CoolProp reste petit).
pub const MAX_COMPONENTS: usize = 20;

/// Réponse API d'un mélange : identité + composition stockée (avec
/// `mole_fractions` converties) + spec CoolProp inline prête à l'emploi
/// (`"Propane[0.5]&Ethane[0.5]"`), ou `None` si la composition n'a pas
/// encore été convertie.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluidMixture {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub spec: Option<String>,
    pub composition: FluidMixtureComposition,
    pub created_at: String,
    pub updated_at: String,
}

/// Corps de création/modification d'un mélange (le serveur complète
/// `mole_fractions`/`molar_mass` après conversion).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FluidMixtureInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub composition: FluidMixtureComposition,
}

impl FluidMixtureComposition {
    /// Validation structurelle partagée front/back. La vérification
    /// d'instanciability CoolProp (fluides réels) est côté serveur
    /// uniquement (`pnex_coolprop::validate_fluid_spec`).
    pub fn validate(&self) -> Vec<MixtureViolation> {
        let mut v = Vec::new();
        if self.components.is_empty() {
            v.push(MixtureViolation::new(
                Some("composition"),
                "components_missing",
                "au moins un composant est requis",
            ));
            return v;
        }
        if self.components.len() > MAX_COMPONENTS {
            v.push(MixtureViolation::new(
                Some("composition"),
                "too_many_components",
                format!("au plus {MAX_COMPONENTS} composants"),
            ));
        }
        for (i, c) in self.components.iter().enumerate() {
            if c.fluid.trim().is_empty() {
                v.push(MixtureViolation::new(
                    Some("composition"),
                    "fluid_missing",
                    format!("composant {} : nom de fluide requis", i + 1),
                ));
            }
            if !c.fraction.is_finite() || c.fraction <= 0.0 {
                v.push(MixtureViolation::new(
                    Some("composition"),
                    "fraction_invalid",
                    format!("composant {} : fraction > 0 requise", i + 1),
                ));
            }
        }
        let sum: f64 = self.components.iter().map(|c| c.fraction).sum();
        if !v.iter().any(|x| x.code == "fraction_invalid") && (sum - 1.0).abs() > FRACTION_SUM_TOL {
            v.push(MixtureViolation::new(
                Some("composition"),
                "fraction_sum",
                format!("la somme des fractions doit valoir 1 (ici {sum:.6})"),
            ));
        }
        // mole_fractions : même longueur que components (présent ⇔ la
        // composition a été validée + convertie côté serveur).
        if !self.mole_fractions.is_empty() && self.mole_fractions.len() != self.components.len() {
            v.push(MixtureViolation::new(
                Some("composition"),
                "mole_fractions_length",
                "mole_fractions doit avoir la même longueur que components",
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
            return Err("fractions molaires non calculées (composition non convertie)".into());
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
            return Err("masses molaires : une par composant attendue".into());
        }
        let mols: Vec<f64> = self
            .components
            .iter()
            .zip(molar_masses)
            .map(|(c, m)| {
                if !m.is_finite() || *m <= 0.0 {
                    return Err(format!("masse molaire invalide pour {}", c.fluid.trim()));
                }
                Ok(c.fraction / m)
            })
            .collect::<Result<_, _>>()?;
        let total: f64 = mols.iter().sum();
        if !total.is_finite() || total <= 0.0 {
            return Err("somme des moles nulle ou invalide".into());
        }
        Ok(mols.iter().map(|n| n / total).collect())
    }

    /// Masse molaire du mélange en kg/mol depuis des masses molaires de
    /// composants (kg/mol) : M = Σx_i·M_i en base molaire,
    /// M = 1/Σ(w_i/M_i) en base massique.
    pub fn mixture_molar_mass(&self, molar_masses: &[f64]) -> Result<f64, String> {
        if molar_masses.len() != self.components.len() {
            return Err("masses molaires : une par composant attendue".into());
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
                    return Err("somme des moles nulle ou invalide".into());
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
        assert_eq!(m.validate()[0].code, "fraction_sum");

        m.components[1].fluid = "  ".into();
        assert!(m.validate().iter().any(|v| v.code == "fluid_missing"));

        m.components.clear();
        assert_eq!(m.validate()[0].code, "components_missing");
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
