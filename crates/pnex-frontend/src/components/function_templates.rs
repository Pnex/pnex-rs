//! Creation templates ("Start from" cards) — starter code per language.
//! Content is user-editable generated code (English, allowlisted from the
//! i18n guard like the skeletons).

use pnex_core::{FunctionInput, FunctionLanguage, FunctionOutput, FunctionSignature, FunctionType};

/// Which starter template the new function starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TemplateKind {
    #[default]
    Empty,
    ThresholdAlarm,
    UnitConvert,
    PayloadParser,
}

impl TemplateKind {
    pub fn all() -> [TemplateKind; 4] {
        [
            TemplateKind::Empty,
            TemplateKind::ThresholdAlarm,
            TemplateKind::UnitConvert,
            TemplateKind::PayloadParser,
        ]
    }

    /// i18n key of the card title / signature line.
    pub fn title_key(self) -> &'static str {
        match self {
            TemplateKind::Empty => "functions-tpl-empty",
            TemplateKind::ThresholdAlarm => "functions-tpl-threshold",
            TemplateKind::UnitConvert => "functions-tpl-unit",
            TemplateKind::PayloadParser => "functions-tpl-payload",
        }
    }

    pub fn sig_key(self) -> &'static str {
        match self {
            TemplateKind::Empty => "functions-tpl-empty-sig",
            TemplateKind::ThresholdAlarm => "functions-tpl-threshold-sig",
            TemplateKind::UnitConvert => "functions-tpl-unit-sig",
            TemplateKind::PayloadParser => "functions-tpl-payload-sig",
        }
    }
}

/// Declared interface of a template (same for both languages, D497).
pub fn template_signature(kind: TemplateKind) -> FunctionSignature {
    let input = |name: &str, default: Option<f64>, desc: &str| FunctionInput {
        name: name.into(),
        ty: FunctionType::Number,
        default: default.map(|d| serde_json::json!(d)),
        desc: Some(desc.into()),
    };
    let output = |name: &str, ty, desc: &str| FunctionOutput {
        name: name.into(),
        ty,
        desc: Some(desc.into()),
    };
    let (inputs, outputs) = match kind {
        TemplateKind::Empty => (
            vec![input("value", None, "Measured value")],
            vec![output("result", FunctionType::Number, "Computed result")],
        ),
        TemplateKind::ThresholdAlarm => (
            vec![
                input("value", None, "Measured value"),
                input("threshold", Some(20.0), "Alarm threshold"),
            ],
            vec![output(
                "alarm",
                FunctionType::Bool,
                "True when value exceeds the threshold",
            )],
        ),
        TemplateKind::UnitConvert => (
            vec![
                input("raw", None, "Raw sensor value"),
                input("factor", Some(1.0), "Scale factor"),
                input("offset", Some(0.0), "Offset applied after scaling"),
            ],
            vec![output("value", FunctionType::Number, "Converted value")],
        ),
        TemplateKind::PayloadParser => (
            vec![],
            vec![
                output(
                    "temperature",
                    FunctionType::Number,
                    "Parsed temperature (°C)",
                ),
                output("humidity", FunctionType::Number, "Parsed humidity (%)"),
            ],
        ),
    };
    FunctionSignature { inputs, outputs }
}

/// Starter code per (template, language) — contract `handle(inputs, msg)`.
pub fn template_code(kind: TemplateKind, lang: FunctionLanguage) -> String {
    match (kind, lang) {
        (TemplateKind::Empty, FunctionLanguage::Js) => r#"function handle(inputs, msg) {
  const result = inputs.value * 2;
  return { result };
}"#
        .to_string(),
        (TemplateKind::Empty, FunctionLanguage::Starlark) => r#"def handle(inputs, msg):
    result = inputs["value"] * 2
    return {"result": result}"#
            .to_string(),
        (TemplateKind::ThresholdAlarm, FunctionLanguage::Js) => r#"function handle(inputs, msg) {
  return { alarm: inputs.value > inputs.threshold };
}"#
        .to_string(),
        (TemplateKind::ThresholdAlarm, FunctionLanguage::Starlark) => r#"def handle(inputs, msg):
    return {"alarm": inputs["value"] > inputs["threshold"]}"#
            .to_string(),
        (TemplateKind::UnitConvert, FunctionLanguage::Js) => r#"function handle(inputs, msg) {
  const value = inputs.raw * inputs.factor + inputs.offset;
  return { value };
}"#
        .to_string(),
        (TemplateKind::UnitConvert, FunctionLanguage::Starlark) => r#"def handle(inputs, msg):
    value = inputs["raw"] * inputs["factor"] + inputs["offset"]
    return {"value": value}"#
            .to_string(),
        (TemplateKind::PayloadParser, FunctionLanguage::Js) => r#"function handle(inputs, msg) {
  const data = JSON.parse(msg.payload);
  return {
    temperature: data.t ?? null,
    humidity: data.h ?? null,
  };
}"#
        .to_string(),
        (TemplateKind::PayloadParser, FunctionLanguage::Starlark) => r#"def handle(inputs, msg):
    data = json.decode(msg["payload"])
    return {"temperature": data.get("t"), "humidity": data.get("h")}"#
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh function must open without the "used but not declared"
    /// banner: every template declares the ports its code reads and returns.
    #[test]
    fn every_template_declares_its_ports() {
        for kind in TemplateKind::all() {
            for lang in [FunctionLanguage::Js, FunctionLanguage::Starlark] {
                let code = template_code(kind, lang);
                let sig = template_signature(kind);
                assert!(sig.validate().is_ok(), "{kind:?}");
                let undeclared =
                    crate::components::function_analysis::analyze_undeclared(&code, lang, &sig);
                assert!(undeclared.is_empty(), "{kind:?}/{lang:?}: {undeclared:?}");
            }
        }
    }
}
