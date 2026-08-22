pub mod color;
pub mod extract;
pub mod input;
pub mod screencast;

pub use extract::{DIRTY_BINDING, Extraction, Page};
pub use input::{Input, KeyInput, MouseAction, MouseInput};
pub use screencast::{SCREENCAST_FRAME, ScreencastFrame};
