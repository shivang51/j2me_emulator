use crate::jvm::jvm_core::{JvmObject, JvmStackValue};

const DEFAULT_WIDTH: i32 = 128;
const DEFAULT_HEIGHT: i32 = 128;

pub const CLASS_NAME: &str = "javax/microedition/lcdui/game/GameCanvas";

pub fn handle_virtual_method(
    object: &mut JvmObject,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
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
        ("setFullScreenMode", "(Z)V") => Ok(None),
        _ => Err(format!(
            "Unsupported GameCanvas instance method: {}{}",
            method_name, descriptor
        )),
    }
}
