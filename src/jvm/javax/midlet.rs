use crate::{jvm::jvm_core::JvmStackValue, services::jar_extractor::JarFileData};

pub const CLASS_NAME: &str = "javax/microedition/midlet/MIDlet";

pub fn handle_virtual_method(
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    loaded_jar: Option<&JarFileData>,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("<init>", "()V") => Ok(None),
        ("getAppProperty", "(Ljava/lang/String;)Ljava/lang/String;") => {
            let Some(JvmStackValue::String(key)) = args.first() else {
                return Err(format!("MIDlet.getAppProperty: invalid args {:?}", args));
            };

            let value = loaded_jar
                .and_then(|jar| jar.manifest.properties.get(key))
                .cloned()
                .map(JvmStackValue::String)
                .unwrap_or(JvmStackValue::Null);

            Ok(Some(value))
        }
        _ => Err(format!(
            "Unsupported MIDlet method: {}{}",
            method_name, descriptor
        )),
    }
}
