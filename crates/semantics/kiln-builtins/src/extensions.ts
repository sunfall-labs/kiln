// kiln-builtins: whether this realm installs kiln's own members (ADR 0068).
//
// Shared by both interpreter and native execution, and first in the common bootstrap so every
// later part can read it. The engines own the decision -- `KILN_EXTENSIONS=0` at run time, or a
// binary built with `kiln build --no-kiln-extensions` -- and publish it as `__kilnExtensions`,
// which is captured here and deleted before any program runs. Absent means installed: kiln's own
// members are on by default, and a host that does not say otherwise gets them.
//
// Every member installed under this flag must be declared in
// `kiln_builtin_surface::KILN_EXTENSIONS`, and a test over this crate holds the two together.
const kilnExtensions: boolean = (globalThis as any).__kilnExtensions !== false;
delete (globalThis as any).__kilnExtensions;
