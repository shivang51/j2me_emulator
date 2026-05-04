// use to allocate javax objects

use crate::jvm::javax::media::player::Player;

#[derive(Debug)]
pub struct Factory {}

impl Factory {
    pub fn create_player() -> Player {
        Player::new()
    }
}
