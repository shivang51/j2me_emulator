use crate::jvm::jvm_core::{JvmObject, JvmStackValue, JvmState, JVM};

const DEFAULT_WIDTH: i32 = 128;
const DEFAULT_HEIGHT: i32 = 128;

pub const CLASS_NAME: &str = "javax/microedition/lcdui/game/GameCanvas";

pub fn handle_virtual_method(
    object: &mut JvmObject,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    state: &mut JvmState,
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
        ("getKeyStates", "()I") => Ok(Some(JvmStackValue::Int(0))), // No keys pressed
        ("setFullScreenMode", "(Z)V") => Ok(None),
        ("flushGraphics", "()V") => Ok(None),
        ("flushGraphics", "(IIII)V") => Ok(None),
        ("getGraphics", "()Ljavax/microedition/lcdui/Graphics;") => {
            let graphics_handle = JVM::allocate_internal(
                state,
                crate::jvm::javax::lcdui::graphics::CLASS_NAME.to_string(),
                std::collections::HashMap::new(),
            );
            Ok(Some(JvmStackValue::ObjectRef(graphics_handle)))
        }
        ("repaint", "()V") => Ok(None),
        ("repaint", "(IIII)V") => Ok(None),
        ("serviceRepaints", "()V") => Ok(None),
        _ => Err(format!(
            "Unsupported GameCanvas instance method: {}{}",
            method_name, descriptor
        )),
    }
}
