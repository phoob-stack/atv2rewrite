//! The player's tuning from the launcher's Config tab (`tuning.cfg`, see
//! [`atv_sim::tuning`]), read again for every race so changes made between
//! races take effect without restarting.

use atv_formats::GameSource;
use atv_sim::tuning::{Tuning, FILE};
use atv_sim::VehicleParams;

pub fn load() -> Tuning {
    directories::ProjectDirs::from("", "", "atv2").map(|d| Tuning::load(&d.config_dir().join(FILE))).unwrap_or_default()
}

/// A quad's handling from the game's `.params`, with the player's tuning
/// for players (and for computer riders when the tuning says so), and its
/// collision shape.
pub fn quad_params(game: &GameSource, tuning: &Tuning, quad: &str, human: bool) -> VehicleParams {
    let read = |p: &str| game.read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
    let generic = read("/Cars/Generic/Generic.params").unwrap_or_default();
    let own = read(&format!("/Cars/{quad}/{quad}.params"));
    let mut params = if human || tuning.computer_riders {
        tuning.params(quad, &generic, own.as_deref())
    } else {
        VehicleParams::from_files(&generic, own.as_deref())
    };
    if let Some(body) = atv_sim::body::Body::from_game(game) {
        params.body = body;
    }
    params
}
