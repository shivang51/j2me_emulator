use std::{
    collections::HashMap,
    sync::atomic::{AtomicI32, Ordering},
};

use crate::{
    input::{game_action_for_keycode, MidpKeyCode, INPUT_STATE},
    jvm::{
        javax::lcdui::{display, graphics, image},
        jvm_core::{HeapObject, JvmStackValue, JVM},
    },
};

pub const DEFAULT_WIDTH: i32 = 364;
pub const DEFAULT_HEIGHT: i32 = 364;

static CANVAS_WIDTH: AtomicI32 = AtomicI32::new(DEFAULT_WIDTH);
static CANVAS_HEIGHT: AtomicI32 = AtomicI32::new(DEFAULT_HEIGHT);

pub const CLASS_NAME: &str = "javax/microedition/lcdui/game/GameCanvas";
const SCREEN_GRAPHICS_FIELD: &str =
    "javax/microedition/lcdui/game/GameCanvas.screenGraphics:Ljavax/microedition/lcdui/Graphics;";
const BUFFER_IMAGE_FIELD: &str =
    "javax/microedition/lcdui/game/GameCanvas.bufferImage:Ljavax/microedition/lcdui/Image;";
const BUFFER_GRAPHICS_FIELD: &str =
    "javax/microedition/lcdui/game/GameCanvas.bufferGraphics:Ljavax/microedition/lcdui/Graphics;";

pub fn canvas_size() -> (i32, i32) {
    (
        CANVAS_WIDTH.load(Ordering::Relaxed),
        CANVAS_HEIGHT.load(Ordering::Relaxed),
    )
}

pub fn configure_canvas_size_from_path(path: Option<&str>) -> (i32, i32) {
    let size = path
        .and_then(parse_canvas_size)
        .unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));
    set_canvas_size(size.0, size.1)
}

pub fn set_canvas_size(width: i32, height: i32) -> (i32, i32) {
    let width = width.clamp(1, 2048);
    let height = height.clamp(1, 2048);
    CANVAS_WIDTH.store(width, Ordering::Relaxed);
    CANVAS_HEIGHT.store(height, Ordering::Relaxed);
    (width, height)
}

fn parse_canvas_size(path: &str) -> Option<(i32, i32)> {
    let bytes = path.as_bytes();

    for x_index in 0..bytes.len() {
        if !matches!(bytes[x_index], b'x' | b'X') {
            continue;
        }

        let mut start = x_index;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }

        let mut end = x_index + 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }

        if start == x_index || end == x_index + 1 {
            continue;
        }

        let width = std::str::from_utf8(&bytes[start..x_index])
            .ok()?
            .parse::<i32>()
            .ok()?;
        let height = std::str::from_utf8(&bytes[x_index + 1..end])
            .ok()?
            .parse::<i32>()
            .ok()?;

        if width > 0 && height > 0 {
            return Some((width, height));
        }
    }

    None
}

pub fn paint(jvm: &JVM) -> Result<(), String> {
    let displayable_ref;
    let class_name;
    let graphics_handle;

    {
        let disp = display::get_display_safe(jvm)?;

        let displayable_res = display::get_displayable_obj_safe(disp, jvm)?;
        displayable_ref = displayable_res.ok_or_else(|| "No displayable object set".to_string())?;

        class_name = if let JvmStackValue::ObjectRef(id) = displayable_ref {
            let state = jvm.state.lock();
            if let Some(HeapObject::Instance(inst)) = state.heap.get(id as usize) {
                inst.class_name.clone()
            } else {
                return Err("Displayable is not an instance".into());
            }
        } else {
            return Err("Displayable is not an object ref".into());
        };

        graphics_handle = get_screen_graphics_handle(jvm);
    }

    let start = std::time::Instant::now();
    let res = JVM::execute_method(
        displayable_ref,
        &class_name,
        "paint",
        "(Ljavax/microedition/lcdui/Graphics;)V",
        &[JvmStackValue::ObjectRef(graphics_handle)],
        jvm,
        &mut Vec::new(),
    );
    let elapsed = start.elapsed();
    if elapsed > std::time::Duration::from_millis(100) {
        println!(
            "[WARNING] game_canvas::paint for {} took {:?}",
            class_name, elapsed
        );
    }
    res
}

fn get_screen_graphics_handle(jvm: &JVM) -> u32 {
    let mut state = jvm.state.lock();

    if let Some(JvmStackValue::ObjectRef(handle)) = state.static_fields.get(SCREEN_GRAPHICS_FIELD) {
        if matches!(
            state.heap.get(*handle as usize),
            Some(HeapObject::Instance(inst))
                if inst.class_name == graphics::CLASS_NAME
                    && !inst.fields.contains_key("targetImageId:I")
        ) {
            return *handle;
        }
    }

    let graphics_ref =
        JVM::allocate_internal(&mut state, graphics::CLASS_NAME.to_string(), HashMap::new());
    state.static_fields.insert(
        SCREEN_GRAPHICS_FIELD.to_string(),
        JvmStackValue::ObjectRef(graphics_ref),
    );
    graphics_ref
}

fn ensure_object_instance(jvm: &JVM, object_id: u32) -> Result<(), String> {
    let state = jvm.state.lock();
    match state.heap.get(object_id as usize) {
        Some(HeapObject::Instance(_)) => Ok(()),
        _ => Err(format!(
            "GameCanvas method call on non-instance object ref {}",
            object_id
        )),
    }
}

fn object_field_ref(jvm: &JVM, object_id: u32, field_name: &str) -> Option<u32> {
    let state = jvm.state.lock();
    let Some(HeapObject::Instance(object)) = state.heap.get(object_id as usize) else {
        return None;
    };

    match object.fields.get(field_name) {
        Some(JvmStackValue::ObjectRef(id)) => Some(*id),
        _ => None,
    }
}

fn set_object_field(
    jvm: &JVM,
    object_id: u32,
    field_name: &str,
    value: JvmStackValue,
) -> Result<(), String> {
    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(object)) = state.heap.get_mut(object_id as usize) else {
        return Err(format!(
            "GameCanvas method call on non-instance object ref {}",
            object_id
        ));
    };

    object.fields.insert(field_name.to_string(), value);
    Ok(())
}

fn set_object_field_with_state(
    state: &mut crate::jvm::jvm_core::JvmState,
    object_id: u32,
    field_name: &str,
    value: JvmStackValue,
) -> Result<(), String> {
    let Some(HeapObject::Instance(object)) = state.heap.get_mut(object_id as usize) else {
        return Err(format!(
            "GameCanvas method call on non-instance object ref {}",
            object_id
        ));
    };

    object.fields.insert(field_name.to_string(), value);
    Ok(())
}

fn allocate_object_field_ref(
    jvm: &JVM,
    object_id: u32,
    field_name: &str,
    class_name: &str,
    fields: HashMap<String, JvmStackValue>,
) -> Result<u32, String> {
    let mut state = jvm.state.lock();
    let handle = JVM::allocate_internal(&mut state, class_name.to_string(), fields);
    if let Err(err) = set_object_field_with_state(
        &mut state,
        object_id,
        field_name,
        JvmStackValue::ObjectRef(handle),
    ) {
        return Err(err);
    }
    Ok(handle)
}

fn heap_image_matches_canvas(jvm: &JVM, object_id: u32, width: i32, height: i32) -> bool {
    let state = jvm.state.lock();
    let Some(HeapObject::Instance(inst)) = state.heap.get(object_id as usize) else {
        return false;
    };

    inst.class_name == image::CLASS_NAME
        && matches!(inst.fields.get("width:I"), Some(JvmStackValue::Int(w)) if *w == width)
        && matches!(inst.fields.get("height:I"), Some(JvmStackValue::Int(h)) if *h == height)
}

fn heap_graphics_targets_image(jvm: &JVM, graphics_id: u32, image_id: u32) -> bool {
    let state = jvm.state.lock();
    let Some(HeapObject::Instance(inst)) = state.heap.get(graphics_id as usize) else {
        return false;
    };

    inst.class_name == graphics::CLASS_NAME
        && matches!(
            inst.fields.get("targetImageId:I"),
            Some(JvmStackValue::Int(target_id)) if *target_id == image_id as i32
        )
}

fn graphics_target_image_id(graphics_ref: &JvmStackValue, jvm: &JVM) -> Option<u32> {
    let JvmStackValue::ObjectRef(graphics_id) = graphics_ref else {
        return None;
    };

    let state = jvm.state.lock();
    let HeapObject::Instance(inst) = state.heap.get(*graphics_id as usize)? else {
        return None;
    };

    match inst.fields.get("targetImageId:I") {
        Some(JvmStackValue::Int(image_id)) => Some(*image_id as u32),
        _ => None,
    }
}

fn ensure_buffer_image(object_id: u32, jvm: &JVM) -> Result<u32, String> {
    let (width, height) = canvas_size();

    if let Some(image_handle) = object_field_ref(jvm, object_id, BUFFER_IMAGE_FIELD) {
        if heap_image_matches_canvas(jvm, image_handle, width, height) {
            return Ok(image_handle);
        }
    }

    let mut fields = HashMap::new();
    fields.insert("width:I".to_string(), JvmStackValue::Int(width));
    fields.insert("height:I".to_string(), JvmStackValue::Int(height));

    allocate_object_field_ref(
        jvm,
        object_id,
        BUFFER_IMAGE_FIELD,
        image::CLASS_NAME,
        fields,
    )
}

fn ensure_buffer_graphics(object_id: u32, jvm: &JVM) -> Result<u32, String> {
    let image_handle = ensure_buffer_image(object_id, jvm)?;

    if let Some(graphics_handle) = object_field_ref(jvm, object_id, BUFFER_GRAPHICS_FIELD) {
        if heap_graphics_targets_image(jvm, graphics_handle, image_handle) {
            return Ok(graphics_handle);
        }
    }

    let mut fields = HashMap::new();
    fields.insert(
        "targetImageId:I".to_string(),
        JvmStackValue::Int(image_handle as i32),
    );

    allocate_object_field_ref(
        jvm,
        object_id,
        BUFFER_GRAPHICS_FIELD,
        graphics::CLASS_NAME,
        fields,
    )
}

fn paint_buffer_to_graphics(
    object_id: u32,
    graphics_ref: &JvmStackValue,
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match graphics_ref {
        JvmStackValue::ObjectRef(_) => {}
        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
        value => {
            return Err(format!(
                "GameCanvas.paint: expected Graphics object, found {:?}",
                value
            ));
        }
    }

    let image_handle = ensure_buffer_image(object_id, jvm)?;
    if graphics_target_image_id(graphics_ref, jvm) == Some(image_handle) {
        return Ok(None);
    }

    graphics::handle_virtual_method(
        graphics_ref,
        "drawImage",
        "(Ljavax/microedition/lcdui/Image;III)V",
        &[
            JvmStackValue::ObjectRef(image_handle),
            JvmStackValue::Int(0),
            JvmStackValue::Int(0),
            JvmStackValue::Int(0),
        ],
        jvm,
    )?;
    Ok(None)
}

fn flush_buffer(object_id: u32, jvm: &JVM) -> Result<Option<JvmStackValue>, String> {
    let graphics_ref = JvmStackValue::ObjectRef(get_screen_graphics_handle(jvm));
    paint_buffer_to_graphics(object_id, &graphics_ref, jvm)
}

fn flush_buffer_region(
    object_id: u32,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let x = get_int_arg(args, 0, "GameCanvas.flushGraphics")?;
    let y = get_int_arg(args, 1, "GameCanvas.flushGraphics")?;
    let width = get_int_arg(args, 2, "GameCanvas.flushGraphics")?;
    let height = get_int_arg(args, 3, "GameCanvas.flushGraphics")?;
    let image_handle = ensure_buffer_image(object_id, jvm)?;
    let graphics_ref = JvmStackValue::ObjectRef(get_screen_graphics_handle(jvm));

    graphics::handle_virtual_method(
        &graphics_ref,
        "drawRegion",
        "(Ljavax/microedition/lcdui/Image;IIIIIIII)V",
        &[
            JvmStackValue::ObjectRef(image_handle),
            JvmStackValue::Int(x),
            JvmStackValue::Int(y),
            JvmStackValue::Int(width),
            JvmStackValue::Int(height),
            JvmStackValue::Int(0),
            JvmStackValue::Int(x),
            JvmStackValue::Int(y),
            JvmStackValue::Int(0),
        ],
        jvm,
    )?;
    Ok(None)
}

fn get_int_arg(args: &[JvmStackValue], index: usize, method_name: &str) -> Result<i32, String> {
    match args.get(index) {
        Some(JvmStackValue::Int(value)) => Ok(*value),
        Some(value) => Err(format!(
            "{}: expected int argument at index {}, found {:?}",
            method_name, index, value
        )),
        None => Err(format!(
            "{}: missing int argument at index {}",
            method_name, index
        )),
    }
}

pub fn handle_virtual_method(
    object_id: u32,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    ensure_object_instance(jvm, object_id)?;

    match (method_name, descriptor) {
        ("<init>", "()V") => {
            set_object_field(jvm, object_id, "suppressKeyEvents:Z", JvmStackValue::Int(0))?;
            Ok(None)
        }
        ("<init>", "(Z)V") => {
            if let Some(JvmStackValue::Int(suppress_key_evts)) = args.get(0) {
                set_object_field(
                    jvm,
                    object_id,
                    "suppressKeyEvents:Z",
                    JvmStackValue::Int(*suppress_key_evts),
                )?;
                return Ok(None);
            } else {
                Err("GameCanvas.<init>: expected boolean argument".into())
            }
        }
        ("getWidth", "()I") => Ok(Some(JvmStackValue::Int(canvas_size().0))),
        ("getHeight", "()I") => Ok(Some(JvmStackValue::Int(canvas_size().1))),
        ("getGameAction", "(I)I") => {
            let Some(JvmStackValue::Int(keycode)) = args.get(0) else {
                return Err("Canvas.getGameAction: expected int keycode".into());
            };

            Ok(Some(JvmStackValue::Int(game_action_for_keycode(
                MidpKeyCode::from_raw(*keycode),
            ))))
        }
        ("getKeyStates", "()I") => {
            let input_state = INPUT_STATE.lock();
            Ok(Some(JvmStackValue::Int(input_state.key_state_mask())))
        }
        ("hasPointerEvents", "()Z") => Ok(Some(JvmStackValue::Int(1))),
        ("hasPointerMotionEvents", "()Z") => Ok(Some(JvmStackValue::Int(1))),
        ("hasRepeatEvents", "()Z") => Ok(Some(JvmStackValue::Int(0))),
        ("isDoubleBuffered", "()Z") => Ok(Some(JvmStackValue::Int(1))),
        ("keyPressed", "(I)V") => Ok(None),
        ("keyReleased", "(I)V") => Ok(None),
        ("keyRepeated", "(I)V") => Ok(None),
        ("pointerPressed", "(II)V") => Ok(None),
        ("pointerDragged", "(II)V") => Ok(None),
        ("pointerReleased", "(II)V") => Ok(None),
        ("showNotify", "()V") => Ok(None),
        ("hideNotify", "()V") => Ok(None),
        ("setFullScreenMode", "(Z)V") => Ok(None),
        ("paint", "(Ljavax/microedition/lcdui/Graphics;)V") => {
            let graphics_ref = args
                .get(0)
                .ok_or_else(|| "GameCanvas.paint: missing Graphics argument".to_string())?;
            paint_buffer_to_graphics(object_id, graphics_ref, jvm)
        }
        ("flushGraphics", "()V") => flush_buffer(object_id, jvm),
        ("flushGraphics", "(IIII)V") => flush_buffer_region(object_id, args, jvm),
        ("getGraphics", "()Ljavax/microedition/lcdui/Graphics;") => {
            let graphics_handle = ensure_buffer_graphics(object_id, jvm)?;
            Ok(Some(JvmStackValue::ObjectRef(graphics_handle)))
        }
        ("repaint", "()V") => {
            // let res = paint(jvm);
            // if let Err(e) = res {
            //     eprintln!("GameCanvas.repaint() failed: {}", e);
            // }
            Ok(None)
        }
        ("repaint", "(IIII)V") => Ok(None),
        ("serviceRepaints", "()V") => Ok(None),
        _ => Err(format!(
            "Unsupported GameCanvas instance method: {}{}",
            method_name, descriptor
        )),
    }
}
