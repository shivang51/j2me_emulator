use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{LazyLock, Mutex},
};

use crate::jvm::jvm_core::{HeapObject, JVM, JvmStackValue};

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
    media_file: Option<PathBuf>,
    child: Option<Child>,
    loop_count: i32,
    volume: i32,
    state: i32,
}

static PLAYERS: LazyLock<Mutex<HashMap<usize, PlayerRuntime>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn pause_all() {
    let players = PLAYERS.lock().unwrap();
    for player in players.values() {
        if let Some(child) = &player.child {
            signal_child(child, ChildSignal::Stop);
        }
    }
}

pub fn resume_all() {
    let players = PLAYERS.lock().unwrap();
    for player in players.values() {
        if let Some(child) = &player.child {
            signal_child(child, ChildSignal::Continue);
        }
    }
}

pub fn stop_all() {
    let mut players = PLAYERS.lock().unwrap();
    let players_to_stop = std::mem::take(&mut *players);
    drop(players);

    for mut player in players_to_stop.into_values() {
        if let Some(mut child) = player.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        if let Some(path) = player.media_file {
            let _ = fs::remove_file(path);
        }
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
            spawn_tone(note, duration_ms, volume);
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
            let _ = ensure_player_media_file(player_id);
            set_state(player_id, STATE_PREFETCHED, jvm);
            Ok(None)
        }
        ("start", "()V") => {
            let state = start_player(player_id);
            set_state(player_id, state, jvm);
            Ok(None)
        }
        ("stop", "()V") => {
            stop_player(player_id);
            set_state(player_id, STATE_PREFETCHED, jvm);
            Ok(None)
        }
        ("deallocate", "()V") => {
            stop_player(player_id);
            set_state(player_id, STATE_REALIZED, jvm);
            Ok(None)
        }
        ("close", "()V") => {
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

        let data = locator
            .as_ref()
            .and_then(|path| state.resources.get(path))
            .cloned()
            .unwrap_or_default();

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
            media_file: None,
            child: None,
            loop_count: 1,
            volume: 100,
            state: STATE_UNREALIZED,
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

fn start_player(player_id: usize) -> i32 {
    stop_player(player_id);

    let mut players = PLAYERS.lock().unwrap();
    let Some(player) = players.get_mut(&player_id) else {
        return STATE_CLOSED;
    };

    let Some(media_file) = ensure_media_file(player_id, player) else {
        player.state = STATE_PREFETCHED;
        return player.state;
    };

    player.child = spawn_decoder(
        &media_file,
        &player.content_type,
        player.loop_count,
        player.volume,
    );
    player.state = if player.child.is_some() {
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

    if let Some(mut child) = player.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn close_player(player_id: usize) {
    stop_player(player_id);
    if let Some(player) = PLAYERS.lock().unwrap().remove(&player_id) {
        if let Some(path) = player.media_file {
            let _ = fs::remove_file(path);
        }
    }
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
    let state_value = {
        let mut players = PLAYERS.lock().unwrap();
        let Some(player) = players.get_mut(&player_id) else {
            return STATE_CLOSED;
        };

        if reap_finished_child(player) {
            player.state = STATE_PREFETCHED;
            state_to_sync = Some(player.state);
        }

        player.state
    };

    if let Some(state_value) = state_to_sync {
        set_heap_state(player_id, state_value, jvm);
    }

    state_value
}

fn reap_finished_child(player: &mut PlayerRuntime) -> bool {
    let Some(child) = player.child.as_mut() else {
        return false;
    };

    match child.try_wait() {
        Ok(Some(_status)) => {
            if let Some(mut child) = player.child.take() {
                let _ = child.wait();
            }
            true
        }
        Ok(None) => false,
        Err(err) => {
            eprintln!("[Media] Failed to query decoder process: {}", err);
            if let Some(mut child) = player.child.take() {
                let _ = child.wait();
            }
            true
        }
    }
}

fn ensure_player_media_file(player_id: usize) -> Option<PathBuf> {
    let mut players = PLAYERS.lock().unwrap();
    let player = players.get_mut(&player_id)?;
    ensure_media_file(player_id, player)
}

fn ensure_media_file(player_id: usize, player: &mut PlayerRuntime) -> Option<PathBuf> {
    if player.media_data.is_empty() {
        if let Some(locator) = &player.locator {
            eprintln!("[Media] No media bytes available for {}", locator);
        }
        return None;
    }

    if let Some(path) = &player.media_file {
        return Some(path.clone());
    }

    let extension = media_extension(
        player.locator.as_deref(),
        &player.content_type,
        &player.media_data,
    );
    let mut path = env::temp_dir();
    path.push(format!(
        "j2me_emulator_audio_{}_{}.{}",
        std::process::id(),
        player_id,
        extension
    ));

    if fs::write(&path, &player.media_data).is_err() {
        eprintln!(
            "[Media] Failed to write media temp file: {}",
            path.display()
        );
        return None;
    }

    player.media_file = Some(path.clone());
    Some(path)
}

fn spawn_decoder(
    media_file: &Path,
    content_type: &str,
    loop_count: i32,
    volume: i32,
) -> Option<Child> {
    let file = media_file.to_string_lossy().to_string();
    let mut attempts = Vec::new();
    let is_midi = is_midi_media(content_type, media_file);
    let is_wav = is_wav_media(content_type, media_file);

    if is_midi {
        if let Some(soundfont) = find_soundfont() {
            attempts.push((
                vec!["fluidsynth", "/usr/bin/fluidsynth", "/usr/sbin/fluidsynth"],
                vec!["-q".to_string(), "-i".to_string(), soundfont, file.clone()],
            ));
        }

        attempts.push((
            vec!["aplaymidi", "/usr/bin/aplaymidi", "/usr/sbin/aplaymidi"],
            vec![file.clone()],
        ));
    }

    let mut ffplay_args = vec![
        "-nodisp".to_string(),
        "-autoexit".to_string(),
        "-loglevel".to_string(),
        "quiet".to_string(),
        "-volume".to_string(),
        volume.clamp(0, 100).to_string(),
    ];
    if loop_count == -1 {
        ffplay_args.push("-loop".to_string());
        ffplay_args.push("0".to_string());
    } else if loop_count > 1 {
        ffplay_args.push("-loop".to_string());
        ffplay_args.push(loop_count.to_string());
    }
    ffplay_args.push(file.clone());
    attempts.push((
        vec!["ffplay", "/usr/bin/ffplay", "/usr/sbin/ffplay"],
        ffplay_args,
    ));

    if is_wav {
        attempts.push((
            vec!["paplay", "/usr/bin/paplay", "/usr/sbin/paplay"],
            vec![file.clone()],
        ));
        attempts.push((
            vec!["pw-play", "/usr/bin/pw-play", "/usr/sbin/pw-play"],
            vec![file.clone()],
        ));
        attempts.push((
            vec!["aplay", "/usr/bin/aplay", "/usr/sbin/aplay"],
            vec!["-q".to_string(), file.clone()],
        ));
    }

    for (commands, args) in attempts {
        let Some(command) = find_command(&commands) else {
            continue;
        };

        match Command::new(&command)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => return Some(child),
            Err(err) => eprintln!("[Media] Failed to start {}: {}", command, err),
        }
    }

    eprintln!(
        "[Media] No usable decoder found for {} ({})",
        media_file.display(),
        content_type
    );
    None
}

fn spawn_tone(note: i32, duration_ms: i32, volume: i32) {
    let Some(ffplay) = find_command(&["ffplay", "/usr/bin/ffplay", "/usr/sbin/ffplay"]) else {
        return;
    };

    let frequency = 8.175_798_915_6_f64 * 2_f64.powf(note as f64 / 12.0);
    let duration = (duration_ms.max(1) as f64 / 1000.0).max(0.001);
    let volume = volume.clamp(0, 100) as f64 / 100.0;

    let _ = Command::new(ffplay)
        .args([
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "quiet",
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency={:.3}:duration={:.3}", frequency, duration),
            "-af",
            &format!("volume={:.2}", volume),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn get_int_arg(args: &[JvmStackValue], index: usize, name: &str) -> Result<i32, String> {
    match args.get(index) {
        Some(JvmStackValue::Int(value)) => Ok(*value),
        value => Err(format!("{}: expected int, found {:?}", name, value)),
    }
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

fn media_extension(locator: Option<&str>, content_type: &str, data: &[u8]) -> &'static str {
    if data.starts_with(b"MThd") {
        return "mid";
    }
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WAVE") {
        return "wav";
    }

    if let Some(ext) = locator
        .and_then(|path| Path::new(path).extension())
        .and_then(|ext| ext.to_str())
    {
        if matches!(
            ext.to_ascii_lowercase().as_str(),
            "mid" | "midi" | "wav" | "mp3" | "amr" | "ogg"
        ) {
            return match ext.to_ascii_lowercase().as_str() {
                "midi" => "mid",
                "wav" => "wav",
                "mp3" => "mp3",
                "amr" => "amr",
                "ogg" => "ogg",
                _ => "mid",
            };
        }
    }

    let normalized = content_type.to_ascii_lowercase();
    if normalized.contains("midi") || normalized.contains("mid") {
        "mid"
    } else if normalized.contains("wav") || normalized.contains("wave") {
        "wav"
    } else if normalized.contains("mpeg") || normalized.contains("mp3") {
        "mp3"
    } else if normalized.contains("amr") {
        "amr"
    } else if normalized.contains("ogg") {
        "ogg"
    } else {
        "bin"
    }
}

fn is_midi_media(content_type: &str, path: &Path) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    content_type.contains("midi")
        || content_type.contains("audio/mid")
        || path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
            .unwrap_or(false)
}

fn is_wav_media(content_type: &str, path: &Path) -> bool {
    let content_type = content_type.to_ascii_lowercase();
    content_type.contains("wav")
        || content_type.contains("wave")
        || path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("wav"))
            .unwrap_or(false)
}

fn find_command(candidates: &[&str]) -> Option<String> {
    for candidate in candidates {
        let path = Path::new(candidate);
        if path.is_absolute() && path.exists() {
            return Some((*candidate).to_string());
        }
    }

    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        for candidate in candidates {
            if candidate.contains('/') {
                continue;
            }

            let full_path = dir.join(candidate);
            if full_path.exists() {
                return Some(full_path.to_string_lossy().to_string());
            }
        }
    }

    None
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

enum ChildSignal {
    Stop,
    Continue,
}

#[cfg(unix)]
fn signal_child(child: &Child, signal: ChildSignal) {
    use std::os::raw::c_int;

    const SIGSTOP: c_int = 19;
    const SIGCONT: c_int = 18;

    unsafe extern "C" {
        fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    let signal = match signal {
        ChildSignal::Stop => SIGSTOP,
        ChildSignal::Continue => SIGCONT,
    };

    unsafe {
        let _ = kill(child.id() as c_int, signal);
    }
}

#[cfg(not(unix))]
fn signal_child(_child: &Child, _signal: ChildSignal) {}
