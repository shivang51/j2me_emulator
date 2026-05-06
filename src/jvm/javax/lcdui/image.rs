use std::collections::HashMap;

use crate::jvm::jvm_core::{HeapObject, JVM, JvmObject, JvmStackValue};

pub const CLASS_NAME: &str = "javax/microedition/lcdui/Image";

pub fn handle_static_method(
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &mut JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("createImage", "(Ljava/lang/String;)Ljavax/microedition/lcdui/Image;") => {
            create_image(args, jvm)
        }
        _ => Err(format!(
            "Unsupported Image static method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_virtual_method(
    objectref: JvmStackValue,
    method_name: &str,
    descriptor: &str,
    jvm: &mut JVM,
) -> Result<Option<JvmStackValue>, String> {
    let image = get_image_object(objectref, jvm)?;

    match (method_name, descriptor) {
        ("getWidth", "()I") => get_int_field(image, &["width:I", "width:Int"]).map(Some),
        ("getHeight", "()I") => get_int_field(image, &["height:I", "height:Int"]).map(Some),
        _ => Err(format!(
            "Unsupported Image instance method: {}{}",
            method_name, descriptor
        )),
    }
}

fn create_image(args: &[JvmStackValue], jvm: &mut JVM) -> Result<Option<JvmStackValue>, String> {
    let path = match args.get(0) {
        Some(JvmStackValue::String(path)) => path,
        Some(value) => {
            return Err(format!(
                "Image.createImage: expected String path, found {:?}",
                value
            ));
        }
        None => return Err("Image.createImage: missing path argument".into()),
    };

    let resource_name = normalize_resource_path(path);
    let (width, height) = jvm
        .resources
        .get(resource_name)
        .and_then(|resource| png_dimensions(resource))
        .unwrap_or((0, 0));

    let mut fields = HashMap::new();
    fields.insert(
        "path:Ljava/lang/String;".to_string(),
        JvmStackValue::String(path.clone()),
    );
    fields.insert("width:I".to_string(), JvmStackValue::Int(width));
    fields.insert("height:I".to_string(), JvmStackValue::Int(height));

    let image_obj = HeapObject::Instance(JvmObject {
        class_name: CLASS_NAME.to_string(),
        fields,
    });
    jvm.heap.push(image_obj);

    Ok(Some(JvmStackValue::ObjectRef((jvm.heap.len() - 1) as u32)))
}

fn get_image_object(objectref: JvmStackValue, jvm: &mut JVM) -> Result<&mut HeapObject, String> {
    let image_id = match objectref {
        JvmStackValue::ObjectRef(id) => id,
        JvmStackValue::Null => return Err("Image: NullPointerException".into()),
        value => {
            return Err(format!(
                "Image: expected object reference, found {:?}",
                value
            ));
        }
    };

    jvm.heap
        .get_mut(image_id as usize)
        .ok_or_else(|| format!("Image: invalid heap reference: {}", image_id))
}

fn get_int_field(image: &mut HeapObject, keys: &[&str]) -> Result<JvmStackValue, String> {
    let HeapObject::Instance(obj) = image else {
        return Err("Image: expected instance object".into());
    };

    for key in keys {
        if let Some(JvmStackValue::Int(value)) = obj.fields.get(*key) {
            return Ok(JvmStackValue::Int(*value));
        }
    }

    Err(format!("Image instance missing int field: {}", keys[0]))
}

fn normalize_resource_path(path: &str) -> &str {
    path.strip_prefix('/').unwrap_or(path)
}

fn png_dimensions(bytes: &[u8]) -> Option<(i32, i32)> {
    let signature = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

    if bytes.len() < 24 || bytes[..8] != signature {
        return None;
    }

    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);

    Some((width as i32, height as i32))
}
