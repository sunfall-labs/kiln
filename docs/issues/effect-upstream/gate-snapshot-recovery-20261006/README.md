# Reviewed emitted-C snapshot recovery

Source c14e33cf includes the private-brand bytecode change, canonical computed keys, String-wrapper property fix, and corrected direct-call frame publication. All 37 emitted-C tests passed after deliberate snapshot refresh, then passed again with INSTA_UPDATE=no. Raw verification logged the minimal prelude at 85,732 lines. The 86,000-line guard accommodates the reviewed private kernel changes; its floor and the ABI/body snapshots remain enforced.

Review normalized only module/program hashes, the bytecode version header, and obsolete snapshot assertion_line metadata. All body snapshots then remain identical except function_call_with_closure, whose typed direct call now publishes and restores its JavaScript frame and attributes the exact failure sentinel. The module signature and unsupported-version diagnostic advance consistently from bytecode 16 to 17.

ABI differences are existing private operations: proxy brand storage/membership, weak collection compaction, receiver/prototype property dispatch, namespace cell reads, String wrapper own slots, superclass adoption, and direct-call error attribution. No public builtin implementation was introduced. Full gate acceptance remains separate and required.
