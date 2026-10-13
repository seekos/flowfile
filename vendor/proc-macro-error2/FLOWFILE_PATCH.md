# FlowFile patch

This is `proc-macro-error2` 2.0.1 with the compiler-suggested visibility fix
for Rust warning E0365:

- change `extern crate proc_macro` to `pub extern crate proc_macro`

The crate's hidden `__export` module publicly re-exports `proc_macro` for its
macros. Rust 1.98 warns that publicly re-exporting the original private import
will become a hard error in a future compiler. Making that import public keeps
the existing macro API and behavior while satisfying the stricter visibility
rule.
