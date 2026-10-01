//! Wrapper safe autour de l'API C brute de CoolProp.
//!
//! Conventions implémentées ici (voir `vendor/CoolProp/src/CoolPropLib.cpp`) :
//!
//! * Chaque appel C est sérialisé derrière un mutex global : plusieurs points
//!   d'entrée de CoolProp mutent la configuration process-global ou la
//!   chaîne d'erreur globale, et le registre des handles AbstractState est
//!   partagé.
//! * L'errstring globale est *drainée* (la lire la vide) avant chaque appel
//!   qui rapporte ses erreurs par ce biais, pour qu'un indicateur d'échec
//!   après l'appel corresponde toujours à un message frais.
//! * Les appels de style `errcode` mappent `0` → succès, tout le reste → le
//!   message bufferisé.
//! * Les scalaires `double` signalent l'échec en retournant `_HUGE`
//!   (= `HUGE_VAL`).

use std::ffi::{c_char, c_double, c_int, c_long, CStr, CString};
use std::sync::{Mutex, MutexGuard};

use pnex_coolprop_sys as sys;

/// Taille du buffer utilisé pour chaque out-param `message_buffer` / chaîne.
pub const ERRBUF: usize = 10_000;

/// Sérialise tous les appels FFI CoolProp (voir la doc du module).
static FFI: Mutex<()> = Mutex::new(());

/// Erreur portant le message de CoolProp.
#[derive(Debug, Clone)]
pub struct CoolPropError(pub String);

pub type Result<T> = std::result::Result<T, CoolPropError>;

fn lock() -> MutexGuard<'static, ()> {
    FFI.lock().unwrap_or_else(|e| e.into_inner())
}

fn cstr(s: &str) -> CString {
    // Les octets NUL intérieurs ne sont pas représentables dans une chaîne C ;
    // on les retire plutôt que de paniquer sur une entrée utilisateur.
    CString::new(s.replace('\0', "")).expect("string with NUL bytes removed cannot fail")
}

fn buffer_to_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Lit une chaîne de paramètre global ; `None` si CoolProp signale un échec.
/// À appeler en tenant le verrou FFI.
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

/// Drain-and-return de l'errstring globale (la lire la vide dans CoolProp).
fn errstring() -> String {
    global_param_string("errstring").unwrap_or_default()
}

/// Décode le motif de résultat `errcode` / `message_buffer`.
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

/// Draine l'errstring, exécute l'appel C void, relit l'errstring : les
/// setters void (`set_config_*`, ...) ne rapportent leurs échecs *que* par
/// ce biais.
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

/// Vérifie un scalaire `double` contre le sentinel d'échec `_HUGE`.
fn check_scalar(v: f64) -> Result<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(CoolPropError(fresh_errstring_or_default()))
    }
}

// ----------------------------------------------------------------------
// Accesseurs de propriétés haut niveau
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

/// `Props1SI(FluidName, Output)` — sortie à état unique sans entrée
/// (ex. `"Tcrit"`, `"Pcrit"`, `"molar_mass"`).
pub fn props_1si(fluid: &str, output: &str) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (f, o) = (cstr(fluid), cstr(output));
    let v = unsafe { sys::Props1SI(f.as_ptr(), o.as_ptr()) };
    check_scalar(v)
}

/// `PropsSImulti(...)` — PropsSI vectorisé. Retourne une ligne par point
/// d'entrée, un élément par sortie (miroir du `[points][outputs]` CoolProp).
#[allow(clippy::too_many_arguments)] // miroir de la signature C
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
    // Dimensions de capacité in/out : le wrapper C vérifie
    // `_result.size() > *resdim1 || _result[0].size() > *resdim2` où les
    // lignes sont les points d'entrée et les colonnes les sorties
    // (IO CoolProp : [points][outputs]), puis y réécrit les tailles réelles.
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

/// `Props1SImulti(...)` — Props1SI multi-fluides. Miroir du wrapper C, qui
/// n'expose que la première ligne de résultats (voir CoolPropLib.cpp).
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

/// `PhaseSI(...)` — nom de la phase à un état donné.
///
/// Note : le noyau C++ de CoolProp rapporte ici les échecs en écrivant
/// `"unknown: <message d'erreur>"` dans le buffer *tout en retournant 1* ;
/// cette orthographe est donc traitée comme une erreur (une vraie phase
/// inconnue est la chaîne nue `"unknown"`).
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
// Informations paramètres / fluides
// ----------------------------------------------------------------------

pub fn get_global_param_string(param: &str) -> Result<String> {
    let _g = lock();
    global_param_string(param).ok_or_else(|| CoolPropError(fresh_errstring_or_default()))
}

/// Noms de tous les fluides de la bibliothèque Helmholtz-EOS de CoolProp,
/// via `get_global_param_string("fluids_list")`.
///
/// CoolProp joint les noms avec `LIST_STRING_DELIMITER` (`,` par défaut) ;
/// comme pour [`abstract_state_fluid_names`], un délimiteur non par défaut
/// configuré via `set_config_string` n'est pas honoré au découpage.
pub fn fluids_list() -> Result<Vec<String>> {
    let raw = get_global_param_string("fluids_list")?;
    Ok(raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

/// `get_parameter_information_string(param, Output, n)` — nom long, unités ou
/// rôle IO d'un paramètre.
///
/// Quirk (fidèle au wrapper C) : le buffer de sortie sert aussi de
/// *sélecteur d'entrée* pour le type d'information — il doit être
/// pré-rempli avec `info` (`"long"`, `"short"`, `"units"` ou `"IO"`,
/// casse exacte — `src/DataStructures.cpp` compare littéralement) avant
/// l'appel.
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

/// Clés de configuration acceptées par CoolProp v8.0.0 (source :
/// `include/CoolProp/detail/configuration_keys.h`). Validée côté client car
/// `config_string_to_key` lève `ValueError()` avec un message *vide* pour une
/// clé inconnue, indistinguable d'un succès via l'errstring.
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

/// Retourne 1 en succès, 0 en échec (l'errstring porte la raison).
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
// Utilitaires divers
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
// Propriétés de l'air humide
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

pub fn haprops_legacy(
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
        sys::HAProps(
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

// ----------------------------------------------------------------------
// Wrappers de style FORTRAN 77
// ----------------------------------------------------------------------

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

pub fn hapropssi_fortran(
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
    let (p1, p2, p3, mut out) = (prop1, prop2, prop3, 0f64);
    unsafe {
        sys::hapropssi_(
            o.as_ptr(),
            n1.as_ptr(),
            &p1 as *const c_double,
            n2.as_ptr(),
            &p2 as *const c_double,
            n3.as_ptr(),
            &p3 as *const c_double,
            &mut out,
        );
    }
    check_scalar(out)
}

pub fn haprops_fortran(
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
    let (p1, p2, p3, mut out) = (prop1, prop2, prop3, 0f64);
    unsafe {
        sys::haprops_(
            o.as_ptr(),
            n1.as_ptr(),
            &p1 as *const c_double,
            n2.as_ptr(),
            &p2 as *const c_double,
            n3.as_ptr(),
            &p3 as *const c_double,
            &mut out,
        );
    }
    check_scalar(out)
}

// ----------------------------------------------------------------------
// AbstractState (accès stateful bas niveau)
// ----------------------------------------------------------------------

fn err_call(errcode: c_long, msg: &[u8]) -> Result<()> {
    check_errcode(errcode, msg)
}

/// `AbstractState_factory` — retourne un nouveau handle.
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

/// `AbstractState_get_mole_fractions` — le nombre de composants est découvert
/// via `fluid_names` (l'API C exige un buffer pré-alloué).
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

/// Comme [`abstract_state_fluid_names`] mais pour un appel verrouillé.
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

/// Résultat de `AbstractState_update_and_common_out`.
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

/// Données d'enveloppe de phase de la variante à mémoire vérifiée.
pub struct PhaseEnvelope {
    pub t: Vec<f64>,
    pub p: Vec<f64>,
    pub rhomolar_vap: Vec<f64>,
    pub rhomolar_liq: Vec<f64>,
    /// `x[point][composant]` — composition liquide.
    pub x: Vec<Vec<f64>>,
    /// `y[point][composant]` — composition vapeur.
    pub y: Vec<Vec<f64>>,
    pub actual_length: usize,
    pub actual_components: usize,
}

/// `AbstractState_get_phase_envelope_data_checkedMemory` — rapporte le nombre
/// réel de points et de composants. Exige que l'enveloppe ait été construite.
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

/// `AbstractState_get_phase_envelope_data` — la variante brute : retourne
/// exactement `length` points sans rapport de la taille réelle de
/// l'enveloppe (les entrées en trop sont à zéro si l'enveloppe est plus
/// courte que `length`).
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

/// Courbe spinodale. L'API C ne rapporte pas le nombre réel de points ;
/// les entrées non remplies (à zéro) sont rognées heuristiquement.
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
    // Les entrées non remplies sont à zéro ; une vraie température réduite
    // inverse est toujours positive, donc rognage au premier tau == 0.
    let n = tau.iter().position(|&v| v == 0.0).unwrap_or(tau.len());
    tau.truncate(n);
    delta.truncate(n);
    m1.truncate(n);
    Ok(Spinodal { tau, delta, m1 })
}

/// Un point critique du mélange.
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
    // L'API C ne rapporte pas combien de points ont été remplis ; les
    // entrées non remplies sont à zéro et un vrai point critique a toujours
    // T > 0 et p > 0.
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
// Gestion de la bibliothèque de fluides
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

/// `C_extract_backend` — découpe par ex. `"REFPROP::Water[0.5]&Ethane[0.5]"`.
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
// Aides « mélanges » (validation de spec, masses molaires)
// ----------------------------------------------------------------------

/// Vérifie qu'une spec de fluide/mélange (`"Water"`,
/// `"Propane[0.5]&Ethane[0.5]"`, `"HEOS::R410A"`, `"INCOMP::MEG[0.4]"`)
/// est instanciable par CoolProp : un état de référence (300 K, 1 bar) est
/// calculé avec la même syntaxe que PropsSI — finite ⇔ spec valide,
/// sinon le message CoolProp est retourné. À appeler à la sauvegarde d'un
/// mélange org (400 avec le message si rejet).
pub fn validate_fluid_spec(spec: &str) -> Result<()> {
    props_si("Dmolar", "T", 300.0, "P", 101_325.0, spec)
        .map(|_| ())
        .map_err(|e| {
            if e.0.trim().is_empty() {
                CoolPropError(format!("spec de fluide rejetée par CoolProp : {spec:?}"))
            } else {
                e
            }
        })
}

/// Masses molaires (kg/mol) de fluides purs, dans l'ordre demandé.
/// Premier échec CoolProp = erreur globale.
pub fn molar_masses(fluids: &[&str]) -> Result<Vec<f64>> {
    fluids
        .iter()
        .map(|f| props_1si(f.trim(), "molar_mass"))
        .collect()
}

// ----------------------------------------------------------------------
// Accesseurs dépréciés (unités KSI)
// ----------------------------------------------------------------------

pub fn props_s(
    output: &str,
    name1: &str,
    prop1: f64,
    name2: &str,
    prop2: f64,
    refr: &str,
) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (o, n1, n2, r) = (cstr(output), cstr(name1), cstr(name2), cstr(refr));
    let v = unsafe {
        sys::PropsS(
            o.as_ptr(),
            n1.as_ptr(),
            prop1,
            n2.as_ptr(),
            prop2,
            r.as_ptr(),
        )
    };
    check_scalar(v)
}

/// `Props` — comme PropsS mais `Name1`/`Name2` sont des caractères uniques.
pub fn props_legacy(
    output: &str,
    name1: char,
    prop1: f64,
    name2: char,
    prop2: f64,
    refr: &str,
) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (o, r) = (cstr(output), cstr(refr));
    let (n1, n2) = (name1 as c_char, name2 as c_char);
    let v = unsafe { sys::Props(o.as_ptr(), n1, prop1, n2, prop2, r.as_ptr()) };
    check_scalar(v)
}

pub fn props1_legacy(fluid: &str, output: &str) -> Result<f64> {
    let _g = lock();
    let _ = errstring();
    let (f, o) = (cstr(fluid), cstr(output));
    let v = unsafe { sys::Props1(f.as_ptr(), o.as_ptr()) };
    check_scalar(v)
}

/// Convertit une chaîne C empruntée en `String` Rust (helper de test).
#[allow(dead_code)]
pub(crate) fn cstr_to_string(p: *const c_char) -> String {
    unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
}

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
    fn validate_fluid_spec_accepte_et_rejette() {
        assert!(validate_fluid_spec("Water").is_ok());
        assert!(validate_fluid_spec("Propane[0.5]&Ethane[0.5]").is_ok());
        let err = validate_fluid_spec("NOT_A_FLUID").unwrap_err();
        assert!(!err.0.is_empty());
    }

    #[test]
    fn molar_masses_sont_coherentes() {
        let masses = molar_masses(&["Water", "Ethanol"]).expect("masses");
        assert!((masses[0] - 0.01801528).abs() < 1e-6, "{masses:?}");
        assert!((masses[1] - 0.04607).abs() < 1e-3, "{masses:?}");
    }
}
