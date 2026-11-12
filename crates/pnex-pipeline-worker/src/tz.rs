//! The SQL functions `local_to_utc` and `utc_to_local` of a pipeline
//! (pipeline-engine.md D939), on the zone code of `pnex_core::pipeline::tz`.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime};
use datafusion::arrow::array::{Array, ArrayRef, StringArray, TimestampMicrosecondArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, TimeUnit};
use datafusion::error::{DataFusionError, Result as DfResult};
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use pnex_core::pipeline::tz::{utc_to_wall, wall_to_utc, zone, Tz};

/// Time zone of the timestamps the engine writes.
pub const UTC: &str = "+00:00";

fn micros(t: NaiveDateTime) -> i64 {
    t.and_utc().timestamp_micros()
}

fn naive(us: i64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp_micros(us).map(|t| t.naive_utc())
}

/// Applies `f` to (timestamp, zone) rows; a null in, a null out.
fn convert(
    args: &[ColumnarValue],
    out_tz: Option<&str>,
    f: fn(NaiveDateTime, &Tz) -> NaiveDateTime,
) -> DfResult<ColumnarValue> {
    let arrays = ColumnarValue::values_to_arrays(args)?;
    let [ts, zones] = arrays.as_slice() else {
        return Err(DataFusionError::Plan("two arguments expected".into()));
    };
    let ts = cast(ts, &DataType::Timestamp(TimeUnit::Microsecond, None))?;
    let ts = ts
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .ok_or_else(|| DataFusionError::Plan("timestamp expected".into()))?;
    let zones = cast(zones, &DataType::Utf8)?;
    let zones = zones
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| DataFusionError::Plan("time zone expected".into()))?;
    let mut parsed: HashMap<&str, Tz> = HashMap::new();
    let mut out = Vec::with_capacity(ts.len());
    for i in 0..ts.len() {
        if ts.is_null(i) || zones.is_null(i) {
            out.push(None);
            continue;
        }
        let name = zones.value(i);
        let tz = match parsed.get(name) {
            Some(tz) => *tz,
            None => {
                let tz = zone(name).map_err(DataFusionError::Execution)?;
                parsed.insert(name, tz);
                tz
            }
        };
        out.push(naive(ts.value(i)).map(|t| micros(f(t, &tz))));
    }
    let array =
        TimestampMicrosecondArray::from(out).with_timezone_opt(out_tz.map(Arc::<str>::from));
    Ok(ColumnarValue::Array(Arc::new(array) as ArrayRef))
}

/// `local_to_utc(ts, zone)` or `utc_to_local(ts, zone)`: any timestamp
/// precision in, microseconds out.
#[derive(Debug, PartialEq, Eq, Hash)]
struct ZoneFn {
    name: &'static str,
    to_utc: bool,
    signature: Signature,
}

impl ScalarUDFImpl for ZoneFn {
    fn name(&self) -> &str {
        self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, args: &[DataType]) -> DfResult<DataType> {
        if !matches!(args.first(), Some(DataType::Timestamp(..))) {
            return Err(DataFusionError::Plan(format!(
                "{}: the first argument is a timestamp",
                self.name
            )));
        }
        Ok(DataType::Timestamp(
            TimeUnit::Microsecond,
            self.to_utc.then(|| UTC.into()),
        ))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DfResult<ColumnarValue> {
        if self.to_utc {
            convert(&args.args, Some(UTC), wall_to_utc)
        } else {
            convert(&args.args, None, utc_to_wall)
        }
    }
}

/// `local_to_utc(ts, zone)` and `utc_to_local(ts, zone)`.
pub fn udfs() -> [ScalarUDF; 2] {
    [("local_to_utc", true), ("utc_to_local", false)].map(|(name, to_utc)| {
        ScalarUDF::new_from_impl(ZoneFn {
            name,
            to_utc,
            signature: Signature::any(2, Volatility::Immutable),
        })
    })
}
