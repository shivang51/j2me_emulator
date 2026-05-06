use crate::jvm::jvm_core::JvmStackValue;

pub const CLASS_NAME: &str = "javax/microedition/midlet/MIDlet";

pub fn handle_virtual_method(
    method_name: &str,
    descriptor: &str,
    _args: &[JvmStackValue],
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("<init>", "()V") => Ok(None),
        _ => Err(format!(
            "Unsupported MIDlet method: {}{}",
            method_name, descriptor
        )),
    }
}
