//! The timeline's picture: what it is made of at a moment, and (from the
//! compositor on) putting it together on the graphics card.
//!
//! See this crate's README.md for what lives here and what is still to come.

pub mod compositor;
pub mod frame;

pub use compositor::{Compositor, Drawn, Frame, Pixels, Shower, Showing, Working};
pub use frame::{Area, FrameDescription, Layer, Part, describe};
