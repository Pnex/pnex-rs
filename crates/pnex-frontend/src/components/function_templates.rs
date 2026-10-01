//! Creation templates ("Partir de" cards) — starter code per language.
//! Content is user-editable generated code (English, allowlisted from the
//! i18n guard like the skeletons).

use pnex_core::FunctionLanguage;

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

/// Starter code per (template, language) — contract `handle(inputs, msg)`.
pub fn template_code(kind: TemplateKind, lang: FunctionLanguage) -> String {
    match (kind, lang) {
        (TemplateKind::Empty, FunctionLanguage::Js) => r#"// @input value number "Measured value"
// @output result number "Computed result"
function handle(inputs, msg) {
  const result = inputs.value * 2;
  return { result };
}"#
        .to_string(),
        (TemplateKind::Empty, FunctionLanguage::Starlark) => {
            r#"# @input value number "Measured value"
# @output result number "Computed result"
def handle(inputs, msg):
    result = inputs["value"] * 2
    return {"result": result}"#
                .to_string()
        }
        (TemplateKind::ThresholdAlarm, FunctionLanguage::Js) => {
            r#"// @input value number "Measured value"
// @input threshold number=20 "Alarm threshold"
// @output alarm bool "True when value exceeds the threshold"
function handle(inputs, msg) {
  return { alarm: inputs.value > inputs.threshold };
}"#
            .to_string()
        }
        (TemplateKind::ThresholdAlarm, FunctionLanguage::Starlark) => {
            r#"# @input value number "Measured value"
# @output alarm bool "True when value exceeds the threshold"
def handle(inputs, msg):
    return {"alarm": inputs["value"] > inputs["threshold"]}"#
                .to_string()
        }
        (TemplateKind::UnitConvert, FunctionLanguage::Js) => {
            r#"// @input raw number "Raw sensor value"
// @input factor number=1 "Scale factor"
// @input offset number=0 "Offset applied after scaling"
// @output value number "Converted value"
function handle(inputs, msg) {
  const value = inputs.raw * inputs.factor + inputs.offset;
  return { value };
}"#
            .to_string()
        }
        (TemplateKind::UnitConvert, FunctionLanguage::Starlark) => {
            r#"# @input raw number "Raw sensor value"
# @input factor number=1 "Scale factor"
# @input offset number=0 "Offset applied after scaling"
# @output value number "Converted value"
def handle(inputs, msg):
    value = inputs["raw"] * inputs["factor"] + inputs["offset"]
    return {"value": value}"#
                .to_string()
        }
        (TemplateKind::PayloadParser, FunctionLanguage::Js) => {
            r#"// @output temperature number "Parsed temperature (°C)"
// @output humidity number "Parsed humidity (%)"
function handle(inputs, msg) {
  const data = JSON.parse(msg.payload);
  return {
    temperature: data.t ?? null,
    humidity: data.h ?? null,
  };
}"#
            .to_string()
        }
        (TemplateKind::PayloadParser, FunctionLanguage::Starlark) => {
            r#"# @output temperature number "Parsed temperature (°C)"
# @output humidity number "Parsed humidity (%)"
def handle(inputs, msg):
    data = json.decode(msg["payload"])
    return {"temperature": data.get("t"), "humidity": data.get("h")}"#
                .to_string()
        }
    }
}
