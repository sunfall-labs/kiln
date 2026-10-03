//! The single source of truth for the private native number-provider contract.
//!
//! This crate deliberately contains no implementation dependencies.  The provider implementation
//! uses the registry to assert its Rust function signatures, while `kiln-native` uses the same
//! expansion to validate packaged manifest metadata.  Keeping the contract here avoids making the
//! compiler depend on the provider's Tokio/regexp/Temporal implementation crate.

/// The kind of work an ABI group authoritatively owns (ADR 0060).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnershipKind {
    RuntimeEngine,
    SharedSemanticEngine,
    ForeignInterface,
}

/// One machine-readable ownership row for a provider feature group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupOwnership {
    pub group: &'static str,
    pub owner: &'static str,
    pub kind: OwnershipKind,
}

/// The authoritative owner of every group in [`provider_abi_registry!`].
pub const PROVIDER_GROUP_OWNERSHIP: &[GroupOwnership] = &[
    GroupOwnership {
        group: "realm",
        owner: "kiln-runtime realm engine",
        kind: OwnershipKind::RuntimeEngine,
    },
    GroupOwnership {
        group: "root",
        owner: "kiln-number-core",
        kind: OwnershipKind::SharedSemanticEngine,
    },
    GroupOwnership {
        group: "regexp",
        owner: "regress adapter",
        kind: OwnershipKind::SharedSemanticEngine,
    },
    GroupOwnership {
        group: "bigint",
        owner: "kiln-native-provider-ffi BigInt engine",
        kind: OwnershipKind::SharedSemanticEngine,
    },
    GroupOwnership {
        group: "temporal",
        owner: "kiln-temporal",
        kind: OwnershipKind::SharedSemanticEngine,
    },
    GroupOwnership {
        group: "json",
        owner: "kiln-json",
        kind: OwnershipKind::SharedSemanticEngine,
    },
    GroupOwnership {
        group: "intl",
        owner: "kiln-intl",
        kind: OwnershipKind::SharedSemanticEngine,
    },
    GroupOwnership {
        group: "crypto",
        owner: "kiln-key-core",
        kind: OwnershipKind::SharedSemanticEngine,
    },
    GroupOwnership {
        group: "parallel",
        owner: "kiln-native-provider-ffi parallel engine",
        kind: OwnershipKind::RuntimeEngine,
    },
    GroupOwnership {
        group: "ffi",
        owner: "kiln-native-provider-ffi foreign interface",
        kind: OwnershipKind::ForeignInterface,
    },
    GroupOwnership {
        group: "date",
        owner: "kiln-date-core",
        kind: OwnershipKind::SharedSemanticEngine,
    },
];

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
            (realm, kiln_node_path_part, node_path_part);
            (realm, kiln_node_path_relative, node_path_relative);
            (realm, kiln_url_parse, url_parse);
            (realm, kiln_url_set, url_set);
            (realm, kiln_text_decoder_label, text_decoder_label);
            (realm, kiln_text_decode, text_decode);
            (realm, kiln_text_decoder_state_create, text_decoder_state_create);
            (realm, kiln_text_decoder_state_decode, text_decoder_state_decode);
            (realm, kiln_text_decoder_state_free, text_decoder_state_free);
            (realm, kiln_string_case, string_case);
            (realm, kiln_realm_symbol_for, realm_symbol_for);
            (realm, kiln_realm_symbol_key_for, realm_symbol_key_for);
            (realm, kiln_realm_callable_store, realm_callable_store);
            (realm, kiln_realm_value_slot, realm_value_slot);
            (realm, kiln_realm_set_argv, realm_set_argv);
            (realm, kiln_realm_argv_count, realm_argv_count);
            (realm, kiln_realm_argv_at, realm_argv_at);
            (realm, kiln_realm_entropy_word, realm_entropy_word);
            (realm, kiln_realm_runtime_report, realm_runtime_report);
            (realm, kiln_realm_gc_current_heap, realm_gc_current_heap);
            (realm, kiln_realm_generation_slot, realm_generation_slot);
            (realm, kiln_realm_process_shared, realm_process_shared);
            (realm, kiln_realm_retire_tag, realm_retire_tag);
            (realm, kiln_realm_retired_safe, realm_retired_safe);
            (realm, kiln_realm_retire, realm_retire);
            (realm, kiln_realm_run_pinned, realm_run_pinned);
            (realm, kiln_realm_run_workers, realm_run_pinned);
            (realm, kiln_realm_run_scheduled, realm_run_scheduled);
            (realm, kiln_realm_run_migrating, realm_run_migrating);
            (realm, kiln_realm_set_graph_control, realm_set_graph_control);
            (realm, kiln_realm_graph_capsule_stage, realm_graph_capsule_stage);
            (realm, kiln_realm_graph_capsule_peek, realm_graph_capsule_peek);
            (realm, kiln_realm_graph_capsule_detached, realm_graph_capsule_detached);
            (realm, kiln_realm_graph_capsule_cancel, realm_graph_capsule_cancel);
            (realm, kiln_realm_graph_capsule_take, realm_graph_capsule_take);
            (realm, kiln_realm_gc_attach, realm_gc_attach);
            (realm, kiln_realm_gc_register_roots, realm_gc_register_roots);
            (realm, kiln_realm_gc_register_roots_with_visitor, realm_gc_register_roots_with_visitor);
            (realm, kiln_realm_gc_unregister_roots, realm_gc_unregister_roots);
            (realm, kiln_realm_gc_mark_roots, realm_gc_mark_roots);
            (realm, kiln_realm_gc_has_root_visitor, realm_gc_has_root_visitor);
            (realm, kiln_realm_gc_visit_provider_roots, realm_gc_visit_provider_roots);
            (realm, kiln_realm_gc_mark_value, realm_gc_mark_value);
            (realm, kiln_realm_gc_mark_block, realm_gc_mark_block);
            (realm, kiln_realm_use_wall_clock, realm_use_wall_clock);
            (realm, kiln_realm_now, realm_now);
            (realm, kiln_realm_performance_now, realm_now);
            (realm, kiln_realm_now_nanoseconds, realm_now_nanoseconds);
            (realm, kiln_realm_queue_microtask, realm_queue_work);
            (realm, kiln_realm_queue_next_tick, realm_queue_work);
            (realm, kiln_realm_set_promise_lane, realm_set_promise_lane);
            (realm, kiln_realm_set_timeout, realm_set_timer);
            (realm, kiln_realm_set_interval, realm_set_timer);
            (realm, kiln_realm_set_immediate, realm_queue_macrotask);
            (realm, kiln_realm_queue_io, realm_queue_macrotask);
            (realm, kiln_realm_clear_timer, realm_clear_timer);
            (realm, kiln_realm_set_timer_ref, realm_set_timer_ref);
            (realm, kiln_realm_refresh_timer, realm_clear_timer);
            (realm, kiln_realm_rearm_timeout, realm_rearm_timer);
            (realm, kiln_realm_drive, realm_drive);
            (realm, kiln_realm_drive_step, realm_drive_step);
            (realm, kiln_realm_destroy, realm_destroy);
            (realm, kiln_realm_read_file, realm_read_file);
            (realm, kiln_realm_start_host_op, realm_start_host_op);
            (realm, kiln_realm_start_active_host_op, realm_start_active_host_op);
            (realm, kiln_realm_cancel_request, realm_cancel_request);
            (realm, kiln_realm_cancel_active_request, realm_cancel_active_request);
            (realm, kiln_realm_cancel_active_request_if_pending, realm_cancel_active_request_if_pending);
            (realm, kiln_realm_http_server_attach_active, realm_http_attach);
            (realm, kiln_realm_http_server_attach_with_options_active, realm_http_attach_options);
            (realm, kiln_realm_http_server_close_active, realm_http_close_mode);
            (realm, kiln_realm_http_client_attach_active, realm_http_attach);
            (realm, kiln_realm_http_client_request_id_active, realm_http_request_id);
            (realm, kiln_realm_http_agent_acquire_active, realm_http_agent_acquire);
            (realm, kiln_realm_http_client_owner_active, realm_http_client_owner);
            (realm, kiln_realm_http_agent_connected_active, realm_http_agent_connected);
            (realm, kiln_realm_http_agent_connect_failed_active, realm_http_agent_connect_failed);
            (realm, kiln_realm_http_agent_cancel_connect_active, realm_http_agent_cancel_connect);
            (realm, kiln_realm_http_agent_can_retain_active, realm_http_agent_can_retain);
            (realm, kiln_realm_http_agent_release_active, realm_http_agent_release);
            (realm, kiln_realm_http_client_start_active, realm_http_client_start);
            (realm, kiln_realm_http_client_write_active, realm_http_client_write);
            (realm, kiln_realm_http_client_end_active, realm_http_client_end);
            (realm, kiln_realm_http_response_start_active, realm_http_response_start);
            (realm, kiln_realm_http_response_write_active, realm_http_response_write);
            (realm, kiln_realm_http_response_end_active, realm_http_response_end);
            (realm, kiln_realm_http_upgrade_write_active, realm_http_response_write);
            (realm, kiln_realm_http_upgrade_write_active_with_fence, realm_http_upgrade_write_fence);
            (realm, kiln_node_fs_read_file_sync, node_fs_read_file_sync);
            (realm, kiln_node_fs_free_bytes, node_fs_free_bytes);
            (realm, kiln_node_fs_write_file_sync, node_fs_write_file_sync);
            (realm, kiln_node_fs_mkdir_sync, node_fs_mkdir_sync);
            (realm, kiln_node_fs_stat_sync, node_fs_stat_sync);
            (realm, kiln_node_fs_readdir_sync, node_fs_readdir_sync);
            (realm, kiln_node_fs_realpath_sync, node_fs_realpath_sync);
            (realm, kiln_node_fs_unlink_sync, node_fs_unlink_sync);
            (realm, kiln_node_fs_rmdir_sync, node_fs_rmdir_sync);
            (realm, kiln_worker_post, worker_post);
            (realm, kiln_worker_post_plain, worker_post_plain);
            (realm, kiln_worker_take, worker_take);
            (realm, kiln_worker_message_free, worker_message_free);
            (realm, kiln_worker_op, worker_op);
            (realm, kiln_worker_broadcast_join, worker_broadcast_join);
            (realm, kiln_worker_set_runner, worker_set_runner);
            (realm, kiln_worker_set_turn_runner, worker_set_turn_runner);
            (realm, kiln_worker_spawn, worker_spawn);
            (realm, kiln_worker_self, worker_self);
            (realm, kiln_worker_stdio, worker_stdio);
            (realm, kiln_worker_exit, worker_exit);
            (realm, kiln_worker_exit_code, worker_exit_code);
            (realm, kiln_shared_create, shared_create);
            (realm, kiln_shared_graph_retain, shared_graph_retain);
            (realm, kiln_shared_graph_release, shared_graph_release);
            (realm, kiln_shared_graph_view, shared_graph_view);
            (realm, kiln_shared_graph_info, shared_graph_info);
            (realm, kiln_shared_graph_find_field, shared_graph_find_field);
            (realm, kiln_shared_graph_field, shared_graph_field);
            (realm, kiln_shared_graph_build, shared_graph_build);
            (realm, kiln_shared_graph_refusal, shared_graph_build);
            (realm, kiln_shared_release, shared_release);
            (realm, kiln_shared_grow, shared_grow);
            (realm, kiln_atomics_wait, atomics_wait);
            (realm, kiln_atomics_wait_async_register, atomics_wait_async_register);
            (realm, kiln_atomics_wait_async_cancel, atomics_wait_async_cancel);
            (realm, kiln_atomics_notify, atomics_notify);
            (root, kiln_number_to_js_string, number_to_js_string);
            (root, kiln_string_to_number, string_to_number);
            (date, kiln_date_parse, date_parse);
            (root, kiln_typed_array_sort, typed_array_sort);
            (regexp, kiln_regexp_compile, regexp_compile);
            (regexp, kiln_regexp_free, regexp_free);
            (regexp, kiln_regexp_search, regexp_search);
            (regexp, kiln_regexp_group_count, regexp_group_count);
            (regexp, kiln_regexp_group_name, regexp_group_name);
            (regexp, kiln_regexp_named_group_count, regexp_named_group_count);
            (regexp, kiln_regexp_named_group_at, regexp_named_group_at);
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
            (json, kiln_json_parse, json_parse);
            (intl, kiln_intl_segment, intl_segment);
            (intl, kiln_string_normalize, string_normalize);
            (intl, kiln_intl_collate, intl_collate);
            (intl, kiln_intl_string_case, intl_string_case);
            (intl, kiln_intl_datetime_format, intl_datetime_format);
            (crypto, kiln_crypto_key_import, crypto_key_import);
            (crypto, kiln_crypto_key_public, crypto_key_public);
            (crypto, kiln_crypto_key_info, crypto_key_info);
            (crypto, kiln_crypto_key_export, crypto_key_export);
            (crypto, kiln_crypto_key_free, crypto_key_free);
            (parallel, kiln_parallel_map, parallel_map);
            (parallel, kiln_parallel_sort, parallel_sort);
            (parallel, kiln_parallel_threads, parallel_threads);
            (ffi, kiln_ffi_open, ffi_open);
            (ffi, kiln_ffi_close, ffi_close);
            (ffi, kiln_ffi_prepare, ffi_prepare);
            (ffi, kiln_ffi_call, ffi_call);
            (ffi, kiln_ffi_register_callback, ffi_register_callback);
            (ffi, kiln_ffi_unregister_callback, ffi_unregister_callback);
            (ffi, kiln_ffi_last_error, ffi_last_error);
        }
    };
}

macro_rules! provider_export_names {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        &[$(stringify!($name),)*]
    };
}

/// Version of the private provider ABI represented by [`provider_abi_registry!`].
pub const PROVIDER_ABI_VERSION: u32 = 77;

/// Canonical ordered export names required in a packaged provider manifest.
pub const PROVIDER_EXPORTS: &[&str] = provider_abi_registry!(provider_export_names);

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! provider_export_groups {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            &[$(stringify!($group),)*]
        };
    }

    const PROVIDER_EXPORT_GROUPS: &[&str] = provider_abi_registry!(provider_export_groups);

    #[test]
    fn registry_has_the_packaged_contract_shape() {
        // 76 and 185 exports: `kiln_realm_rearm_timeout` joined, so a `Timeout` that already ran
        // restarts under its own identity on `refresh()`, as node's does. Before that,
        // 75 and 184 exports: `kiln_realm_refresh_timer` joined, `Timeout.prototype.refresh()`:
        // the realm's timer queue restarts the timer it already owns from the realm's current time.
        // Before that, 74 and 183 exports: `kiln_realm_set_timer_ref` joined, the per-timer flag behind
        // `timer.unref()` and `AbortSignal.timeout`, which the realm's timer queue owns. Before
        // that, 73 and 182 exports: `kiln_shared_graph_refusal` joined, so a native `share` throws the
        // refusal `kiln-shared-graph` words, exactly the interpreter's text. Before that,
        // 72 and 181 exports: `kiln_text_decoder_label` and `kiln_text_decode` joined, so native
        // `TextDecoder` resolves labels and decodes through `kiln-encoding` (ADR 0060) as the
        // interpreter does. Before that, 52 and 144 exports: the native launcher can move an audited realm between
        // scheduler lanes after each generated frame has returned.
        // 51 and 143 exports: the scheduled launcher drives one realm turn at a time and
        // multiplexes pinned realm sessions over fixed scheduler lanes. Before that, 50 and
        // 141 exports: the plain-message builder moved to the shared Rust codec. Before that,
        // 49 and 140 exports: `kiln_parallel_threads` joined so a parallel call site can ask
        // the pool's size before it takes its pooled lowering -- a one-thread pool runs the
        // sequential method's own loop (parallel issue 22) -- and a tier-3 site can size its
        // worker-arena table (parallel issue 08). Before that, 48 and 139 exports:
        // `kiln_realm_run_workers` joined for `KILN_WORKERS` thread mode
        // (realms issue 07): `kiln_realm_run_pinned` whose realm threads share one listening
        // socket per address, with accepts balanced in process. Before that, 47 and 138 exports:
        // `node:worker_threads` on realms (realms issue 06) -- the sixteen
        // `kiln_worker_*`, `kiln_shared_*` and `kiln_atomics_*` operations that carry message
        // tokens between a native realm and the runtime's cross-realm ports, start a worker's realm
        // through the launcher, and share a `SharedArrayBuffer`'s store and its `Atomics.wait`
        // list between realms. Before that, 46 and 122 exports: `kiln_realm_retire_tag`, `kiln_realm_retired_safe` and
        // `kiln_realm_retire` joined when shared cache state became safe to write from realms on
        // several threads at once (ADR 0071, quiescent-state reclamation at turn boundaries), and
        // `kiln_realm_run_pinned` when a launcher could run several realms in one process, each
        // pinned to a thread of the runtime's `WorkerPool` (realms issue 05).
        // Before that, 45 and 118 exports: `kiln_typed_array_sort` joined for the sequential
        // `%TypedArray%.prototype.sort` default order (parallel issue 16): one sort per call, on
        // the calling thread, over the order `kiln_number_core::typed_order` owns for both
        // engines. Before that, 44 and 117 exports: `kiln_parallel_sort` joined for `parallelSort` (ADR 0069, parallel
        // issue 10): a stable sort of a typed array's elements, or of a numeric array's, in place
        // on the parallel pool, for a default order or a comparator the backend proved
        // consistent. Before that, 43 and 116 exports: `kiln_realm_set_immediate` and `kiln_realm_queue_io` joined when
        // the realm gained node's check and poll lanes beside its timers, so an immediate and a
        // message port's delivery each run in their own phase of a turn. Before that, 42 and 114:
        // `kiln_realm_process_shared` joined when shared keys and shapes (ADR 0071) moved into one
        // process-wide block every unit reaches through the archive. Before that, 35 keeps the same export set but adds the distinct boolean scalar kind to node:ffi;
        // an older provider would interpret that kind as unsupported. Before that, 34 and 97
        // exports added the seven node:ffi provider operations. Before that, 33 and 90 exports:
        // `kiln_realm_set_promise_lane` joined when the promise job queue
        // moved into the prelude (Phase 6.3). The realm no longer receives one work item per
        // reaction; it calls the lane back at the point in its drain where promise jobs belong,
        // and asks `pending` whether there are any. Before that, 32 and 89: `kiln_parallel_map`
        // joined for `parallelMap` (ADR 0058, application 2): one call that runs an emitted chunk
        // worker over index ranges on a private Rayon pool. Before that, 31 and 88: `JSON.parse`
        // moved into the archive as a tape (`kiln_json_parse`, one call that lends the reader's
        // per-thread tape), so the emitted C stopped carrying a reader of its own. Before that,
        // 30 and 87: `kiln_realm_generation_slot` joined the contract when the prelude learned to
        // cache per-realm answers -- the heap pointer first -- behind a generation the realm bumps
        // on every entry, exit and heap attach, so a thread that hosts several realms never serves
        // one realm the other's cache. A shipped archive built before any of these does not export
        // the newer symbol, and the version is how a resolver knows to rebuild rather than to fail
        // at link time.
        assert_eq!(PROVIDER_ABI_VERSION, 77);
        assert_eq!(PROVIDER_EXPORTS.len(), 188);
        assert_eq!(PROVIDER_EXPORTS.first(), Some(&"kiln_realm_create"));
        assert_eq!(PROVIDER_EXPORTS.last(), Some(&"kiln_ffi_last_error"));
    }

    #[test]
    fn every_export_group_has_one_owner() {
        for group in PROVIDER_EXPORT_GROUPS {
            assert_eq!(
                PROVIDER_GROUP_OWNERSHIP
                    .iter()
                    .filter(|row| row.group == *group)
                    .count(),
                1,
                "provider ABI group `{group}` must have exactly one semantic owner"
            );
        }
        for (index, row) in PROVIDER_GROUP_OWNERSHIP.iter().enumerate() {
            assert!(PROVIDER_EXPORT_GROUPS.contains(&row.group));
            assert!(
                !PROVIDER_GROUP_OWNERSHIP[..index]
                    .iter()
                    .any(|earlier| earlier.group == row.group)
            );
            assert!(!row.owner.is_empty());
        }
    }
}
