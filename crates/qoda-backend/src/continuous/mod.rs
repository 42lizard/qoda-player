//! Compatible converted-source groups; trusted backend inputs only.
mod decode;
mod description;
pub mod input;
mod ring;
mod session;
pub mod stream;
pub use description::{Description, parse as describe};
pub use session::Session;
