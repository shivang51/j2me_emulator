use crate::jvm::jvm_core::JvmStackValue;

const DEFAULT_WIDTH: i32 = 128;
const DEFAULT_HEIGHT: i32 = 128;

pub fn handle_virtual_method(
    method_name: &str,
    descriptor: &str,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("getWidth", "()I") => Ok(Some(JvmStackValue::Int(DEFAULT_WIDTH))),
        ("getHeight", "()I") => Ok(Some(JvmStackValue::Int(DEFAULT_HEIGHT))),
        ("setFullScreenMode", "(Z)V") => Ok(None),
        _ => Err(format!(
            "Unsupported GameCanvas instance method: {}{}",
            method_name, descriptor
        )),
    }
}
