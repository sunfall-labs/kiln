//! The single source of truth for the private native number-provider contract.
//!
//! This crate deliberately contains no implementation dependencies.  The provider implementation
//! uses the registry to assert its Rust function signatures, while `kiln-native` uses the same
//! expansion to validate packaged manifest metadata.  Keeping the contract here avoids making the
//! compiler depend on the provider's Tokio/regexp/Temporal implementation crate.
#![no_std]

/// Expand the private provider ABI entries as `(feature, symbol, signature)` tuples.
///
/// The signature token is interpreted by the consumer because some ABI handles are intentionally
/// private Rust types (`BigInt`, `TemporalValue`, and the callback view structs).  This keeps those
/// implementation types out of this tiny metadata crate while still making the entry point and its
/// required signature a declarative, checked registry entry in the provider crate.
#[macro_export]
macro_rules! provider_abi_registry {
    ($callback:ident) => {
        $callback! {
            (realm, kiln_realm_create, realm_create);
            (realm, kiln_realm_enter, realm_enter);
            (realm, kiln_realm_is_active, realm_is_active);
            (realm, kiln_realm_diagnostic_reset, realm_diagnostic_reset);
            (realm, kiln_realm_diagnostic_note, realm_diagnostic_note);
            (realm, kiln_realm_diagnostic_render, realm_diagnostic_render);
            (realm, kiln_realm_symbol_for, realm_symbol_for);
            (realm, kiln_realm_symbol_key_for, realm_symbol_key_for);
            (realm, kiln_realm_callable_store, realm_callable_store);
            (realm, kiln_realm_value_slot, realm_value_slot);
            (realm, kiln_realm_set_argv, realm_set_argv);
            (realm, kiln_realm_argv_count, realm_argv_count);
            (realm, kiln_realm_argv_at, realm_argv_at);
            (realm, kiln_realm_entropy_word, realm_entropy_word);
            (realm, kiln_realm_gc_current_heap, realm_gc_current_heap);
            (realm, kiln_realm_gc_attach, realm_gc_attach);
            (realm, kiln_realm_gc_register_roots, realm_gc_register_roots);
            (realm, kiln_realm_gc_unregister_roots, realm_gc_unregister_roots);
            (realm, kiln_realm_gc_mark_roots, realm_gc_mark_roots);
            (realm, kiln_realm_gc_mark_value, realm_gc_mark_value);
            (realm, kiln_realm_gc_mark_block, realm_gc_mark_block);
            (realm, kiln_realm_now, realm_now);
            (realm, kiln_realm_now_nanoseconds, realm_now_nanoseconds);
            (realm, kiln_realm_queue_microtask, realm_queue_work);
            (realm, kiln_realm_queue_next_tick, realm_queue_work);
            (realm, kiln_realm_set_timeout, realm_set_timer);
            (realm, kiln_realm_set_interval, realm_set_timer);
            (realm, kiln_realm_clear_timer, realm_clear_timer);
            (realm, kiln_realm_drive, realm_drive);
            (realm, kiln_realm_destroy, realm_destroy);
            (realm, kiln_realm_read_file, realm_read_file);
            (realm, kiln_realm_start_host_op, realm_start_host_op);
            (realm, kiln_realm_start_active_host_op, realm_start_active_host_op);
            (realm, kiln_realm_cancel_request, realm_cancel_request);
            (realm, kiln_realm_cancel_active_request, realm_cancel_active_request);
            (realm, kiln_realm_http_server_attach_active, realm_http_attach);
            (realm, kiln_realm_http_server_attach_with_options_active, realm_http_attach_options);
            (realm, kiln_realm_http_server_close_active, realm_http_close_mode);
            (realm, kiln_realm_http_client_attach_active, realm_http_attach);
            (realm, kiln_realm_http_client_request_id_active, realm_http_request_id);
            (realm, kiln_realm_http_agent_acquire_active, realm_http_agent_acquire);
            (realm, kiln_realm_http_client_owner_active, realm_http_client_owner);
            (realm, kiln_realm_http_agent_connected_active, realm_http_agent_connected);
            (realm, kiln_realm_http_agent_release_active, realm_http_agent_release);
            (realm, kiln_realm_http_client_start_active, realm_http_client_start);
            (realm, kiln_realm_http_client_write_active, realm_http_client_write);
            (realm, kiln_realm_http_client_end_active, realm_http_client_end);
            (realm, kiln_realm_http_response_start_active, realm_http_response_start);
            (realm, kiln_realm_http_response_write_active, realm_http_response_write);
            (realm, kiln_realm_http_response_end_active, realm_http_response_end);
            (root, kiln_number_to_js_string, number_to_js_string);
            (root, kiln_string_to_number, string_to_number);
            (regexp, kiln_regexp_compile, regexp_compile);
            (regexp, kiln_regexp_free, regexp_free);
            (regexp, kiln_regexp_search, regexp_search);
            (regexp, kiln_regexp_group_count, regexp_group_count);
            (regexp, kiln_regexp_group_name, regexp_group_name);
            (bigint, kiln_bigint_parse, bigint_parse);
            (bigint, kiln_bigint_from_double, bigint_from_double);
            (bigint, kiln_bigint_free, bigint_free);
            (bigint, kiln_bigint_to_double, bigint_to_double);
            (bigint, kiln_bigint_to_string, bigint_to_string);
            (bigint, kiln_bigint_binary, bigint_binary);
            (bigint, kiln_bigint_negate, bigint_negate);
            (bigint, kiln_bigint_compare, bigint_compare);
            (bigint, kiln_bigint_is_zero, bigint_is_zero);
            (bigint, kiln_bigint_to_i128, bigint_to_i128);
            (temporal, kiln_temporal_from_fields, temporal_from_fields);
            (temporal, kiln_temporal_construct, temporal_construct);
            (temporal, kiln_temporal_duration_from_fields, temporal_duration_from_fields);
            (temporal, kiln_temporal_parse, temporal_parse);
            (temporal, kiln_temporal_now, temporal_now);
            (temporal, kiln_temporal_instant_from_epoch_milliseconds, temporal_instant_ms);
            (temporal, kiln_temporal_instant_from_epoch_nanoseconds, temporal_instant_ns);
            (temporal, kiln_temporal_free, temporal_free);
            (temporal, kiln_temporal_kind, temporal_kind);
            (temporal, kiln_temporal_get, temporal_get);
            (temporal, kiln_temporal_to_string, temporal_to_string);
            (temporal, kiln_temporal_compare, temporal_compare);
            (temporal, kiln_temporal_invoke, temporal_invoke);
            (temporal, kiln_temporal_to_kind, temporal_to_kind);
            (temporal, kiln_temporal_last_error, temporal_last_error);
        }
    };
}

macro_rules! provider_export_names {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        &[$(stringify!($name),)*]
    };
}

/// Version of the private provider ABI represented by [`provider_abi_registry!`].
pub const PROVIDER_ABI_VERSION: u32 = 26;

/// Canonical ordered export names required in a packaged provider manifest.
pub const PROVIDER_EXPORTS: &[&str] = provider_abi_registry!(provider_export_names);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_the_packaged_contract_shape() {
        assert_eq!(PROVIDER_ABI_VERSION, 26);
        assert_eq!(PROVIDER_EXPORTS.len(), 82);
        assert_eq!(PROVIDER_EXPORTS.first(), Some(&"kiln_realm_create"));
        assert_eq!(PROVIDER_EXPORTS.last(), Some(&"kiln_temporal_last_error"));
    }
}
