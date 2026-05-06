use crate::jvm::jvm_core::JvmStackValue;

pub const CLASS_NAME: &str = "javax/microedition/lcdui/Graphics";

pub fn handle_virtual_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("setColor", "(I)V") => {
            let color = match args.get(0) {
                Some(JvmStackValue::Int(c)) => *c,
                _ => return Err("Graphics.setColor: expected int argument".into()),
            };

            eprintln!(
                "[+] Graphics.setColor called with color #{:06X}",
                color & 0xFFFFFF
            );

            return Ok(None);
        }
        ("fillRect", "(IIII)V") => Ok(None),
        ("drawRect", "(IIII)V") => Ok(None),
        ("drawLine", "(IIII)V") => Ok(None),
        ("drawImage", "(Ljavax/microedition/lcdui/Image;III)V") => Ok(None),
        ("drawRegion", "(Ljavax/microedition/lcdui/Image;IIIIIII)V") => Ok(None),
        ("drawString", "(Ljava/lang/String;III)V") => Ok(None),
        ("drawSubstring", "(Ljava/lang/String;IIIII)V") => Ok(None),
        ("setFont", "(Ljavax/microedition/lcdui/Font;)V") => Ok(None),
        ("setClip", "(IIII)V") => Ok(None),
        ("clipRect", "(IIII)V") => Ok(None),
        ("translate", "(II)V") => Ok(None),
        ("getClipX", "()I") => Ok(Some(JvmStackValue::Int(0))),
        ("getClipY", "()I") => Ok(Some(JvmStackValue::Int(0))),
        ("getClipWidth", "()I") => Ok(Some(JvmStackValue::Int(240))),
        ("getClipHeight", "()I") => Ok(Some(JvmStackValue::Int(320))),
        _ => Err(format!(
            "Unsupported Graphics instance method: {}{}",
            method_name, descriptor
        )),
    }
}
