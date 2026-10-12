//! Engine-agnostic driving simulation for the ATV rewrite.
//!
//! Works in a right-handed, Y-up, metres coordinate system (the Bevy
//! convention); data from the game is converted on load by negating Z.

pub mod ai;
pub mod collision;
pub mod params;
pub mod surface;
pub mod track;
pub mod tuning;
pub mod vehicle;

pub use collision::TrackCollision;
pub use params::VehicleParams;
pub use vehicle::{Input, Vehicle};
