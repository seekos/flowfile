# block 0.1.6 compatibility patch

This is an API-compatible local copy of `block` 0.1.6, originally published
by Steven Sheldon under the MIT license.

The only compatibility change is the representation of the opaque Objective-C
`Class` type. The upstream empty enum is uninhabited and triggers Rust's
`uninhabited_static` future-incompatibility lint when used for
`_NSConcreteStackBlock`. This copy uses an inhabited, zero-sized `repr(C)`
opaque type while preserving the Objective-C Blocks ABI and public Rust API.

Upstream: <https://github.com/SSheldon/rust-block>
