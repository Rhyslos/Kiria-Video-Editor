//! Putting the timeline's picture together on the graphics card: layers
//! drawn into a frame, and a frame shown.
//!
//! See this crate's README.md for what lives here and what is still to come.

pub mod compositor;
pub mod effects;
pub mod geometry;
pub mod reader;

pub use compositor::{Background, Compositor, Drawn, Frame, Mode, Pixels, Shower, Showing, WHOLE, Working};
pub use effects::{Effect, Effects, Pictures, Spot};
pub use geometry::{Area, Colour, Part, Placement, Point};
pub use reader::{ReadError, Reader};
