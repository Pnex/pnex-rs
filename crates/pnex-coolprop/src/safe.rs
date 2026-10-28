//! Safe wrapper around the raw CoolProp C API.
//!
//! Conventions implemented here (see `vendor/CoolProp/src/CoolPropLib.cpp`):
//!
//! * Every C call is serialised behind a global mutex: several CoolProp
//!   entry points mutate the process-global configuration or the
//!   global error string, and the AbstractState handle registry is
//!   shared.
//! * The global errstring is *drained* (reading it clears it) before every call
//!   that reports its errors that way, so that a failure indicator
//!   after the call always matches a fresh message.
//! * `errcode`-style calls map `0` → success, anything else → the
//!   buffered message.
//! * `double` scalars signal failure by returning `_HUGE`
//!   (= `HUGE_VAL`).

use std::ffi::{c_char, c_double, c_int, c_long, CString};
use std::sync::{Mutex, MutexGuard};

use pnex_coolprop_sys as sys;

/// Size of the buffer used for each `message_buffer` / string out-param.
pub const ERRBUF: usize = 10_000;

/// Serialises all CoolProp FFI calls (see the module doc).
static FFI: Mutex<()> = Mutex::new(());

/// Error carrying the CoolProp message.
#[derive(Debug, Clone)]
pub struct CoolPropError(pub String);

pub type Result<T> = std::result::Result<T, CoolPropError>;

fn lock() -> MutexGuard<'static, ()> {
    FFI.lock().unwrap_or_else(|e| e.into_inner())
}

fn cstr(s: &str) -> CString {
    // Interior NUL bytes cannot be represented in a C string;
    // strip them rather than panic on user input.
    CString::new(s.replace('\0', "")).expect("string with NUL bytes removed cannot fail")
}

fn buffer_to_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Reads a global parameter string; `None` if CoolProp signals a failure.
/// Call while holding the FFI lock.
fn global_param_string(param: &str) -> Option<String> {
    let p = cstr(param);
    let mut buf = vec![0u8; ERRBUF];
    let rc = unsafe {
        sys::get_global_param_string(
            p.as_ptr(),
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len() as c_int,
        )
    };
    (rc == 1).then(|| buffer_to_string(&buf))
}

/// Drain-and-return of the global errstring (reading it clears it in CoolProp).
fn errstring() -> String {
    global_param_string("errstring").unwrap_or_default()
}

/// Decodes the `errcode` / `message_buffer` result pattern.
fn check_errcode(errcode: c_long, msg: &[u8]) -> Result<()> {
    match errcode {
        0 => Ok(()),
        1 => Err(CoolPropError(buffer_to_string(msg))),
        2 => Err(CoolPropError(
            "CoolProp error message did not fit in the message buffer".into(),
        )),
        other => Err(CoolPropError(format!(
            "unknown CoolProp error (code {other})"
        ))),
    }
}

/// Drains the errstring, runs the void C call, re-reads the errstring: the
/// void setters (`set_config_*`, ...) report their failures *only* that
/// way.
fn void_call_report_errstring<F>(f: F) -> Result<()>
where
    F: FnOnce(),
{
    let _ = errstring();
    f();
    let msg = errstring();
    if msg.is_empty() {
        Ok(())
    } else {
        Err(CoolPropError(msg))
    }
}

fn fresh_errstring_or_default() -> String {
    let msg = errstring();
    if msg.is_empty() {
        "CoolProp call failed (no error message was set)".to_string()
    } else {
        msg
    }
}

/// Checks a `double` scalar against the `_HUGE` failure sentinel.
fn check_scalar(v: f64) -> Result<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CoolPropError(fresh_errstring_or_default()))
    }
}

// ----------------------------------------------------------------------
// High-level property accessors
// ----------------------------------------------------------------------

/// `PropsSI(Output, Name1, Prop1, Name2, Prop2, FluidName)`
pub fn props_si(
    output: &str,
    name1: &str,
    prop1: f64,
    name2: &str,
    prop2: f64,
    fluid: &str,
) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (o, n1, n2, f) = (cstr(output), cstr(name1), cstr(name2), cstr(fluid));
    let v = unsafe {
        sys::PropsSI(
            o.as_ptr(),
            n1.as_ptr(),
            prop1,
            n2.as_ptr(),
            prop2,
            f.as_ptr(),
        )
    };
    check_scalar(v)
}

/// `Props1SI(FluidName, Output)` — single-state output with no input
/// (e.g. `"Tcrit"`, `"Pcrit"`, `"molar_mass"`).
pub fn props_1si(fluid: &str, output: &str) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (f, o) = (cstr(fluid), cstr(output));
    let v = unsafe { sys::Props1SI(f.as_ptr(), o.as_ptr()) };
    check_scalar(v)
}

/// `PropsSImulti(...)` — vectorised PropsSI. Returns one row per input
/// point, one element per output (mirrors CoolProp's `[points][outputs]`).
#[allow(clippy::too_many_arguments)] // mirrors the C signature
pub fn props_si_multi(
    outputs: &[String],
    name1: &str,
    prop1: &[f64],
    name2: &str,
    prop2: &[f64],
    backend: &str,
    fluids: &[String],
    fractions: &[f64],
) -> Result<Vec<Vec<f64>>> {
    let _g = lock();
    let _ = errstring();
    if prop1.len() != prop2.len() {
        return Err(CoolPropError(format!(
            "prop1 has {} values but prop2 has {}",
            prop1.len(),
            prop2.len()
        )));
    }
    let outs = cstr(&outputs.join(","));
    let (n1, n2) = (cstr(name1), cstr(name2));
    let mut p1 = prop1.to_vec();
    let mut p2 = prop2.to_vec();
    let backend_buf = cstr(backend);
    let fl = cstr(&fluids.join(","));
    let fr = fractions.to_vec();
    let n_out = outputs.len().max(1);
    let n_pts = prop1.len().max(1);
    let mut result = vec![0f64; n_out * n_pts];
    // In/out capacity dimensions: the C wrapper checks
    // `_result.size() > *resdim1 || _result[0].size() > *resdim2` where the
    // rows are the input points and the columns the outputs
    // (CoolProp IO: [points][outputs]), then writes the real sizes back.
    let (mut d1, mut d2) = (n_pts as c_long, n_out as c_long);
    unsafe {
        sys::PropsSImulti(
            outs.as_ptr(),
            n1.as_ptr(),
            p1.as_mut_ptr(),
            p1.len() as c_long,
            n2.as_ptr(),
            p2.as_mut_ptr(),
            p2.len() as c_long,
            backend_buf.as_ptr() as *mut c_char,
            fl.as_ptr(),
            fr.as_ptr(),
            fr.len() as c_long,
            result.as_mut_ptr(),
            &mut d1,
            &mut d2,
        );
    }
    if d1 == 0 || d2 == 0 {
        return Err(CoolPropError(fresh_errstring_or_default()));
    }
    let rows = d1 as usize;
    let cols = d2 as usize;
    Ok((0..rows)
        .map(|i| result[i * cols..(i + 1) * cols].to_vec())
        .collect())
}

/// `Props1SImulti(...)` — multi-fluid Props1SI. Mirrors the C wrapper, which
/// only exposes the first row of results (see CoolPropLib.cpp).
pub fn props_1_si_multi(
    outputs: &[String],
    backend: &str,
    fluids: &[String],
    fractions: &[f64],
) -> Result<Vec<f64>> {
    let _g = lock();
    let _ = errstring();
    let outs = cstr(&outputs.join(","));
    let backend_buf = cstr(backend);
    let fl = cstr(&fluids.join(","));
    let fr = fractions.to_vec();
    let cap = outputs.len().max(1) * fluids.len().max(1);
    let mut result = vec![0f64; cap];
    let mut d1 = cap as c_long;
    unsafe {
        sys::Props1SImulti(
            outs.as_ptr(),
            backend_buf.as_ptr() as *mut c_char,
            fl.as_ptr(),
            fr.as_ptr(),
            fr.len() as c_long,
            result.as_mut_ptr(),
            &mut d1,
        );
    }
    if d1 == 0 {
        return Err(CoolPropError(fresh_errstring_or_default()));
    }
    result.truncate(d1 as usize);
    Ok(result)
}

/// `PhaseSI(...)` — name of the phase at a given state.
///
/// Note: CoolProp's C++ core reports failures here by writing
/// `"unknown: <error message>"` into the buffer *while returning 1*;
/// that spelling is therefore treated as an error (a real unknown
/// phase is the bare string `"unknown"`).
pub fn phase_si(name1: &str, prop1: f64, name2: &str, prop2: f64, fluid: &str) -> Result<String> {
    let _g = lock();
    let _ = errstring();
    let (n1, n2, f) = (cstr(name1), cstr(name2), cstr(fluid));
    let mut buf = vec![0u8; ERRBUF];
    let rc = unsafe {
        sys::PhaseSI(
            n1.as_ptr(),
            prop1,
            n2.as_ptr(),
            prop2,
            f.as_ptr(),
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len() as c_int,
        )
    };
    let phase = buffer_to_string(&buf);
    if rc == 1 && !phase.starts_with("unknown:") {
        Ok(phase)
    } else {
        Err(CoolPropError(if phase.is_empty() {
            fresh_errstring_or_default()
        } else {
            phase.trim_start_matches("unknown:").trim().to_string()
        }))
    }
}

// ----------------------------------------------------------------------
// Parameter / fluid information
// ----------------------------------------------------------------------

pub fn get_global_param_string(param: &str) -> Result<String> {
    let _g = lock();
    global_param_string(param).ok_or_else(|| CoolPropError(fresh_errstring_or_default()))
}

/// Names of all fluids of CoolProp's Helmholtz-EOS library,
/// via `get_global_param_string("fluids_list")`.
///
/// CoolProp joins the names with `LIST_STRING_DELIMITER` (`,` by default);
/// as for [`abstract_state_fluid_names`], a non-default delimiter
/// configured via `set_config_string` is not honoured when splitting.
pub fn fluids_list() -> Result<Vec<String>> {
    let raw = get_global_param_string("fluids_list")?;
    Ok(raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

/// `get_parameter_information_string(param, Output, n)` — long name, units or
/// IO role of a parameter.
///
/// Quirk (faithful to the C wrapper): the output buffer also serves as the
/// *input selector* for the information type — it must be
/// pre-filled with `info` (`"long"`, `"short"`, `"units"` or `"IO"`,
/// exact case — `src/DataStructures.cpp` compares literally) before
/// the call.
pub fn get_parameter_information_string(param: &str, info: &str) -> Result<String> {
    let _g = lock();
    let _ = errstring();
    let p = cstr(param);
    let mut buf = vec![0u8; ERRBUF];
    let info_bytes = info.as_bytes();
    if info_bytes.len() + 1 >= buf.len() {
        return Err(CoolPropError("info selector too long".into()));
    }
    buf[..info_bytes.len()].copy_from_slice(info_bytes);
    let rc = unsafe {
        sys::get_parameter_information_string(
            p.as_ptr(),
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len() as c_int,
        )
    };
    if rc == 1 {
        Ok(buffer_to_string(&buf))
    } else {
        Err(CoolPropError(fresh_errstring_or_default()))
    }
}

pub fn get_fluid_param_string(fluid: &str, param: &str) -> Result<String> {
    let _g = lock();
    let _ = errstring();
    let (f, p) = (cstr(fluid), cstr(param));
    let mut buf = vec![0u8; ERRBUF];
    let rc = unsafe {
        sys::get_fluid_param_string(
            f.as_ptr(),
            p.as_ptr(),
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len() as c_int,
        )
    };
    if rc == 1 {
        Ok(buffer_to_string(&buf))
    } else {
        Err(CoolPropError(fresh_errstring_or_default()))
    }
}

pub fn get_fluid_param_string_len(fluid: &str, param: &str) -> Result<i64> {
    let _g = lock();
    let (f, p) = (cstr(fluid), cstr(param));
    let len = unsafe { sys::get_fluid_param_string_len(f.as_ptr(), p.as_ptr()) };
    if len >= 0 {
        Ok(len)
    } else {
        Err(CoolPropError(format!(
            "invalid fluid/param pair {fluid}/{param}"
        )))
    }
}

// ----------------------------------------------------------------------
// Configuration
// ----------------------------------------------------------------------

/// Configuration keys accepted by CoolProp v8.0.0 (source:
/// `include/CoolProp/detail/configuration_keys.h`). Validated client-side because
/// `config_string_to_key` raises `ValueError()` with an *empty* message for an
/// unknown key, indistinguishable from a success via the errstring.
pub const VALID_CONFIG_KEYS: &[&str] = &[
    "ALLOW_SVDSBTL_IN_PROPSSI",
    "ALTERNATIVE_REFPROP_HMX_BNC_PATH",
    "ALTERNATIVE_REFPROP_LIBRARY_PATH",
    "ALTERNATIVE_REFPROP_PATH",
    "ALTERNATIVE_SVDTABLES_DIRECTORY",
    "ALTERNATIVE_TABLES_DIRECTORY",
    "ASSUME_CRITICAL_POINT_STABLE",
    "CRITICAL_SPLINES_ENABLED",
    "CRITICAL_WITHIN_1UK",
    "DONT_CHECK_PROPERTY_LIMITS",
    "ENABLE_MELTING_CALORIC_HS",
    "ENABLE_SUPERANCILLARIES",
    "FLOAT_PUNCTUATION",
    "HENRYS_LAW_TO_GENERATE_VLE_GUESSES",
    "HSU_D_TWOPHASE_EOS_POLISH",
    "LIST_STRING_DELIMITER",
    "MAXIMUM_TABLE_DIRECTORY_SIZE_IN_GB",
    "MIXTURE_STABILITY_ALGORITHM",
    "NORMALIZE_GAS_CONSTANTS",
    "OVERWRITE_BINARY_INTERACTION",
    "OVERWRITE_DEPARTURE_FUNCTION",
    "OVERWRITE_FLUIDS",
    "PHASE_ENVELOPE_STARTING_PRESSURE_PA",
    "REFPROP_DONT_ESTIMATE_INTERACTION_PARAMETERS",
    "REFPROP_ERROR_THRESHOLD",
    "REFPROP_IGNORE_ERROR_ESTIMATED_INTERACTION_PARAMETERS",
    "REFPROP_RESOLVE_COOLPROP_ALIASES",
    "REFPROP_USE_GERG",
    "REFPROP_USE_PENGROBINSON",
    "R_U_CODATA",
    "SAVE_RAW_TABLES",
    "SPINODAL_MINIMUM_DELTA",
    "SVDSBTL_SAMPLING_THREADS",
    "TABULAR_NX",
    "TABULAR_NY",
    "USE_GUESSES_IN_PROPSSI",
    "VTPR_ALWAYS_RELOAD_LIBRARY",
    "VTPR_UNIFAC_PATH",
];

fn validate_config_key(key: &str) -> Result<()> {
    if VALID_CONFIG_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(CoolPropError(format!(
            "unknown configuration key [{key}] (see the CoolProp documentation for valid keys)"
        )))
    }
}

pub fn set_config_string(key: &str, val: &str) -> Result<()> {
    let _g = lock();
    validate_config_key(key)?;
    let (k, v) = (cstr(key), cstr(val));
    void_call_report_errstring(|| unsafe { sys::set_config_string(k.as_ptr(), v.as_ptr()) })
}

pub fn set_config_double(key: &str, val: f64) -> Result<()> {
    let _g = lock();
    validate_config_key(key)?;
    let k = cstr(key);
    void_call_report_errstring(|| unsafe { sys::set_config_double(k.as_ptr(), val) })
}

pub fn set_config_bool(key: &str, val: bool) -> Result<()> {
    let _g = lock();
    validate_config_key(key)?;
    let k = cstr(key);
    void_call_report_errstring(|| unsafe { sys::set_config_bool(k.as_ptr(), val) })
}

pub fn set_departure_functions(string_data: &str) -> Result<()> {
    let _g = lock();
    let s = cstr(string_data);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::set_departure_functions(
            s.as_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    check_errcode(errcode, &msg)
}

/// Returns 1 on success, 0 on failure (the errstring carries the reason).
pub fn set_reference_state_s(refr: &str, reference_state: &str) -> Result<i64> {
    let _g = lock();
    let _ = errstring();
    let (r, s) = (cstr(refr), cstr(reference_state));
    let rc = unsafe { sys::set_reference_stateS(r.as_ptr(), s.as_ptr()) };
    if rc == 1 {
        Ok(rc as i64)
    } else {
        Err(CoolPropError(fresh_errstring_or_default()))
    }
}

pub fn set_reference_state_d(
    refr: &str,
    t: f64,
    rhomolar: f64,
    hmolar0: f64,
    smolar0: f64,
) -> Result<i64> {
    let _g = lock();
    let _ = errstring();
    let r = cstr(refr);
    let rc = unsafe { sys::set_reference_stateD(r.as_ptr(), t, rhomolar, hmolar0, smolar0) };
    if rc == 1 {
        Ok(rc as i64)
    } else {
        Err(CoolPropError(fresh_errstring_or_default()))
    }
}

// ----------------------------------------------------------------------
// Miscellaneous utilities
// ----------------------------------------------------------------------

pub fn f2k(t_f: f64) -> f64 {
    let _g = lock();
    unsafe { sys::F2K(t_f) }
}

pub fn k2f(t_k: f64) -> f64 {
    let _g = lock();
    unsafe { sys::K2F(t_k) }
}

pub fn get_param_index(param: &str) -> Result<i64> {
    let _g = lock();
    let p = cstr(param);
    let idx = unsafe { sys::get_param_index(p.as_ptr()) };
    if idx >= 0 {
        Ok(idx)
    } else {
        Err(CoolPropError(format!(
            "invalid CoolProp parameter name: {param:?}"
        )))
    }
}

pub fn get_input_pair_index(pair: &str) -> Result<i64> {
    let _g = lock();
    let p = cstr(pair);
    let idx = unsafe { sys::get_input_pair_index(p.as_ptr()) };
    if idx >= 0 {
        Ok(idx)
    } else {
        Err(CoolPropError(format!(
            "invalid CoolProp input pair name: {pair:?}"
        )))
    }
}

pub fn redirect_stdout(file: &str) -> Result<()> {
    let _g = lock();
    let f = cstr(file);
    let rc = unsafe { sys::redirect_stdout(f.as_ptr()) };
    if rc == 1 {
        Ok(())
    } else {
        Err(CoolPropError(format!(
            "could not redirect stdout to {file:?}"
        )))
    }
}

pub fn get_debug_level() -> i32 {
    let _g = lock();
    unsafe { sys::get_debug_level() }
}

pub fn set_debug_level(level: i32) {
    let _g = lock();
    unsafe { sys::set_debug_level(level) };
}

pub fn saturation_ancillary(
    fluid_name: &str,
    output: &str,
    q: i32,
    input: &str,
    value: f64,
) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (f, o, i) = (cstr(fluid_name), cstr(output), cstr(input));
    let v =
        unsafe { sys::saturation_ancillary(f.as_ptr(), o.as_ptr(), q as c_int, i.as_ptr(), value) };
    check_scalar(v)
}

// ----------------------------------------------------------------------
// Humid air properties
// ----------------------------------------------------------------------

pub fn haprops_si(
    output: &str,
    name1: &str,
    prop1: f64,
    name2: &str,
    prop2: f64,
    name3: &str,
    prop3: f64,
) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (o, n1, n2, n3) = (cstr(output), cstr(name1), cstr(name2), cstr(name3));
    let v = unsafe {
        sys::HAPropsSI(
            o.as_ptr(),
            n1.as_ptr(),
            prop1,
            n2.as_ptr(),
            prop2,
            n3.as_ptr(),
            prop3,
        )
    };
    check_scalar(v)
}

pub fn cair_sat(t: f64) -> Result<f64> {
    let _g = lock();
    let v = unsafe { sys::cair_sat(t) };
    check_scalar(v)
}

pub fn propssi_fortran(
    output: &str,
    name1: &str,
    prop1: f64,
    name2: &str,
    prop2: f64,
    fluid_name: &str,
) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (o, n1, n2, f) = (cstr(output), cstr(name1), cstr(name2), cstr(fluid_name));
    let (p1, p2, mut out) = (prop1, prop2, 0f64);
    unsafe {
        sys::propssi_(
            o.as_ptr(),
            n1.as_ptr(),
            &p1 as *const c_double,
            n2.as_ptr(),
            &p2 as *const c_double,
            f.as_ptr(),
            &mut out,
        );
    }
    check_scalar(out)
}

// ----------------------------------------------------------------------
// FORTRAN 77-style wrappers
// ----------------------------------------------------------------------

// ----------------------------------------------------------------------
// AbstractState (low-level stateful access)
// ----------------------------------------------------------------------

fn err_call(errcode: c_long, msg: &[u8]) -> Result<()> {
    check_errcode(errcode, msg)
}

/// `AbstractState_factory` — returns a new handle.
pub fn abstract_state_factory(backend: &str, fluids: &str) -> Result<i64> {
    let _g = lock();
    let (b, f) = (cstr(backend), cstr(fluids));
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let handle = unsafe {
        sys::AbstractState_factory(
            b.as_ptr(),
            f.as_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(handle as i64)
}

pub fn abstract_state_free(handle: i64) -> Result<()> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_free(
            handle as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn abstract_state_fluid_names(handle: i64) -> Result<Vec<String>> {
    let _g = lock();
    let mut buf = vec![0u8; ERRBUF];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_fluid_names(
            handle as c_long,
            buf.as_mut_ptr().cast::<c_char>(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    Ok(buffer_to_string(&buf)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn abstract_state_set_fractions(handle: i64, fractions: &[f64]) -> Result<()> {
    let _g = lock();
    let mut fr = fractions.to_vec();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_set_fractions(
            handle as c_long,
            fr.as_mut_ptr(),
            fr.len() as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

/// `AbstractState_get_mole_fractions` — the number of components is discovered
/// via `fluid_names` (the C API requires a pre-allocated buffer).
pub fn abstract_state_get_mole_fractions(handle: i64) -> Result<Vec<f64>> {
    let _g = lock();
    let names = abstract_state_fluid_names_locked(handle)?;
    let mut fr = vec![0f64; names.len().max(1)];
    let mut n = 0 as c_long;
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_get_mole_fractions(
            handle as c_long,
            fr.as_mut_ptr(),
            fr.len() as c_long,
            &mut n,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    fr.truncate(n as usize);
    Ok(fr)
}

/// Like [`abstract_state_fluid_names`] but for a locked call.
fn abstract_state_fluid_names_locked(handle: i64) -> Result<Vec<String>> {
    let mut buf = vec![0u8; ERRBUF];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_fluid_names(
            handle as c_long,
            buf.as_mut_ptr().cast::<c_char>(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    Ok(buffer_to_string(&buf)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn abstract_state_get_mole_fractions_sat_state(
    handle: i64,
    saturated_state: &str,
) -> Result<Vec<f64>> {
    let _g = lock();
    let names = abstract_state_fluid_names_locked(handle)?;
    let s = cstr(saturated_state);
    let mut fr = vec![0f64; names.len().max(1)];
    let mut n = 0 as c_long;
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_get_mole_fractions_satState(
            handle as c_long,
            s.as_ptr(),
            fr.as_mut_ptr(),
            fr.len() as c_long,
            &mut n,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    fr.truncate(n as usize);
    Ok(fr)
}

pub fn abstract_state_get_fugacity(handle: i64, i: i64) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_get_fugacity(
            handle as c_long,
            i as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_get_fugacity_coefficient(handle: i64, i: i64) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_get_fugacity_coefficient(
            handle as c_long,
            i as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_update(handle: i64, input_pair: i64, value1: f64, value2: f64) -> Result<()> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_update(
            handle as c_long,
            input_pair as c_long,
            value1,
            value2,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn abstract_state_specify_phase(handle: i64, phase: &str) -> Result<()> {
    let _g = lock();
    let p = cstr(phase);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_specify_phase(
            handle as c_long,
            p.as_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn abstract_state_unspecify_phase(handle: i64) -> Result<()> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_unspecify_phase(
            handle as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn abstract_state_keyed_output(handle: i64, param: i64) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_keyed_output(
            handle as c_long,
            param as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_first_saturation_deriv(handle: i64, of: i64, wrt: i64) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_first_saturation_deriv(
            handle as c_long,
            of as c_long,
            wrt as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_first_partial_deriv(
    handle: i64,
    of: i64,
    wrt: i64,
    constant: i64,
) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_first_partial_deriv(
            handle as c_long,
            of as c_long,
            wrt as c_long,
            constant as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_second_two_phase_deriv(
    handle: i64,
    of1: i64,
    wrt1: i64,
    constant1: i64,
    wrt2: i64,
    constant2: i64,
) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_second_two_phase_deriv(
            handle as c_long,
            of1 as c_long,
            wrt1 as c_long,
            constant1 as c_long,
            wrt2 as c_long,
            constant2 as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_second_partial_deriv(
    handle: i64,
    of1: i64,
    wrt1: i64,
    constant1: i64,
    wrt2: i64,
    constant2: i64,
) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_second_partial_deriv(
            handle as c_long,
            of1 as c_long,
            wrt1 as c_long,
            constant1 as c_long,
            wrt2 as c_long,
            constant2 as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_first_two_phase_deriv_splined(
    handle: i64,
    of: i64,
    wrt: i64,
    constant: i64,
    x_end: f64,
) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_first_two_phase_deriv_splined(
            handle as c_long,
            of as c_long,
            wrt as c_long,
            constant as c_long,
            x_end,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_first_two_phase_deriv(
    handle: i64,
    of: i64,
    wrt: i64,
    constant: i64,
) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_first_two_phase_deriv(
            handle as c_long,
            of as c_long,
            wrt as c_long,
            constant as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

/// Result of `AbstractState_update_and_common_out`.
pub struct CommonOut {
    pub t: Vec<f64>,
    pub p: Vec<f64>,
    pub rhomolar: Vec<f64>,
    pub hmolar: Vec<f64>,
    pub smolar: Vec<f64>,
}

pub fn abstract_state_update_and_common_out(
    handle: i64,
    input_pair: i64,
    value1: &[f64],
    value2: &[f64],
) -> Result<CommonOut> {
    let _g = lock();
    if value1.len() != value2.len() {
        return Err(CoolPropError(format!(
            "value1 has {} entries but value2 has {}",
            value1.len(),
            value2.len()
        )));
    }
    let n = value1.len();
    let mut t = vec![0f64; n];
    let mut p = vec![0f64; n];
    let mut rho = vec![0f64; n];
    let mut h = vec![0f64; n];
    let mut s = vec![0f64; n];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_update_and_common_out(
            handle as c_long,
            input_pair as c_long,
            value1.as_ptr(),
            value2.as_ptr(),
            n as c_long,
            t.as_mut_ptr(),
            p.as_mut_ptr(),
            rho.as_mut_ptr(),
            h.as_mut_ptr(),
            s.as_mut_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    Ok(CommonOut {
        t,
        p,
        rhomolar: rho,
        hmolar: h,
        smolar: s,
    })
}

pub fn abstract_state_update_and_1_out(
    handle: i64,
    input_pair: i64,
    value1: &[f64],
    value2: &[f64],
    output: i64,
) -> Result<Vec<f64>> {
    let _g = lock();
    if value1.len() != value2.len() {
        return Err(CoolPropError(format!(
            "value1 has {} entries but value2 has {}",
            value1.len(),
            value2.len()
        )));
    }
    let n = value1.len();
    let mut out = vec![0f64; n];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_update_and_1_out(
            handle as c_long,
            input_pair as c_long,
            value1.as_ptr(),
            value2.as_ptr(),
            n as c_long,
            output as c_long,
            out.as_mut_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    Ok(out)
}

pub fn abstract_state_update_and_5_out(
    handle: i64,
    input_pair: i64,
    value1: &[f64],
    value2: &[f64],
    outputs: [i64; 5],
) -> Result<[Vec<f64>; 5]> {
    let _g = lock();
    if value1.len() != value2.len() {
        return Err(CoolPropError(format!(
            "value1 has {} entries but value2 has {}",
            value1.len(),
            value2.len()
        )));
    }
    let n = value1.len();
    let mut outs = outputs.map(|_| vec![0f64; n]);
    let mut indices: [c_long; 5] = outputs.map(|o| o as c_long);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_update_and_5_out(
            handle as c_long,
            input_pair as c_long,
            value1.as_ptr(),
            value2.as_ptr(),
            n as c_long,
            indices.as_mut_ptr(),
            outs[0].as_mut_ptr(),
            outs[1].as_mut_ptr(),
            outs[2].as_mut_ptr(),
            outs[3].as_mut_ptr(),
            outs[4].as_mut_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    Ok(outs)
}

pub fn abstract_state_set_binary_interaction(
    handle: i64,
    i: i64,
    j: i64,
    parameter: &str,
    value: f64,
) -> Result<()> {
    let _g = lock();
    let p = cstr(parameter);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_set_binary_interaction_double(
            handle as c_long,
            i as c_long,
            j as c_long,
            p.as_ptr(),
            value,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn abstract_state_set_cubic_alpha_c(
    handle: i64,
    i: i64,
    parameter: &str,
    c1: f64,
    c2: f64,
    c3: f64,
) -> Result<()> {
    let _g = lock();
    let p = cstr(parameter);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_set_cubic_alpha_C(
            handle as c_long,
            i as c_long,
            p.as_ptr(),
            c1,
            c2,
            c3,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn abstract_state_set_fluid_parameter_double(
    handle: i64,
    i: i64,
    parameter: &str,
    value: f64,
) -> Result<()> {
    let _g = lock();
    let p = cstr(parameter);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_set_fluid_parameter_double(
            handle as c_long,
            i as c_long,
            p.as_ptr(),
            value,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn abstract_state_build_phase_envelope(handle: i64, level: &str) -> Result<()> {
    let _g = lock();
    let l = cstr(level);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_build_phase_envelope(
            handle as c_long,
            l.as_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

/// Phase envelope data of the checked-memory variant.
pub struct PhaseEnvelope {
    pub t: Vec<f64>,
    pub p: Vec<f64>,
    pub rhomolar_vap: Vec<f64>,
    pub rhomolar_liq: Vec<f64>,
    /// `x[point][component]` — liquid composition.
    pub x: Vec<Vec<f64>>,
    /// `y[point][component]` — vapor composition.
    pub y: Vec<Vec<f64>>,
    pub actual_length: usize,
    pub actual_components: usize,
}

/// `AbstractState_get_phase_envelope_data_checkedMemory` — reports the real
/// number of points and components. Requires the envelope to have been built.
pub fn abstract_state_get_phase_envelope(
    handle: i64,
    max_length: usize,
    max_components: usize,
) -> Result<PhaseEnvelope> {
    let _g = lock();
    let mut t = vec![0f64; max_length];
    let mut p = vec![0f64; max_length];
    let mut rv = vec![0f64; max_length];
    let mut rl = vec![0f64; max_length];
    let mut x = vec![0f64; max_length * max_components];
    let mut y = vec![0f64; max_length * max_components];
    let (mut alen, mut acomp) = (0 as c_long, 0 as c_long);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_get_phase_envelope_data_checkedMemory(
            handle as c_long,
            max_length as c_long,
            max_components as c_long,
            t.as_mut_ptr(),
            p.as_mut_ptr(),
            rv.as_mut_ptr(),
            rl.as_mut_ptr(),
            x.as_mut_ptr(),
            y.as_mut_ptr(),
            &mut alen,
            &mut acomp,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    let (alen, acomp) = (alen as usize, acomp as usize);
    let split = |flat: Vec<f64>| -> Vec<Vec<f64>> {
        (0..alen)
            .map(|i| flat[i * acomp..(i + 1) * acomp].to_vec())
            .collect()
    };
    t.truncate(alen);
    p.truncate(alen);
    rv.truncate(alen);
    rl.truncate(alen);
    Ok(PhaseEnvelope {
        t,
        p,
        rhomolar_vap: rv,
        rhomolar_liq: rl,
        x: split(x),
        y: split(y),
        actual_length: alen,
        actual_components: acomp,
    })
}

/// `AbstractState_get_phase_envelope_data` — the raw variant: returns
/// exactly `length` points without reporting the real size of the
/// envelope (the extra entries are zero if the envelope is shorter
/// than `length`).
pub fn abstract_state_get_phase_envelope_raw(
    handle: i64,
    length: usize,
    components: usize,
) -> Result<PhaseEnvelope> {
    let _g = lock();
    let mut t = vec![0f64; length];
    let mut p = vec![0f64; length];
    let mut rv = vec![0f64; length];
    let mut rl = vec![0f64; length];
    let mut x = vec![0f64; length * components];
    let mut y = vec![0f64; length * components];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_get_phase_envelope_data(
            handle as c_long,
            length as c_long,
            t.as_mut_ptr(),
            p.as_mut_ptr(),
            rv.as_mut_ptr(),
            rl.as_mut_ptr(),
            x.as_mut_ptr(),
            y.as_mut_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    let split = |flat: Vec<f64>| -> Vec<Vec<f64>> {
        (0..length)
            .map(|i| flat[i * components..(i + 1) * components].to_vec())
            .collect()
    };
    Ok(PhaseEnvelope {
        t,
        p,
        rhomolar_vap: rv,
        rhomolar_liq: rl,
        x: split(x),
        y: split(y),
        actual_length: length,
        actual_components: components,
    })
}

pub fn abstract_state_build_spinodal(handle: i64) -> Result<()> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_build_spinodal(
            handle as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

/// Spinodal curve. The C API does not report the real number of points;
/// unfilled (zero) entries are trimmed heuristically.
pub struct Spinodal {
    pub tau: Vec<f64>,
    pub delta: Vec<f64>,
    pub m1: Vec<f64>,
}

pub fn abstract_state_get_spinodal(handle: i64, max_length: usize) -> Result<Spinodal> {
    let _g = lock();
    let mut tau = vec![0f64; max_length];
    let mut delta = vec![0f64; max_length];
    let mut m1 = vec![0f64; max_length];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_get_spinodal_data(
            handle as c_long,
            max_length as c_long,
            tau.as_mut_ptr(),
            delta.as_mut_ptr(),
            m1.as_mut_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    // Unfilled entries are zero; a real inverse reduced temperature
    // is always positive, hence trimming at the first tau == 0.
    let n = tau.iter().position(|&v| v == 0.0).unwrap_or(tau.len());
    tau.truncate(n);
    delta.truncate(n);
    m1.truncate(n);
    Ok(Spinodal { tau, delta, m1 })
}

/// A critical point of the mixture.
pub struct CriticalPoint {
    pub t: f64,
    pub p: f64,
    pub rhomolar: f64,
    pub stable: bool,
}

pub fn abstract_state_all_critical_points(
    handle: i64,
    max_points: usize,
) -> Result<Vec<CriticalPoint>> {
    let _g = lock();
    let mut t = vec![0f64; max_points];
    let mut p = vec![0f64; max_points];
    let mut rho = vec![0f64; max_points];
    let mut stable = vec![0 as c_long; max_points];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_all_critical_points(
            handle as c_long,
            max_points as c_long,
            t.as_mut_ptr(),
            p.as_mut_ptr(),
            rho.as_mut_ptr(),
            stable.as_mut_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    // The C API does not report how many points were filled; the
    // unfilled entries are zero and a real critical point always has
    // T > 0 and p > 0.
    Ok(t.iter()
        .zip(&p)
        .zip(&rho)
        .zip(&stable)
        .filter(|(((&t, &p), _), _)| t > 0.0 && p > 0.0)
        .map(|(((&t, &p), &rho), &s)| CriticalPoint {
            t,
            p,
            rhomolar: rho,
            stable: s != 0,
        })
        .collect())
}

pub fn abstract_state_keyed_output_sat_state(
    handle: i64,
    saturated_state: &str,
    param: i64,
) -> Result<f64> {
    let _g = lock();
    let s = cstr(saturated_state);
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_keyed_output_satState(
            handle as c_long,
            s.as_ptr(),
            param as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_backend_name(handle: i64) -> Result<String> {
    let _g = lock();
    let mut buf = vec![0u8; ERRBUF];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_backend_name(
            handle as c_long,
            buf.as_mut_ptr().cast::<c_char>(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    Ok(buffer_to_string(&buf))
}

pub fn abstract_state_fluid_param_string(handle: i64, param: &str) -> Result<String> {
    let _g = lock();
    let p = cstr(param);
    let mut ret = vec![0u8; ERRBUF];
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::AbstractState_fluid_param_string(
            handle as c_long,
            p.as_ptr(),
            ret.as_mut_ptr().cast::<c_char>(),
            ret.len() as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)?;
    Ok(buffer_to_string(&ret))
}

pub fn abstract_state_phase(handle: i64) -> Result<i32> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let phase = unsafe {
        sys::AbstractState_phase(
            handle as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(phase)
}

pub fn abstract_state_saturated_liquid_keyed_output(handle: i64, param: i64) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_saturated_liquid_keyed_output(
            handle as c_long,
            param as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

pub fn abstract_state_saturated_vapor_keyed_output(handle: i64, param: i64) -> Result<f64> {
    let _g = lock();
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    let v = unsafe {
        sys::AbstractState_saturated_vapor_keyed_output(
            handle as c_long,
            param as c_long,
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        )
    };
    err_call(errcode, &msg)?;
    Ok(v)
}

// ----------------------------------------------------------------------
// Fluid library management
// ----------------------------------------------------------------------

pub fn add_fluids_as_json(backend: &str, fluid_string: &str) -> Result<()> {
    let _g = lock();
    let (b, f) = (cstr(backend), cstr(fluid_string));
    let mut errcode = 0 as c_long;
    let mut msg = vec![0u8; ERRBUF];
    unsafe {
        sys::add_fluids_as_JSON(
            b.as_ptr(),
            f.as_ptr(),
            &mut errcode,
            msg.as_mut_ptr().cast::<c_char>(),
            msg.len() as c_long,
        );
    }
    err_call(errcode, &msg)
}

pub fn is_valid_fluid_string(name: &str) -> bool {
    let _g = lock();
    let n = cstr(name);
    unsafe { sys::C_is_valid_fluid_string(n.as_ptr()) != 0 }
}

/// `C_extract_backend` — splits e.g. `"REFPROP::Water[0.5]&Ethane[0.5]"`.
pub fn extract_backend(fluid_string: &str) -> Result<(String, String)> {
    let _g = lock();
    let fs = cstr(fluid_string);
    let mut backend = vec![0u8; ERRBUF];
    let mut fluid = vec![0u8; ERRBUF];
    let rc = unsafe {
        sys::C_extract_backend(
            fs.as_ptr(),
            backend.as_mut_ptr().cast::<c_char>(),
            backend.len() as c_long,
            fluid.as_mut_ptr().cast::<c_char>(),
            fluid.len() as c_long,
        )
    };
    if rc == 0 {
        Ok((buffer_to_string(&backend), buffer_to_string(&fluid)))
    } else {
        Err(CoolPropError(
            "could not extract backend/fluid (output buffers too small)".into(),
        ))
    }
}

// ----------------------------------------------------------------------
// "Mixture" helpers (spec validation, molar masses)
// ----------------------------------------------------------------------

/// Checks that a fluid/mixture spec (`"Water"`,
/// `"Propane[0.5]&Ethane[0.5]"`, `"HEOS::R410A"`, `"INCOMP::MEG[0.4]"`)
/// can be instantiated by CoolProp: a reference state (300 K, 1 bar) is
/// computed with the same syntax as PropsSI — finite ⇔ valid spec,
/// otherwise the CoolProp message is returned. Call when saving an
/// org mixture (400 with the message on rejection).
pub fn validate_fluid_spec(spec: &str) -> Result<()> {
    props_si("Dmolar", "T", 300.0, "P", 101_325.0, spec)
        .map(|_| ())
        .map_err(|e| {
            if e.0.trim().is_empty() {
                CoolPropError(format!("fluid spec rejected by CoolProp: {spec:?}"))
            } else {
                e
            }
        })
}

/// Molar masses (kg/mol) of pure fluids, in the requested order.
/// First CoolProp failure = global error.
pub fn molar_masses(fluids: &[&str]) -> Result<Vec<f64>> {
    fluids
        .iter()
        .map(|f| props_1si(f.trim(), "molar_mass"))
        .collect()
}

// ----------------------------------------------------------------------
// Deprecated accessors (KSI units)
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f2k_k2f_round_trip() {
        assert!((f2k(32.0) - 273.15).abs() < 1e-9);
        assert!((k2f(f2k(100.0)) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn scalar_error_carries_coolprop_message() {
        let err = props_si("Bogus", "T", 300.0, "P", 101325.0, "Water").unwrap_err();
        assert!(!err.0.is_empty(), "error message should not be empty");
    }

    #[test]
    fn config_key_validation_rejects_unknown_keys() {
        assert!(set_config_bool("NOT_A_REAL_KEY", true).is_err());
    }

    #[test]
    fn valid_config_keys_table_has_no_duplicates() {
        let mut keys = VALID_CONFIG_KEYS.to_vec();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(n, keys.len());
    }

    #[test]
    fn validate_fluid_spec_accepts_and_rejects() {
        assert!(validate_fluid_spec("Water").is_ok());
        assert!(validate_fluid_spec("Propane[0.5]&Ethane[0.5]").is_ok());
        let err = validate_fluid_spec("NOT_A_FLUID").unwrap_err();
        assert!(!err.0.is_empty());
    }

    #[test]
    fn molar_masses_are_consistent() {
        let masses = molar_masses(&["Water", "Ethanol"]).expect("masses");
        assert!((masses[0] - 0.01801528).abs() < 1e-6, "{masses:?}");
        assert!((masses[1] - 0.04607).abs() < 1e-3, "{masses:?}");
    }
}
