use crate::jvm::jvm_core::JvmStackValue;

pub const CLASS_NAME: &str = "javax/microedition/lcdui/Graphics";

pub fn handle_virtual_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("setColor", "(I)V") => Ok(None),
        ("fillRect", "(IIII)V") => Ok(None),
        _ => Err(format!(
            "Unsupported Graphics instance method: {}{}",
            method_name, descriptor
        )),
    }
}
