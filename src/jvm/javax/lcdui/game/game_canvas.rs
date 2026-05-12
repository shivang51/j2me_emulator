use std::collections::HashMap;

use crate::{
    app::INPUT_STATE,
    jvm::{
        javax::lcdui::{display, graphics},
        jvm_core::{HeapObject, JVM, JvmObject, JvmStackValue},
    },
};

const DEFAULT_WIDTH: i32 = 128;
const DEFAULT_HEIGHT: i32 = 128;

pub const CLASS_NAME: &str = "javax/microedition/lcdui/game/GameCanvas";

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
    static mut GRAPHICS_HANDLE: i32 = -1;
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

        if unsafe { GRAPHICS_HANDLE } == -1 {
            let fields = HashMap::new();
            let mut state = jvm.state.lock();
            let hwd = JVM::allocate_internal(&mut state, graphics::CLASS_NAME.to_string(), fields);
            unsafe { GRAPHICS_HANDLE = hwd as i32 };
        }

        graphics_handle = unsafe { GRAPHICS_HANDLE } as u32;
    }

    return JVM::execute_method(
        displayable_ref,
        &class_name,
        "paint",
        "(Ljavax/microedition/lcdui/Graphics;)V",
        &[JvmStackValue::ObjectRef(graphics_handle)],
        jvm,
        &mut Vec::new(),
    );
}

pub fn handle_virtual_method(
    object: &mut JvmObject,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
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
            return Ok(Some(JvmStackValue::Int(state)));
        }
        ("setFullScreenMode", "(Z)V") => Ok(None),
        ("flushGraphics", "()V") => todo!("GameCanvas.flushGraphics"),
        ("flushGraphics", "(IIII)V") => todo!("GameCanvas.flushGraphics(IIII)"),
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
        ("repaint", "(IIII)V") => todo!("GameCanvas.repaint(IIII)"),
        ("serviceRepaints", "()V") => todo!("GameCanvas.serviceRepaints"),
        _ => Err(format!(
            "Unsupported GameCanvas instance method: {}{}",
            method_name, descriptor
        )),
    }
}
