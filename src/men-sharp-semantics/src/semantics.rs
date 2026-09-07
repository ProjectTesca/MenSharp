//! The phases: what the source says, and whether it says it correctly.
//!
//! Each module here is one step of the front end, in the order the driver
//! runs them:
//!
//! - [`collect`] reads one syntax tree into the declarations it contains —
//!   a pure function of a file, so the driver may fan it out.
//! - [`merge`] is the sequential barrier that makes one [`Declarations`]
//!   out of the per-file views: C# namespaces are open across files and
//!   `partial` types span them, so no single file knows the whole shape of
//!   anything.
//! - [`resolve`] turns every written type into a [`Type`], giving each
//!   member its signature.
//! - [`check`] walks the bodies: a type for every expression, an overload
//!   for every call, and the errors on the way.
//!
//! These read the type system in [`crate::types`]; the type system does
//! not read them back, except that [`crate::types::lookup::TypeSystem`] is
//! a view assembled *over* `Declarations` and `Signatures` — the one place
//! the arrow points this way.
//!
//! [`Declarations`]: merge::Declarations
//! [`Type`]: crate::types::Type

pub mod check;
pub mod collect;
pub mod merge;
pub mod resolve;
