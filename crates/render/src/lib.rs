//! Putting the timeline's picture together on the graphics card: layers
//! drawn into a frame, and a frame shown.
//!
//! See this crate's README.md for what lives here and what is still to come.

pub mod compositor;
pub mod geometry;

pub use compositor::{Compositor, Drawn, Frame, Pixels, Shower, Showing, Working};
pub use geometry::{Area, Colour, Part, Placement, Point};
