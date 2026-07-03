use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use egui_winit::winit::event::KeyEvent;
use egui_winit::winit::keyboard::{Key, KeyCode};
use parking_lot::Mutex;

mod raw_midp_keycode {
    pub const ARROW_UP: i32 = -1;
    pub const ARROW_DOWN: i32 = -2;
    pub const ARROW_LEFT: i32 = -3;
    pub const ARROW_RIGHT: i32 = -4;
    pub const FIRE: i32 = -5;
    pub const SOFT_LEFT: i32 = -6;
    pub const SOFT_RIGHT: i32 = -7;
    pub const CLEAR: i32 = -8;

    pub const NUM_0: i32 = '0' as i32;
    pub const NUM_1: i32 = '1' as i32;
    pub const NUM_2: i32 = '2' as i32;
    pub const NUM_3: i32 = '3' as i32;
    pub const NUM_4: i32 = '4' as i32;
    pub const NUM_5: i32 = '5' as i32;
    pub const NUM_6: i32 = '6' as i32;
    pub const NUM_7: i32 = '7' as i32;
    pub const NUM_8: i32 = '8' as i32;
    pub const NUM_9: i32 = '9' as i32;
    pub const STAR: i32 = '*' as i32;
    pub const POUND: i32 = '#' as i32;
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MidpKeyCode(i32);

impl MidpKeyCode {
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> i32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MidpKey {
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Fire,
    SoftLeft,
    SoftRight,
    Clear,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    Star,
    Pound,
}

const MIDP_KEY_DISPLAY_ORDER: &[MidpKey] = &[
    MidpKey::ArrowUp,
    MidpKey::ArrowDown,
    MidpKey::ArrowLeft,
    MidpKey::ArrowRight,
    MidpKey::Fire,
    MidpKey::SoftLeft,
    MidpKey::SoftRight,
    MidpKey::Clear,
    MidpKey::Num0,
    MidpKey::Num1,
    MidpKey::Num2,
    MidpKey::Num3,
    MidpKey::Num4,
    MidpKey::Num5,
    MidpKey::Num6,
    MidpKey::Num7,
    MidpKey::Num8,
    MidpKey::Num9,
    MidpKey::Star,
    MidpKey::Pound,
];

impl MidpKey {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ArrowUp => "Up",
            Self::ArrowDown => "Down",
            Self::ArrowLeft => "Left",
            Self::ArrowRight => "Right",
            Self::Fire => "Fire",
            Self::SoftLeft => "Soft Left",
            Self::SoftRight => "Soft Right",
            Self::Clear => "Clear",
            Self::Num0 => "0",
            Self::Num1 => "1",
            Self::Num2 => "2",
            Self::Num3 => "3",
            Self::Num4 => "4",
            Self::Num5 => "5",
            Self::Num6 => "6",
            Self::Num7 => "7",
            Self::Num8 => "8",
            Self::Num9 => "9",
            Self::Star => "*",
            Self::Pound => "#",
        }
    }

    pub const fn keycode(self) -> MidpKeyCode {
        match self {
            Self::ArrowUp => MidpKeyCode::from_raw(raw_midp_keycode::ARROW_UP),
            Self::ArrowDown => MidpKeyCode::from_raw(raw_midp_keycode::ARROW_DOWN),
            Self::ArrowLeft => MidpKeyCode::from_raw(raw_midp_keycode::ARROW_LEFT),
            Self::ArrowRight => MidpKeyCode::from_raw(raw_midp_keycode::ARROW_RIGHT),
            Self::Fire => MidpKeyCode::from_raw(raw_midp_keycode::FIRE),
            Self::SoftLeft => MidpKeyCode::from_raw(raw_midp_keycode::SOFT_LEFT),
            Self::SoftRight => MidpKeyCode::from_raw(raw_midp_keycode::SOFT_RIGHT),
            Self::Clear => MidpKeyCode::from_raw(raw_midp_keycode::CLEAR),
            Self::Num0 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_0),
            Self::Num1 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_1),
            Self::Num2 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_2),
            Self::Num3 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_3),
            Self::Num4 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_4),
            Self::Num5 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_5),
            Self::Num6 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_6),
            Self::Num7 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_7),
            Self::Num8 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_8),
            Self::Num9 => MidpKeyCode::from_raw(raw_midp_keycode::NUM_9),
            Self::Star => MidpKeyCode::from_raw(raw_midp_keycode::STAR),
            Self::Pound => MidpKeyCode::from_raw(raw_midp_keycode::POUND),
        }
    }

    pub fn from_keycode(keycode: MidpKeyCode) -> Option<Self> {
        Some(match keycode.raw() {
            raw_midp_keycode::ARROW_UP => Self::ArrowUp,
            raw_midp_keycode::ARROW_DOWN => Self::ArrowDown,
            raw_midp_keycode::ARROW_LEFT => Self::ArrowLeft,
            raw_midp_keycode::ARROW_RIGHT => Self::ArrowRight,
            raw_midp_keycode::FIRE => Self::Fire,
            raw_midp_keycode::SOFT_LEFT => Self::SoftLeft,
            raw_midp_keycode::SOFT_RIGHT => Self::SoftRight,
            raw_midp_keycode::CLEAR => Self::Clear,
            raw_midp_keycode::NUM_0 => Self::Num0,
            raw_midp_keycode::NUM_1 => Self::Num1,
            raw_midp_keycode::NUM_2 => Self::Num2,
            raw_midp_keycode::NUM_3 => Self::Num3,
            raw_midp_keycode::NUM_4 => Self::Num4,
            raw_midp_keycode::NUM_5 => Self::Num5,
            raw_midp_keycode::NUM_6 => Self::Num6,
            raw_midp_keycode::NUM_7 => Self::Num7,
            raw_midp_keycode::NUM_8 => Self::Num8,
            raw_midp_keycode::NUM_9 => Self::Num9,
            raw_midp_keycode::STAR => Self::Star,
            raw_midp_keycode::POUND => Self::Pound,
            _ => return None,
        })
    }

    pub const fn game_action(self) -> GameAction {
        match self {
            Self::ArrowUp | Self::Num2 => GameAction::Up,
            Self::ArrowDown | Self::Num8 => GameAction::Down,
            Self::ArrowLeft | Self::Num4 => GameAction::Left,
            Self::ArrowRight | Self::Num6 => GameAction::Right,
            Self::Fire | Self::Num5 => GameAction::Fire,
            Self::Num1 => GameAction::GameA,
            Self::Num3 => GameAction::GameB,
            Self::Num7 => GameAction::GameC,
            Self::Num9 => GameAction::GameD,
            _ => GameAction::None,
        }
    }

    pub const fn key_state_mask(self) -> Option<i32> {
        Some(match self {
            Self::ArrowUp | Self::Num2 => key_state::UP,
            Self::ArrowDown | Self::Num8 => key_state::DOWN,
            Self::ArrowLeft | Self::Num4 => key_state::LEFT,
            Self::ArrowRight | Self::Num6 => key_state::RIGHT,
            Self::Fire | Self::Num5 => key_state::FIRE,
            Self::Num1 => key_state::GAME_A,
            Self::Num3 => key_state::GAME_B,
            Self::Num7 => key_state::GAME_C,
            Self::Num9 => key_state::GAME_D,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i32)]
pub enum GameAction {
    None = 0,
    Up = 1,
    Left = 2,
    Right = 5,
    Down = 6,
    Fire = 8,
    GameA = 9,
    GameB = 10,
    GameC = 11,
    GameD = 12,
}

impl GameAction {
    pub const fn raw(self) -> i32 {
        self as i32
    }
}

pub mod key_state {
    pub const UP: i32 = 1 << 1;
    pub const LEFT: i32 = 1 << 2;
    pub const RIGHT: i32 = 1 << 5;
    pub const DOWN: i32 = 1 << 6;
    pub const FIRE: i32 = 1 << 8;
    pub const GAME_A: i32 = 1 << 9;
    pub const GAME_B: i32 = 1 << 10;
    pub const GAME_C: i32 = 1 << 11;
    pub const GAME_D: i32 = 1 << 12;
}

pub fn game_action_for_keycode(keycode: MidpKeyCode) -> i32 {
    MidpKey::from_keycode(keycode)
        .map(|key| key.game_action().raw())
        .unwrap_or_else(|| GameAction::None.raw())
}

pub struct InputState {
    pressed_keys: HashSet<MidpKey>,
}

impl InputState {
    fn new() -> Self {
        Self {
            pressed_keys: HashSet::new(),
        }
    }

    pub fn set_pressed(&mut self, key: MidpKey, is_pressed: bool) {
        if is_pressed {
            self.pressed_keys.insert(key);
        } else {
            self.pressed_keys.remove(&key);
        }
    }

    pub fn key_state_mask(&self) -> i32 {
        self.pressed_keys
            .iter()
            .filter_map(|key| key.key_state_mask())
            .fold(0, |state, mask| state | mask)
    }

    pub fn clear(&mut self) {
        self.pressed_keys.clear();
    }
}

pub static INPUT_STATE: LazyLock<Mutex<InputState>> =
    LazyLock::new(|| Mutex::new(InputState::new()));

#[derive(Clone)]
pub struct KeyBindings {
    physical_keys: HashMap<KeyCode, MidpKey>,
    characters: HashMap<char, MidpKey>,
}

pub struct KeyBindingDisplay {
    pub midp_key: MidpKey,
    pub host_keys: Vec<String>,
}

impl KeyBindings {
    pub fn new() -> Self {
        Self {
            physical_keys: HashMap::new(),
            characters: HashMap::new(),
        }
    }

    pub fn bind_physical_key(&mut self, host_key: KeyCode, midp_key: MidpKey) {
        self.physical_keys.insert(host_key, midp_key);
    }

    #[allow(dead_code)]
    pub fn unbind_physical_key(&mut self, host_key: KeyCode) {
        self.physical_keys.remove(&host_key);
    }

    pub fn bind_character(&mut self, character: char, midp_key: MidpKey) {
        self.characters.insert(character, midp_key);
    }

    #[allow(dead_code)]
    pub fn unbind_character(&mut self, character: char) {
        self.characters.remove(&character);
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.physical_keys.clear();
        self.characters.clear();
    }

    pub fn key_for_event(&self, event: &KeyEvent, physical_key: KeyCode) -> Option<MidpKey> {
        self.key_for_logical_key(&event.logical_key)
            .or_else(|| self.physical_keys.get(&physical_key).copied())
    }

    pub fn display_rows(&self) -> Vec<KeyBindingDisplay> {
        MIDP_KEY_DISPLAY_ORDER
            .iter()
            .copied()
            .filter_map(|midp_key| {
                let mut host_keys = Vec::new();

                for (host_key, mapped_key) in &self.physical_keys {
                    if *mapped_key == midp_key {
                        push_unique_sorted(&mut host_keys, host_key_label(*host_key));
                    }
                }

                for (character, mapped_key) in &self.characters {
                    if *mapped_key == midp_key {
                        push_unique_sorted(&mut host_keys, character_label(*character));
                    }
                }

                (!host_keys.is_empty()).then_some(KeyBindingDisplay {
                    midp_key,
                    host_keys,
                })
            })
            .collect()
    }

    fn bind_physical_keys(&mut self, host_keys: &[KeyCode], midp_key: MidpKey) {
        for host_key in host_keys {
            self.bind_physical_key(*host_key, midp_key);
        }
    }

    fn key_for_logical_key(&self, key: &Key) -> Option<MidpKey> {
        let Key::Character(text) = key.as_ref() else {
            return None;
        };

        let mut chars = text.chars();
        let character = chars.next()?;
        if chars.next().is_some() {
            return None;
        }

        self.characters.get(&character).copied()
    }
}

fn push_unique_sorted(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
        values.sort();
    }
}

fn character_label(character: char) -> String {
    character.to_string()
}

fn host_key_label(host_key: KeyCode) -> String {
    match host_key {
        KeyCode::ArrowUp => "Arrow Up".to_string(),
        KeyCode::ArrowDown => "Arrow Down".to_string(),
        KeyCode::ArrowLeft => "Arrow Left".to_string(),
        KeyCode::ArrowRight => "Arrow Right".to_string(),
        KeyCode::Space => "Space".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::NumpadEnter => "Numpad Enter".to_string(),
        KeyCode::Escape => "Esc".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::KeyA => "A".to_string(),
        KeyCode::KeyD => "D".to_string(),
        KeyCode::KeyE => "E".to_string(),
        KeyCode::KeyF => "F".to_string(),
        KeyCode::KeyQ => "Q".to_string(),
        KeyCode::KeyS => "S".to_string(),
        KeyCode::Digit0 => "0".to_string(),
        KeyCode::Digit1 => "1".to_string(),
        KeyCode::Digit2 => "2".to_string(),
        KeyCode::Digit3 => "3".to_string(),
        KeyCode::Digit4 => "4".to_string(),
        KeyCode::Digit5 => "5".to_string(),
        KeyCode::Digit6 => "6".to_string(),
        KeyCode::Digit7 => "7".to_string(),
        KeyCode::Digit8 => "8".to_string(),
        KeyCode::Digit9 => "9".to_string(),
        KeyCode::Numpad0 => "Numpad 0".to_string(),
        KeyCode::Numpad1 => "Numpad 1".to_string(),
        KeyCode::Numpad2 => "Numpad 2".to_string(),
        KeyCode::Numpad3 => "Numpad 3".to_string(),
        KeyCode::Numpad4 => "Numpad 4".to_string(),
        KeyCode::Numpad5 => "Numpad 5".to_string(),
        KeyCode::Numpad6 => "Numpad 6".to_string(),
        KeyCode::Numpad7 => "Numpad 7".to_string(),
        KeyCode::Numpad8 => "Numpad 8".to_string(),
        KeyCode::Numpad9 => "Numpad 9".to_string(),
        KeyCode::NumpadMultiply | KeyCode::NumpadStar => "Numpad *".to_string(),
        KeyCode::NumpadHash => "Numpad #".to_string(),
        _ => format!("{:?}", host_key),
    }
}

impl Default for KeyBindings {
    fn default() -> Self {
        let mut bindings = Self::new();

        bindings.bind_physical_keys(
            &[KeyCode::Space, KeyCode::Enter, KeyCode::NumpadEnter],
            MidpKey::Fire,
        );
        bindings.bind_physical_key(KeyCode::ArrowUp, MidpKey::ArrowUp);
        bindings.bind_physical_key(KeyCode::ArrowDown, MidpKey::ArrowDown);
        bindings.bind_physical_key(KeyCode::ArrowLeft, MidpKey::ArrowLeft);
        bindings.bind_physical_key(KeyCode::ArrowRight, MidpKey::ArrowRight);
        bindings.bind_physical_key(KeyCode::KeyQ, MidpKey::SoftLeft);
        bindings.bind_physical_key(KeyCode::KeyE, MidpKey::SoftRight);
        bindings.bind_physical_keys(
            &[KeyCode::Escape, KeyCode::Backspace, KeyCode::Delete],
            MidpKey::Clear,
        );

        bindings.bind_physical_keys(
            &[KeyCode::KeyA, KeyCode::Digit1, KeyCode::Numpad1],
            MidpKey::Num1,
        );
        bindings.bind_physical_keys(&[KeyCode::Digit2, KeyCode::Numpad2], MidpKey::Num2);
        bindings.bind_physical_keys(
            &[KeyCode::KeyS, KeyCode::Digit3, KeyCode::Numpad3],
            MidpKey::Num3,
        );
        bindings.bind_physical_keys(&[KeyCode::Digit4, KeyCode::Numpad4], MidpKey::Num4);
        bindings.bind_physical_keys(&[KeyCode::Digit5, KeyCode::Numpad5], MidpKey::Num5);
        bindings.bind_physical_keys(&[KeyCode::Digit6, KeyCode::Numpad6], MidpKey::Num6);
        bindings.bind_physical_keys(
            &[KeyCode::KeyD, KeyCode::Digit7, KeyCode::Numpad7],
            MidpKey::Num7,
        );
        bindings.bind_physical_keys(&[KeyCode::Digit8, KeyCode::Numpad8], MidpKey::Num8);
        bindings.bind_physical_keys(
            &[KeyCode::KeyF, KeyCode::Digit9, KeyCode::Numpad9],
            MidpKey::Num9,
        );
        bindings.bind_physical_keys(&[KeyCode::Digit0, KeyCode::Numpad0], MidpKey::Num0);
        bindings.bind_physical_keys(
            &[KeyCode::NumpadMultiply, KeyCode::NumpadStar],
            MidpKey::Star,
        );
        bindings.bind_physical_key(KeyCode::NumpadHash, MidpKey::Pound);

        bindings.bind_character('0', MidpKey::Num0);
        bindings.bind_character('1', MidpKey::Num1);
        bindings.bind_character('2', MidpKey::Num2);
        bindings.bind_character('3', MidpKey::Num3);
        bindings.bind_character('4', MidpKey::Num4);
        bindings.bind_character('5', MidpKey::Num5);
        bindings.bind_character('6', MidpKey::Num6);
        bindings.bind_character('7', MidpKey::Num7);
        bindings.bind_character('8', MidpKey::Num8);
        bindings.bind_character('9', MidpKey::Num9);
        bindings.bind_character('*', MidpKey::Star);
        bindings.bind_character('#', MidpKey::Pound);

        bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midp_keys_convert_to_j2me_keycodes() {
        assert_eq!(MidpKey::Star.keycode().raw(), '*' as i32);
        assert_eq!(MidpKey::Pound.keycode().raw(), '#' as i32);
        assert_eq!(MidpKey::Num5.keycode().raw(), '5' as i32);
        assert_eq!(
            MidpKey::from_keycode(MidpKey::Num5.keycode()),
            Some(MidpKey::Num5)
        );
    }

    #[test]
    fn game_actions_support_directional_keypad_aliases() {
        assert_eq!(
            game_action_for_keycode(MidpKey::Num2.keycode()),
            GameAction::Up.raw()
        );
        assert_eq!(
            game_action_for_keycode(MidpKey::Num8.keycode()),
            GameAction::Down.raw()
        );
        assert_eq!(
            game_action_for_keycode(MidpKey::Num5.keycode()),
            GameAction::Fire.raw()
        );
    }

    #[test]
    fn key_state_mask_tracks_pressed_midp_keys() {
        let mut input_state = InputState::new();

        input_state.set_pressed(MidpKey::Num2, true);
        input_state.set_pressed(MidpKey::Fire, true);
        assert_eq!(
            input_state.key_state_mask(),
            key_state::UP | key_state::FIRE
        );

        input_state.set_pressed(MidpKey::Num2, false);
        assert_eq!(input_state.key_state_mask(), key_state::FIRE);
    }
}
