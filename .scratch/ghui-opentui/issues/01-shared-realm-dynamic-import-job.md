# Dynamic import must queue on the shared module realm

Status: resolved

## Failure

During Node module-graph execution, `Instruction::DynamicImport` queued `Job::Import` directly on
`self.realm`. The Node graph owns and drains `module_realm`, so the import job was stranded. A
dependency suspended at top-level await never resumed, while its importer continued and read a
captured class binding before initialisation.

## Resolution

The landing change queues the scheduled import through `with_realm_mut`. The regression
`node_dependency_dynamic_import_job_resumes_top_level_await_before_importer` covers a dependency
that catches a failed dynamic import, completes top-level await, initialises a captured class, and
then allows its importer to call it.
