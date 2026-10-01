//! Liaisons FFI brutes à chaque fonction exportée par l'API C de CoolProp
//! (`include/CoolProp/CoolPropLib.h`, CoolProp v8.0.0, submodule
//! `vendor/CoolProp`).
//!
//! Déclarations écrites à la main (pas de bindgen) — l'API est petite et
//! stable, et le build reste exempt de dépendance libclang.
//!
//! Origine : adapté du projet coolprop-rs (MIT, même auteur).
//!
//! # Error conventions (see CoolPropLib.cpp)
//!
//! * Functions with `errcode`/`message_buffer` out-params: `*errcode == 0`
//!   means success; `1` = CoolProp error (message in buffer), `2` = message
//!   did not fit in the buffer, `3` = unknown exception.
//! * Scalar `double`-returning functions signal errors with `_HUGE`
//!   (`HUGE_VAL`, i.e. infinity); the message is retrievable afterwards via
//!   `get_global_param_string("errstring", ...)`.
//! * `get_global_param_string` and friends return `1` on success, `0` on
//!   error (error in the global errstring).

#![allow(non_snake_case)]

use std::ffi::{c_char, c_double, c_int, c_long};

extern "C" {
    // ------------------------------------------------------------------
    // High-level property accessors
    // ------------------------------------------------------------------
    pub fn Props1SI(FluidName: *const c_char, Output: *const c_char) -> c_double;
    pub fn Props1SImulti(
        Outputs: *const c_char,
        backend: *mut c_char,
        FluidNames: *const c_char,
        fractions: *const c_double,
        length_fractions: c_long,
        result: *mut c_double,
        resdim1: *mut c_long,
    );
    pub fn PropsSI(
        Output: *const c_char,
        Name1: *const c_char,
        Prop1: c_double,
        Name2: *const c_char,
        Prop2: c_double,
        FluidName: *const c_char,
    ) -> c_double;
    pub fn PropsSImulti(
        Outputs: *const c_char,
        Name1: *const c_char,
        Prop1: *mut c_double,
        size_Prop1: c_long,
        Name2: *const c_char,
        Prop2: *mut c_double,
        size_Prop2: c_long,
        backend: *mut c_char,
        FluidNames: *const c_char,
        fractions: *const c_double,
        length_fractions: c_long,
        result: *mut c_double,
        resdim1: *mut c_long,
        resdim2: *mut c_long,
    );
    pub fn PhaseSI(
        Name1: *const c_char,
        Prop1: c_double,
        Name2: *const c_char,
        Prop2: c_double,
        FluidName: *const c_char,
        phase: *mut c_char,
        n: c_int,
    ) -> c_long;

    // ------------------------------------------------------------------
    // Parameter / fluid information
    // ------------------------------------------------------------------
    pub fn get_global_param_string(param: *const c_char, Output: *mut c_char, n: c_int) -> c_long;
    pub fn get_parameter_information_string(
        param: *const c_char,
        Output: *mut c_char,
        n: c_int,
    ) -> c_long;
    pub fn get_fluid_param_string(
        fluid: *const c_char,
        param: *const c_char,
        Output: *mut c_char,
        n: c_int,
    ) -> c_long;
    pub fn get_fluid_param_string_len(fluid: *const c_char, param: *const c_char) -> c_long;

    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------
    pub fn set_config_string(key: *const c_char, val: *const c_char);
    pub fn set_config_double(key: *const c_char, val: c_double);
    pub fn set_config_bool(key: *const c_char, val: bool);
    pub fn set_departure_functions(
        string_data: *const c_char,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn set_reference_stateS(Ref: *const c_char, reference_state: *const c_char) -> c_int;
    pub fn set_reference_stateD(
        Ref: *const c_char,
        T: c_double,
        rhomolar: c_double,
        hmolar0: c_double,
        smolar0: c_double,
    ) -> c_int;

    // ------------------------------------------------------------------
    // FORTRAN 77 style wrappers
    // ------------------------------------------------------------------
    pub fn propssi_(
        Output: *const c_char,
        Name1: *const c_char,
        Prop1: *const c_double,
        Name2: *const c_char,
        Prop2: *const c_double,
        FluidName: *const c_char,
        output: *mut c_double,
    );
    pub fn hapropssi_(
        Output: *const c_char,
        Name1: *const c_char,
        Prop1: *const c_double,
        Name2: *const c_char,
        Prop2: *const c_double,
        Name3: *const c_char,
        Prop3: *const c_double,
        output: *mut c_double,
    );
    pub fn haprops_(
        Output: *const c_char,
        Name1: *const c_char,
        Prop1: *const c_double,
        Name2: *const c_char,
        Prop2: *const c_double,
        Name3: *const c_char,
        Prop3: *const c_double,
        output: *mut c_double,
    );

    // ------------------------------------------------------------------
    // Miscellaneous utilities
    // ------------------------------------------------------------------
    pub fn F2K(T_F: c_double) -> c_double;
    pub fn K2F(T_K: c_double) -> c_double;
    pub fn get_param_index(param: *const c_char) -> c_long;
    pub fn get_input_pair_index(pair: *const c_char) -> c_long;
    pub fn redirect_stdout(file: *const c_char) -> c_long;
    pub fn get_debug_level() -> c_int;
    pub fn set_debug_level(level: c_int);
    pub fn saturation_ancillary(
        fluid_name: *const c_char,
        output: *const c_char,
        Q: c_int,
        input: *const c_char,
        value: c_double,
    ) -> c_double;

    // ------------------------------------------------------------------
    // Humid air properties
    // ------------------------------------------------------------------
    pub fn HAPropsSI(
        Output: *const c_char,
        Name1: *const c_char,
        Prop1: c_double,
        Name2: *const c_char,
        Prop2: c_double,
        Name3: *const c_char,
        Prop3: c_double,
    ) -> c_double;
    pub fn cair_sat(T: c_double) -> c_double;
    pub fn HAProps(
        Output: *const c_char,
        Name1: *const c_char,
        Prop1: c_double,
        Name2: *const c_char,
        Prop2: c_double,
        Name3: *const c_char,
        Prop3: c_double,
    ) -> c_double;

    // ------------------------------------------------------------------
    // Low-level stateful access (AbstractState)
    // ------------------------------------------------------------------
    pub fn AbstractState_factory(
        backend: *const c_char,
        fluids: *const c_char,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_long;
    pub fn AbstractState_fluid_names(
        handle: c_long,
        fluids: *mut c_char,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_free(
        handle: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_set_fractions(
        handle: c_long,
        fractions: *const c_double,
        N: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_get_mole_fractions(
        handle: c_long,
        fractions: *mut c_double,
        maxN: c_long,
        N: *mut c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_get_mole_fractions_satState(
        handle: c_long,
        saturated_state: *const c_char,
        fractions: *mut c_double,
        maxN: c_long,
        N: *mut c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_get_fugacity(
        handle: c_long,
        i: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_get_fugacity_coefficient(
        handle: c_long,
        i: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_update(
        handle: c_long,
        input_pair: c_long,
        value1: c_double,
        value2: c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_specify_phase(
        handle: c_long,
        phase: *const c_char,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_unspecify_phase(
        handle: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_keyed_output(
        handle: c_long,
        param: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_first_saturation_deriv(
        handle: c_long,
        Of: c_long,
        Wrt: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_first_partial_deriv(
        handle: c_long,
        Of: c_long,
        Wrt: c_long,
        Constant: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_second_two_phase_deriv(
        handle: c_long,
        Of1: c_long,
        Wrt1: c_long,
        Constant1: c_long,
        Wrt2: c_long,
        Constant2: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_second_partial_deriv(
        handle: c_long,
        Of1: c_long,
        Wrt1: c_long,
        Constant1: c_long,
        Wrt2: c_long,
        Constant2: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_first_two_phase_deriv_splined(
        handle: c_long,
        Of: c_long,
        Wrt: c_long,
        Constant: c_long,
        x_end: c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_first_two_phase_deriv(
        handle: c_long,
        Of: c_long,
        Wrt: c_long,
        Constant: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_update_and_common_out(
        handle: c_long,
        input_pair: c_long,
        value1: *const c_double,
        value2: *const c_double,
        length: c_long,
        T: *mut c_double,
        p: *mut c_double,
        rhomolar: *mut c_double,
        hmolar: *mut c_double,
        smolar: *mut c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_update_and_1_out(
        handle: c_long,
        input_pair: c_long,
        value1: *const c_double,
        value2: *const c_double,
        length: c_long,
        output: c_long,
        out: *mut c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_update_and_5_out(
        handle: c_long,
        input_pair: c_long,
        value1: *const c_double,
        value2: *const c_double,
        length: c_long,
        outputs: *mut c_long,
        out1: *mut c_double,
        out2: *mut c_double,
        out3: *mut c_double,
        out4: *mut c_double,
        out5: *mut c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_set_binary_interaction_double(
        handle: c_long,
        i: c_long,
        j: c_long,
        parameter: *const c_char,
        value: c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_set_cubic_alpha_C(
        handle: c_long,
        i: c_long,
        parameter: *const c_char,
        c1: c_double,
        c2: c_double,
        c3: c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_set_fluid_parameter_double(
        handle: c_long,
        i: c_long,
        parameter: *const c_char,
        value: c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_build_phase_envelope(
        handle: c_long,
        level: *const c_char,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_get_phase_envelope_data(
        handle: c_long,
        length: c_long,
        T: *mut c_double,
        p: *mut c_double,
        rhomolar_vap: *mut c_double,
        rhomolar_liq: *mut c_double,
        x: *mut c_double,
        y: *mut c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_get_phase_envelope_data_checkedMemory(
        handle: c_long,
        length: c_long,
        maxComponents: c_long,
        T: *mut c_double,
        p: *mut c_double,
        rhomolar_vap: *mut c_double,
        rhomolar_liq: *mut c_double,
        x: *mut c_double,
        y: *mut c_double,
        actual_length: *mut c_long,
        actual_components: *mut c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_build_spinodal(
        handle: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_get_spinodal_data(
        handle: c_long,
        length: c_long,
        tau: *mut c_double,
        delta: *mut c_double,
        M1: *mut c_double,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_all_critical_points(
        handle: c_long,
        length: c_long,
        T: *mut c_double,
        p: *mut c_double,
        rhomolar: *mut c_double,
        stable: *mut c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_keyed_output_satState(
        handle: c_long,
        saturated_state: *const c_char,
        param: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_backend_name(
        handle: c_long,
        backend: *mut c_char,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_fluid_param_string(
        handle: c_long,
        param: *const c_char,
        return_buffer: *mut c_char,
        return_buffer_length: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn AbstractState_phase(
        handle: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_int;
    pub fn AbstractState_saturated_liquid_keyed_output(
        handle: c_long,
        param: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;
    pub fn AbstractState_saturated_vapor_keyed_output(
        handle: c_long,
        param: c_long,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    ) -> c_double;

    // ------------------------------------------------------------------
    // Fluid library management
    // ------------------------------------------------------------------
    pub fn add_fluids_as_JSON(
        backend: *const c_char,
        fluidstring: *const c_char,
        errcode: *mut c_long,
        message_buffer: *mut c_char,
        buffer_length: c_long,
    );
    pub fn C_is_valid_fluid_string(fluidName: *const c_char) -> c_int;
    pub fn C_extract_backend(
        fluid_string: *const c_char,
        backend: *mut c_char,
        backend_length: c_long,
        fluid: *mut c_char,
        fluid_length: c_long,
    ) -> c_int;

    // ------------------------------------------------------------------
    // Deprecated (KSI-unit) property accessors
    // ------------------------------------------------------------------
    pub fn PropsS(
        Output: *const c_char,
        Name1: *const c_char,
        Prop1: c_double,
        Name2: *const c_char,
        Prop2: c_double,
        Ref: *const c_char,
    ) -> c_double;
    pub fn Props(
        Output: *const c_char,
        Name1: c_char,
        Prop1: c_double,
        Name2: c_char,
        Prop2: c_double,
        Ref: *const c_char,
    ) -> c_double;
    pub fn Props1(FluidName: *const c_char, Output: *const c_char) -> c_double;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_calls_coolprop() {
        // Smoke test: proves the static library is linked and callable.
        unsafe {
            assert!((F2K(32.0) - 273.15).abs() < 1e-9);
            assert!((K2F(273.15) - 32.0).abs() < 1e-9);
        }
    }
}
