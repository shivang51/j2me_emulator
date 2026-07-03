use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use crate::jvm::jvm_core::{HeapObject, JvmStackValue, JVM};

pub const CLASS_NAME: &str = "javax/microedition/media/Player";
pub const MANAGER_CLASS_NAME: &str = "javax/microedition/media/Manager";
pub const VOLUME_CONTROL_CLASS_NAME: &str = "javax/microedition/media/control/VolumeControl";

const STATE_UNREALIZED: i32 = 100;
const STATE_REALIZED: i32 = 200;
const STATE_PREFETCHED: i32 = 300;
const STATE_STARTED: i32 = 400;
const STATE_CLOSED: i32 = 0;
const SOUNDFONT_ENV_VAR: &str = "J2ME_EMULATOR_SOUNDFONT";
const BUNDLED_SOUNDFONT_DIR: &str = "assets/soundfonts";
const BUNDLED_SOUNDFONT_NAMES: &[&str] = &[
    "default.sf2",
    "FluidR3_GM.sf2",
    "GeneralUser-GS.sf2",
    "TimGM6mb.sf2",
];

struct PlayerRuntime {
    content_type: String,
    locator: Option<String>,
    media_data: Vec<u8>,
    playback: Option<audio_backend::PlaybackHandle>,
    loop_count: i32,
    volume: i32,
    state: i32,
    listeners: Vec<u32>,
}

static PLAYERS: LazyLock<Mutex<HashMap<usize, PlayerRuntime>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn pause_all() {
    let players = PLAYERS.lock().unwrap();
    for player in players.values() {
        if let Some(playback) = &player.playback {
            playback.pause();
        }
    }
}

pub fn resume_all() {
    let players = PLAYERS.lock().unwrap();
    for player in players.values() {
        if let Some(playback) = &player.playback {
            playback.resume();
        }
    }
}

pub fn stop_all() {
    let mut players = PLAYERS.lock().unwrap();
    let players_to_stop = std::mem::take(&mut *players);
    drop(players);

    for mut player in players_to_stop.into_values() {
        if let Some(playback) = player.playback.take() {
            playback.stop();
        }
    }
}

pub fn poll_finished(jvm: &JVM) {
    let finished_players = {
        let mut players = PLAYERS.lock().unwrap();
        players
            .iter_mut()
            .filter_map(|(player_id, player)| {
                if reap_finished_playback(player) {
                    player.state = STATE_PREFETCHED;
                    Some(*player_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };

    for player_id in finished_players {
        set_heap_state(player_id, STATE_PREFETCHED, jvm);
        notify_player_listeners(jvm, player_id, "endOfMedia");
    }
}

pub fn handle_manager_static_method(
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        (
            "createPlayer",
            "(Ljava/io/InputStream;Ljava/lang/String;)Ljavax/microedition/media/Player;",
        ) => create_player_from_stream(args, jvm),
        ("createPlayer", "(Ljava/lang/String;)Ljavax/microedition/media/Player;") => {
            create_player_from_locator(args, jvm)
        }
        ("playTone", "(III)V") => {
            let note = get_int_arg(args, 0, "Manager.playTone note")?;
            let duration_ms = get_int_arg(args, 1, "Manager.playTone duration")?;
            let volume = get_int_arg(args, 2, "Manager.playTone volume")?;
            audio_backend::play_tone(note, duration_ms, volume);
            Ok(None)
        }
        _ => Err(format!(
            "Unsupported Manager static method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_virtual_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let player_id = object_id(objectref, "Player")?;

    match (method_name, descriptor) {
        ("realize", "()V") => {
            set_state(player_id, STATE_REALIZED, jvm);
            Ok(None)
        }
        ("prefetch", "()V") => {
            set_state(player_id, STATE_PREFETCHED, jvm);
            Ok(None)
        }
        ("start", "()V") => {
            let state = start_player(player_id);
            set_state(player_id, state, jvm);
            if state == STATE_STARTED {
                notify_player_listeners(jvm, player_id, "started");
            }
            Ok(None)
        }
        ("stop", "()V") => {
            stop_player(player_id);
            set_state(player_id, STATE_PREFETCHED, jvm);
            notify_player_listeners(jvm, player_id, "stopped");
            Ok(None)
        }
        ("deallocate", "()V") => {
            stop_player(player_id);
            set_state(player_id, STATE_REALIZED, jvm);
            Ok(None)
        }
        ("close", "()V") => {
            notify_player_listeners(jvm, player_id, "closed");
            close_player(player_id);
            set_state(player_id, STATE_CLOSED, jvm);
            Ok(None)
        }
        ("setLoopCount", "(I)V") => {
            let loop_count = get_int_arg(args, 0, "Player.setLoopCount")?;
            if let Some(player) = PLAYERS.lock().unwrap().get_mut(&player_id) {
                player.loop_count = loop_count;
            }
            Ok(None)
        }
        ("addPlayerListener", "(Ljavax/microedition/media/PlayerListener;)V") => {
            let listener_id = listener_arg(args, 0, "Player.addPlayerListener")?;
            add_player_listener(player_id, listener_id, jvm);
            Ok(None)
        }
        ("removePlayerListener", "(Ljavax/microedition/media/PlayerListener;)V") => {
            let listener_id = listener_arg(args, 0, "Player.removePlayerListener")?;
            remove_player_listener(player_id, listener_id, jvm);
            Ok(None)
        }
        ("getMediaTime", "()J") => Ok(Some(JvmStackValue::Long(0))),
        ("setMediaTime", "(J)J") => Ok(Some(
            args.first().cloned().unwrap_or(JvmStackValue::Long(0)),
        )),
        ("setMediaTime", "(J)V") => Ok(None),
        ("getDuration", "()J") => Ok(Some(JvmStackValue::Long(-1))),
        ("getContentType", "()Ljava/lang/String;") => Ok(Some(JvmStackValue::String(
            PLAYERS
                .lock()
                .unwrap()
                .get(&player_id)
                .map(|player| player.content_type.clone())
                .unwrap_or_default(),
        ))),
        ("getControl", "(Ljava/lang/String;)Ljavax/microedition/media/Control;") => {
            let control_name = match args.first() {
                Some(JvmStackValue::String(name)) => name.as_str(),
                _ => "",
            };

            if is_volume_control_name(control_name) {
                Ok(Some(JvmStackValue::ObjectRef(allocate_volume_control(
                    jvm, player_id,
                ))))
            } else {
                Ok(Some(JvmStackValue::Null))
            }
        }
        ("getControls", "()[Ljavax/microedition/media/Control;") => {
            let control_ref = allocate_volume_control(jvm, player_id);
            let array_ref = {
                let mut state = jvm.state.lock();
                state.heap.push(HeapObject::Array {
                    element_type: "javax/microedition/media/Control".to_string(),
                    data: vec![JvmStackValue::ObjectRef(control_ref)],
                });
                (state.heap.len() - 1) as u32
            };
            Ok(Some(JvmStackValue::ObjectRef(array_ref)))
        }
        ("getState", "()I") => Ok(Some(JvmStackValue::Int(current_player_state(
            player_id, jvm,
        )))),
        _ => Err(format!(
            "Unsupported Player instance method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_volume_control_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let control_id = object_id(objectref, "VolumeControl")?;
    let player_id = {
        let state = jvm.state.lock();
        match state.heap.get(control_id) {
            Some(HeapObject::Instance(obj)) => match obj.fields.get("playerId:I") {
                Some(JvmStackValue::Int(id)) => *id as usize,
                _ => return Err("VolumeControl missing playerId".into()),
            },
            _ => return Err("VolumeControl object is not an instance".into()),
        }
    };

    match (method_name, descriptor) {
        ("setLevel", "(I)I") => {
            let level = get_int_arg(args, 0, "VolumeControl.setLevel")?.clamp(0, 100);
            if let Some(player) = PLAYERS.lock().unwrap().get_mut(&player_id) {
                player.volume = level;
                if let Some(playback) = &player.playback {
                    playback.set_volume(level);
                }
            }
            Ok(Some(JvmStackValue::Int(level)))
        }
        ("getLevel", "()I") => Ok(Some(JvmStackValue::Int(
            PLAYERS
                .lock()
                .unwrap()
                .get(&player_id)
                .map(|player| player.volume)
                .unwrap_or(100),
        ))),
        ("setMute", "(Z)V") => Ok(None),
        ("isMuted", "()Z") => Ok(Some(JvmStackValue::Int(0))),
        _ => Err(format!(
            "Unsupported VolumeControl method: {}{}",
            method_name, descriptor
        )),
    }
}

fn create_player_from_stream(
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let stream_ref = match args.first() {
        Some(JvmStackValue::ObjectRef(id)) => *id as usize,
        Some(JvmStackValue::Null) => return Err("Manager.createPlayer: null stream".into()),
        value => {
            return Err(format!(
                "Manager.createPlayer: expected InputStream, found {:?}",
                value
            ));
        }
    };

    let content_type = match args.get(1) {
        Some(JvmStackValue::String(value)) => value.clone(),
        _ => "application/octet-stream".to_string(),
    };

    let (locator, data) = {
        let state = jvm.state.lock();
        let Some(HeapObject::Instance(stream)) = state.heap.get(stream_ref) else {
            return Err("Manager.createPlayer: stream is not an instance".into());
        };

        let locator = match stream.fields.get("jvm_res") {
            Some(JvmStackValue::String(path)) => Some(path.clone()),
            _ => None,
        };

        let data = match stream.fields.get("jvm_data") {
            Some(JvmStackValue::Vector(values)) => {
                jvm_byte_values_to_bytes(values, "Manager.createPlayer stream jvm_data")?
            }
            Some(value) => {
                return Err(format!(
                    "Manager.createPlayer: invalid stream jvm_data field {:?}",
                    value
                ));
            }
            None => locator
                .as_ref()
                .and_then(|path| state.resources.get(path))
                .cloned()
                .unwrap_or_default(),
        };

        (locator, data)
    };

    Ok(Some(JvmStackValue::ObjectRef(register_player(
        jvm,
        content_type,
        locator,
        data,
    ))))
}

fn create_player_from_locator(
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let locator = match args.first() {
        Some(JvmStackValue::String(value)) => value.clone(),
        Some(JvmStackValue::Null) => return Err("Manager.createPlayer: null locator".into()),
        value => {
            return Err(format!(
                "Manager.createPlayer: expected String locator, found {:?}",
                value
            ));
        }
    };

    let data = {
        let state = jvm.state.lock();
        resource_key_from_locator(&locator).and_then(|key| state.resources.get(&key).cloned())
    }
    .or_else(|| file_path_from_locator(&locator).and_then(|path| fs::read(path).ok()))
    .unwrap_or_default();

    let content_type = infer_content_type(Some(&locator), &data);

    Ok(Some(JvmStackValue::ObjectRef(register_player(
        jvm,
        content_type,
        Some(locator),
        data,
    ))))
}

fn register_player(
    jvm: &JVM,
    content_type: String,
    locator: Option<String>,
    media_data: Vec<u8>,
) -> u32 {
    let mut fields = HashMap::new();
    fields.insert("state:I".to_string(), JvmStackValue::Int(STATE_UNREALIZED));
    fields.insert(
        "contentType:Ljava/lang/String;".to_string(),
        JvmStackValue::String(content_type.clone()),
    );
    fields.insert(
        "locator:Ljava/lang/String;".to_string(),
        JvmStackValue::String(locator.clone().unwrap_or_default()),
    );
    fields.insert(
        "listeners:Ljava/util/Vector;".to_string(),
        JvmStackValue::Vector(Vec::new()),
    );

    let player_id = {
        let mut state = jvm.state.lock();
        JVM::allocate_internal(&mut state, CLASS_NAME.to_string(), fields)
    };

    PLAYERS.lock().unwrap().insert(
        player_id as usize,
        PlayerRuntime {
            content_type,
            locator,
            media_data,
            playback: None,
            loop_count: 1,
            volume: 100,
            state: STATE_UNREALIZED,
            listeners: Vec::new(),
        },
    );

    player_id
}

fn allocate_volume_control(jvm: &JVM, player_id: usize) -> u32 {
    let mut fields = HashMap::new();
    fields.insert(
        "playerId:I".to_string(),
        JvmStackValue::Int(player_id as i32),
    );

    let mut state = jvm.state.lock();
    JVM::allocate_internal(&mut state, VOLUME_CONTROL_CLASS_NAME.to_string(), fields)
}

fn add_player_listener(player_id: usize, listener_id: u32, jvm: &JVM) {
    let listeners = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(player) = players.get_mut(&player_id) else {
            return;
        };

        if !player.listeners.contains(&listener_id) {
            player.listeners.push(listener_id);
        }
        player.listeners.clone()
    };

    sync_heap_listeners(player_id, &listeners, jvm);
}

fn remove_player_listener(player_id: usize, listener_id: u32, jvm: &JVM) {
    let listeners = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(player) = players.get_mut(&player_id) else {
            return;
        };

        player.listeners.retain(|id| *id != listener_id);
        player.listeners.clone()
    };

    sync_heap_listeners(player_id, &listeners, jvm);
}

fn sync_heap_listeners(player_id: usize, listeners: &[u32], jvm: &JVM) {
    let mut state = jvm.state.lock();
    if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(player_id) {
        obj.fields.insert(
            "listeners:Ljava/util/Vector;".to_string(),
            JvmStackValue::Vector(
                listeners
                    .iter()
                    .copied()
                    .map(JvmStackValue::ObjectRef)
                    .collect(),
            ),
        );
    }
}

fn player_listeners(player_id: usize) -> Vec<u32> {
    PLAYERS
        .lock()
        .unwrap()
        .get(&player_id)
        .map(|player| player.listeners.clone())
        .unwrap_or_default()
}

fn notify_player_listeners(jvm: &JVM, player_id: usize, event: &str) {
    let listeners = player_listeners(player_id);
    if listeners.is_empty() {
        return;
    }

    let listener_classes: Vec<(u32, String)> = {
        let state = jvm.state.lock();
        listeners
            .iter()
            .filter_map(|listener_id| match state.heap.get(*listener_id as usize) {
                Some(HeapObject::Instance(listener)) => {
                    Some((*listener_id, listener.class_name.clone()))
                }
                _ => None,
            })
            .collect()
    };

    for (listener_id, class_name) in listener_classes {
        let mut callback_stack = Vec::new();
        if let Err(err) = JVM::execute_method(
            JvmStackValue::ObjectRef(listener_id),
            &class_name,
            "playerUpdate",
            "(Ljavax/microedition/media/Player;Ljava/lang/String;Ljava/lang/Object;)V",
            &[
                JvmStackValue::ObjectRef(player_id as u32),
                JvmStackValue::String(event.to_string()),
                JvmStackValue::Null,
            ],
            jvm,
            &mut callback_stack,
        ) {
            eprintln!(
                "[Media] Player listener {}.playerUpdate({}) failed: {}",
                class_name, event, err
            );
        }
    }
}

fn start_player(player_id: usize) -> i32 {
    stop_player(player_id);

    let mut players = PLAYERS.lock().unwrap();
    let Some(player) = players.get_mut(&player_id) else {
        return STATE_CLOSED;
    };

    if player.media_data.is_empty() {
        if let Some(locator) = &player.locator {
            eprintln!("[Media] No media bytes available for {}", locator);
        }
        player.state = STATE_PREFETCHED;
        return player.state;
    }

    player.playback = audio_backend::start_player(
        &player.media_data,
        &player.content_type,
        player.loop_count,
        player.volume,
    );
    player.state = if player.playback.is_some() {
        STATE_STARTED
    } else {
        STATE_PREFETCHED
    };
    player.state
}

fn stop_player(player_id: usize) {
    let mut players = PLAYERS.lock().unwrap();
    let Some(player) = players.get_mut(&player_id) else {
        return;
    };

    if let Some(playback) = player.playback.take() {
        playback.stop();
    }
}

fn close_player(player_id: usize) {
    stop_player(player_id);
    PLAYERS.lock().unwrap().remove(&player_id);
}

fn set_state(player_id: usize, state_value: i32, jvm: &JVM) {
    if let Some(player) = PLAYERS.lock().unwrap().get_mut(&player_id) {
        player.state = state_value;
    }

    set_heap_state(player_id, state_value, jvm);
}

fn set_heap_state(player_id: usize, state_value: i32, jvm: &JVM) {
    let mut state = jvm.state.lock();
    if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(player_id) {
        obj.fields
            .insert("state:I".to_string(), JvmStackValue::Int(state_value));
    }
}

fn current_player_state(player_id: usize, jvm: &JVM) -> i32 {
    let mut state_to_sync = None;
    let mut notify_end_of_media = false;
    let state_value = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(player) = players.get_mut(&player_id) else {
            return STATE_CLOSED;
        };

        if reap_finished_playback(player) {
            player.state = STATE_PREFETCHED;
            state_to_sync = Some(player.state);
            notify_end_of_media = true;
        }

        player.state
    };

    if let Some(state_value) = state_to_sync {
        set_heap_state(player_id, state_value, jvm);
    }

    if notify_end_of_media {
        notify_player_listeners(jvm, player_id, "endOfMedia");
    }

    state_value
}

fn reap_finished_playback(player: &mut PlayerRuntime) -> bool {
    let Some(playback) = player.playback.as_ref() else {
        return false;
    };

    if playback.is_finished() {
        player.playback.take();
        true
    } else {
        false
    }
}

fn get_int_arg(args: &[JvmStackValue], index: usize, name: &str) -> Result<i32, String> {
    match args.get(index) {
        Some(JvmStackValue::Int(value)) => Ok(*value),
        value => Err(format!("{}: expected int, found {:?}", name, value)),
    }
}

fn listener_arg(args: &[JvmStackValue], index: usize, name: &str) -> Result<u32, String> {
    match args.get(index) {
        Some(JvmStackValue::ObjectRef(id)) => Ok(*id),
        Some(JvmStackValue::Null) => Err(format!("{}: NullPointerException", name)),
        value => Err(format!(
            "{}: expected PlayerListener object, found {:?}",
            name, value
        )),
    }
}

fn jvm_byte_values_to_bytes(values: &[JvmStackValue], context: &str) -> Result<Vec<u8>, String> {
    values
        .iter()
        .map(|value| match value {
            JvmStackValue::Byte(byte) => Ok(*byte),
            JvmStackValue::Int(int_value) => Ok(*int_value as u8),
            value => Err(format!(
                "{}: expected byte value, found {:?}",
                context, value
            )),
        })
        .collect()
}

fn object_id(value: &JvmStackValue, class_name: &str) -> Result<usize, String> {
    match value {
        JvmStackValue::ObjectRef(id) => Ok(*id as usize),
        JvmStackValue::Null => Err(format!("{}: NullPointerException", class_name)),
        value => Err(format!(
            "{}: expected object reference, found {:?}",
            class_name, value
        )),
    }
}

fn is_volume_control_name(name: &str) -> bool {
    name == "VolumeControl"
        || name == "javax.microedition.media.control.VolumeControl"
        || name.ends_with(".VolumeControl")
}

fn resource_key_from_locator(locator: &str) -> Option<String> {
    let stripped = locator
        .strip_prefix("resource://")
        .or_else(|| locator.strip_prefix("resource:"))
        .or_else(|| locator.strip_prefix("file://"))
        .unwrap_or(locator);
    let stripped = stripped.strip_prefix('/').unwrap_or(stripped);
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn file_path_from_locator(locator: &str) -> Option<PathBuf> {
    locator
        .strip_prefix("file://")
        .map(PathBuf::from)
        .or_else(|| {
            if Path::new(locator).is_absolute() {
                Some(PathBuf::from(locator))
            } else {
                None
            }
        })
}

fn infer_content_type(locator: Option<&str>, data: &[u8]) -> String {
    if data.starts_with(b"MThd") {
        return "audio/midi".to_string();
    }
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WAVE") {
        return "audio/x-wav".to_string();
    }
    if data.starts_with(b"ID3") || data.first().copied() == Some(0xFF) {
        return "audio/mpeg".to_string();
    }
    if data.starts_with(b"OggS") {
        return "audio/ogg".to_string();
    }

    match locator
        .and_then(|path| Path::new(path).extension())
        .and_then(|ext| ext.to_str())
    {
        Some(ext) if ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi") => {
            "audio/midi".to_string()
        }
        Some(ext) if ext.eq_ignore_ascii_case("wav") => "audio/x-wav".to_string(),
        Some(ext) if ext.eq_ignore_ascii_case("mp3") => "audio/mpeg".to_string(),
        Some(ext) if ext.eq_ignore_ascii_case("amr") => "audio/amr".to_string(),
        Some(ext) if ext.eq_ignore_ascii_case("ogg") => "audio/ogg".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn is_midi_media(content_type: &str, data: &[u8]) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    data.starts_with(b"MThd") || content_type.contains("midi") || content_type.contains("audio/mid")
}

fn find_soundfont() -> Option<String> {
    if let Some(path) = env::var_os(SOUNDFONT_ENV_VAR)
        .map(PathBuf::from)
        .and_then(existing_soundfont_file)
    {
        return Some(path);
    }

    for dir in bundled_soundfont_dirs() {
        if let Some(path) = find_soundfont_in_dir(&dir) {
            return Some(path);
        }
    }

    None
}

fn bundled_soundfont_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            dirs.push(exe_dir.join(BUNDLED_SOUNDFONT_DIR));
            dirs.push(exe_dir.join("soundfonts"));
        }
    }

    if let Ok(current_dir) = env::current_dir() {
        dirs.push(current_dir.join(BUNDLED_SOUNDFONT_DIR));
    }

    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BUNDLED_SOUNDFONT_DIR));
    dirs
}

fn find_soundfont_in_dir(dir: &Path) -> Option<String> {
    for name in BUNDLED_SOUNDFONT_NAMES {
        if let Some(path) = existing_soundfont_file(dir.join(name)) {
            return Some(path);
        }
    }

    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("sf2") || ext.eq_ignore_ascii_case("sf3"))
                .unwrap_or(false)
        })
        .find_map(existing_soundfont_file)
}

fn existing_soundfont_file(path: PathBuf) -> Option<String> {
    if path.is_file() {
        Some(path.to_string_lossy().to_string())
    } else {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod audio_backend {
    use std::{
        fs,
        io::Cursor,
        sync::{Arc, LazyLock, Mutex},
        time::Duration,
    };

    use rodio::{
        buffer::SamplesBuffer,
        source::{SineWave, Source},
        Decoder, DeviceSinkBuilder, MixerDeviceSink, Player,
    };
    use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};

    use super::{find_soundfont, is_midi_media, BUNDLED_SOUNDFONT_DIR, SOUNDFONT_ENV_VAR};

    const SAMPLE_RATE: i32 = 44_100;

    static AUDIO_OUTPUT: LazyLock<Mutex<Option<MixerDeviceSink>>> =
        LazyLock::new(|| Mutex::new(None));
    static SOUND_FONT: LazyLock<Mutex<Option<Arc<SoundFont>>>> = LazyLock::new(|| Mutex::new(None));

    pub struct PlaybackHandle {
        player: Player,
    }

    impl PlaybackHandle {
        pub fn pause(&self) {
            self.player.pause();
        }

        pub fn resume(&self) {
            self.player.play();
        }

        pub fn stop(self) {
            self.player.stop();
        }

        pub fn is_finished(&self) -> bool {
            self.player.empty()
        }

        pub fn set_volume(&self, volume: i32) {
            self.player.set_volume(volume_to_gain(volume));
        }
    }

    pub fn start_player(
        data: &[u8],
        content_type: &str,
        loop_count: i32,
        volume: i32,
    ) -> Option<PlaybackHandle> {
        let player = match open_player(volume) {
            Ok(player) => player,
            Err(err) => {
                eprintln!("[Media] Failed to open audio output: {}", err);
                return None;
            }
        };

        if is_midi_media(content_type, data) {
            let source = match synthesize_midi(data) {
                Ok(source) => source,
                Err(err) => {
                    eprintln!("[Media] Failed to synthesize MIDI: {}", err);
                    return None;
                }
            };
            append_looping_source(&player, source, loop_count);
        } else {
            let repeats = finite_loop_count(loop_count);
            if loop_count == -1 {
                let source = decode_audio(data)?;
                player.append(source.repeat_infinite());
            } else {
                for _ in 0..repeats {
                    player.append(decode_audio(data)?);
                }
            }
        }

        Some(PlaybackHandle { player })
    }

    pub fn play_tone(note: i32, duration_ms: i32, volume: i32) {
        let Ok(player) = open_player(volume) else {
            return;
        };

        let frequency = 8.175_798_915_6_f32 * 2_f32.powf(note as f32 / 12.0);
        let duration = Duration::from_millis(duration_ms.max(1) as u64);
        player.append(SineWave::new(frequency).take_duration(duration));
        player.detach();
    }

    fn open_player(volume: i32) -> Result<Player, String> {
        let mut output = AUDIO_OUTPUT.lock().unwrap();
        if output.is_none() {
            *output = Some(
                DeviceSinkBuilder::open_default_sink()
                    .map_err(|err| format!("open default sink failed: {}", err))?,
            );
        }

        let output = output
            .as_ref()
            .ok_or_else(|| "audio output was not initialized".to_string())?;
        let player = Player::connect_new(&output.mixer());
        player.set_volume(volume_to_gain(volume));
        Ok(player)
    }

    fn decode_audio(data: &[u8]) -> Option<Decoder<Cursor<Vec<u8>>>> {
        match Decoder::try_from(Cursor::new(data.to_vec())) {
            Ok(decoder) => Some(decoder),
            Err(err) => {
                eprintln!("[Media] Failed to decode audio data: {}", err);
                None
            }
        }
    }

    fn synthesize_midi(data: &[u8]) -> Result<SamplesBuffer, String> {
        let sound_font = load_sound_font()?;
        let mut midi_data = Cursor::new(data.to_vec());
        let midi_file = Arc::new(
            MidiFile::new(&mut midi_data).map_err(|err| format!("invalid MIDI file: {}", err))?,
        );
        let settings = SynthesizerSettings::new(SAMPLE_RATE);
        let synthesizer = Synthesizer::new(&sound_font, &settings)
            .map_err(|err| format!("synthesizer init failed: {}", err))?;
        let mut sequencer = MidiFileSequencer::new(synthesizer);
        sequencer.play(&midi_file, false);

        let sample_count = ((SAMPLE_RATE as f64 * midi_file.get_length()).ceil() as usize)
            .max((SAMPLE_RATE / 10) as usize);
        let mut left = vec![0.0f32; sample_count];
        let mut right = vec![0.0f32; sample_count];
        sequencer.render(&mut left, &mut right);

        let mut interleaved = Vec::with_capacity(sample_count * 2);
        for (left, right) in left.into_iter().zip(right) {
            interleaved.push(left);
            interleaved.push(right);
        }

        Ok(SamplesBuffer::new(
            rodio::nz!(2),
            rodio::nz!(44_100),
            interleaved,
        ))
    }

    fn load_sound_font() -> Result<Arc<SoundFont>, String> {
        let mut cached = SOUND_FONT.lock().unwrap();
        if let Some(sound_font) = cached.as_ref() {
            return Ok(sound_font.clone());
        }

        let path = find_soundfont().ok_or_else(|| {
            format!(
                "MIDI playback needs a SoundFont in {} or {}",
                BUNDLED_SOUNDFONT_DIR, SOUNDFONT_ENV_VAR
            )
        })?;
        let bytes =
            fs::read(&path).map_err(|err| format!("failed to read SoundFont {}: {}", path, err))?;
        let mut cursor = Cursor::new(bytes);
        let sound_font = Arc::new(
            SoundFont::new(&mut cursor).map_err(|err| format!("invalid SoundFont: {}", err))?,
        );
        *cached = Some(sound_font.clone());
        Ok(sound_font)
    }

    fn append_looping_source(player: &Player, source: SamplesBuffer, loop_count: i32) {
        if loop_count == -1 {
            player.append(source.repeat_infinite());
        } else {
            for _ in 0..finite_loop_count(loop_count) {
                player.append(source.clone());
            }
        }
    }

    fn finite_loop_count(loop_count: i32) -> i32 {
        loop_count.max(1)
    }

    fn volume_to_gain(volume: i32) -> f32 {
        volume.clamp(0, 100) as f32 / 100.0
    }
}

#[cfg(target_arch = "wasm32")]
mod audio_backend {
    pub struct PlaybackHandle;

    impl PlaybackHandle {
        pub fn pause(&self) {}
        pub fn resume(&self) {}
        pub fn stop(self) {}
        pub fn is_finished(&self) -> bool {
            true
        }
        pub fn set_volume(&self, _volume: i32) {}
    }

    pub fn start_player(
        _data: &[u8],
        _content_type: &str,
        _loop_count: i32,
        _volume: i32,
    ) -> Option<PlaybackHandle> {
        eprintln!("[Media] Web audio backend is not wired yet");
        None
    }

    pub fn play_tone(_note: i32, _duration_ms: i32, _volume: i32) {}
}
