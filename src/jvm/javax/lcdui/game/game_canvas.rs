use std::collections::HashMap;

use crate::{
    app::INPUT_STATE,
    jvm::{
        javax::lcdui::{display, graphics},
        jvm_core::{HeapObject, JVM, JvmObject, JvmStackValue},
    },
};

pub const DEFAULT_WIDTH: i32 = 364;
pub const DEFAULT_HEIGHT: i32 = 364;

pub const CLASS_NAME: &str = "javax/microedition/lcdui/game/GameCanvas";
const SCREEN_GRAPHICS_FIELD: &str =
    "javax/microedition/lcdui/game/GameCanvas.screenGraphics:Ljavax/microedition/lcdui/Graphics;";

pub static DOWN_PRESSED: i32 = 64;
pub static FIRE_PRESSED: i32 = 256;
pub static GAME_A_PRESSED: i32 = 512;
pub static GAME_B_PRESSED: i32 = 1024;
pub static GAME_C_PRESSED: i32 = 2048;
pub static GAME_D_PRESSED: i32 = 4096;
pub static LEFT_PRESSED: i32 = 4;
pub static RIGHT_PRESSED: i32 = 32;
pub static UP_PRESSED: i32 = 2;

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
        println!("[WARNING] game_canvas::paint for {} took {:?}", class_name, elapsed);
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

pub fn handle_virtual_method(
    object: &mut JvmObject,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("<init>", "()V") => {
            object
                .fields
                .insert("suppressKeyEvents:Z".into(), JvmStackValue::Int(0));
            Ok(None)
        }
        ("<init>", "(Z)V") => {
            if let Some(JvmStackValue::Int(suppress_key_evts)) = args.get(0) {
                object.fields.insert(
                    "suppressKeyEvents:Z".into(),
                    JvmStackValue::Int(*suppress_key_evts),
                );
                return Ok(None);
            } else {
                Err("GameCanvas.<init>: expected boolean argument".into())
            }
        }
        ("getWidth", "()I") => Ok(Some(JvmStackValue::Int(DEFAULT_WIDTH))),
        ("getHeight", "()I") => Ok(Some(JvmStackValue::Int(DEFAULT_HEIGHT))),
        ("getKeyStates", "()I") => {
            let mut state = 0;
            let input_state = INPUT_STATE.lock();
            if input_state.space_pressed {
                state |= FIRE_PRESSED;
            }
            if input_state.up_pressed {
                state |= UP_PRESSED;
            }
            if input_state.down_pressed {
                state |= DOWN_PRESSED;
            }
            if input_state.left_pressed {
                state |= LEFT_PRESSED;
            }
            if input_state.right_pressed {
                state |= RIGHT_PRESSED;
            }
            if input_state.a_pressed {
                state |= GAME_A_PRESSED;
            }
            if input_state.b_pressed {
                state |= GAME_B_PRESSED;
            }
            if input_state.c_pressed {
                state |= GAME_C_PRESSED;
            }
            if input_state.d_pressed {
                state |= GAME_D_PRESSED;
            }
            return Ok(Some(JvmStackValue::Int(state)));
        }
        ("setFullScreenMode", "(Z)V") => Ok(None),
        ("flushGraphics", "()V") => Ok(None),
        ("flushGraphics", "(IIII)V") => Ok(None),
        ("getGraphics", "()Ljavax/microedition/lcdui/Graphics;") => {
            let mut state = jvm.state.lock();
            let graphics_handle = JVM::allocate_internal(
                &mut state,
                crate::jvm::javax::lcdui::graphics::CLASS_NAME.to_string(),
                std::collections::HashMap::new(),
            );
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
