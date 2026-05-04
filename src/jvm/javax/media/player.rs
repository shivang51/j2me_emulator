// javax/media/Player

#[derive(Debug, Clone)]
pub enum PlayerState {
    Closed,
    Prefetched,
    Realized,
    Started,
    TimeUnknown,
    Unrealized,
}

#[derive(Debug, Clone)]
pub struct Player {
    state: PlayerState,
}

impl Player {
    pub fn new() -> Self {
        Player {
            state: PlayerState::Unrealized,
        }
    }

    pub fn get_state(&self) -> &PlayerState {
        &self.state
    }

    pub fn start(&mut self) {
        self.state = PlayerState::Started;
    }

    pub fn stop(&mut self) {
        self.state = PlayerState::Prefetched;
    }

    pub fn close(&mut self) {
        self.state = PlayerState::Closed;
    }
}
