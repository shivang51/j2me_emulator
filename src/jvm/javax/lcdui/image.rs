use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crate::jvm::jvm_core::{HeapObject, JVM, JvmObject, JvmStackValue};

pub const CLASS_NAME: &str = "javax/microedition/lcdui/Image";

#[derive(Debug, Clone)]
pub struct ImageBufferData {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

pub type SharedImageBuffer = Arc<Mutex<ImageBufferData>>;

pub static IMAGE_CACHE: LazyLock<Mutex<HashMap<usize, SharedImageBuffer>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn handle_static_method(
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("createImage", "(Ljava/lang/String;)Ljavax/microedition/lcdui/Image;") => {
            create_image(args, jvm)
        }
        ("createImage", "(II)Ljavax/microedition/lcdui/Image;") => create_image_ii(args, jvm),
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
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
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

    let mut state = jvm.state.lock();
    let image = state
        .heap
        .get(image_id as usize)
        .ok_or_else(|| format!("Image: invalid heap reference: {}", image_id))?;

    match (method_name, descriptor) {
        ("getWidth", "()I") => get_int_field(image, &["width:I", "width:Int"]).map(Some),
        ("getHeight", "()I") => get_int_field(image, &["height:I", "height:Int"]).map(Some),
        ("getGraphics", "()Ljavax/microedition/lcdui/Graphics;") => {
            let mut fields = HashMap::new();
            fields.insert(
                "targetImageId:I".to_string(),
                JvmStackValue::Int(image_id as i32),
            );

            let handle = JVM::allocate_internal(
                &mut state,
                "javax/microedition/lcdui/Graphics".to_string(),
                fields,
            );
            Ok(Some(JvmStackValue::ObjectRef(handle)))
        }
        _ => Err(format!(
            "Unsupported Image instance method: {}{}",
            method_name, descriptor
        )),
    }
}

fn create_image(args: &[JvmStackValue], jvm: &JVM) -> Result<Option<JvmStackValue>, String> {
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

    let (width, height) = {
        let state = jvm.state.lock();
        state
            .resources
            .get(resource_name)
            .and_then(|resource| png_dimensions(resource))
            .unwrap_or((0, 0))
    };

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

    let mut state = jvm.state.lock();
    state.heap.push(image_obj);

    Ok(Some(JvmStackValue::ObjectRef(
        (state.heap.len() - 1) as u32,
    )))
}

pub fn get_or_create_buffer(img_ref: &JvmStackValue, jvm: &JVM) -> Option<SharedImageBuffer> {
    let img_id = match img_ref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return None,
    };

    let mut cache = IMAGE_CACHE.lock().unwrap();
    if let Some(buffer) = cache.get(&img_id) {
        return Some(Arc::clone(buffer));
    }

    let buffer = {
        let state = jvm.state.lock();
        let Some(HeapObject::Instance(obj)) = state.heap.get(img_id) else {
            return None;
        };

        let width = match obj.fields.get("width:I") {
            Some(JvmStackValue::Int(v)) => *v,
            _ => return None,
        };
        let height = match obj.fields.get("height:I") {
            Some(JvmStackValue::Int(v)) => *v,
            _ => return None,
        };

        let mut pixels = vec![0; (width.max(0) * height.max(0) * 4) as usize];

        if let Some(JvmStackValue::String(path)) = obj.fields.get("path:Ljava/lang/String;") {
            let resource_name = path.strip_prefix('/').unwrap_or(path);
            if let Some(data) = state.resources.get(resource_name) {
                if let Ok(decoded) = image::load_from_memory(data) {
                    let rgba = decoded.to_rgba8();
                    let decoded_width = rgba.width() as i32;
                    let decoded_height = rgba.height() as i32;
                    pixels = rgba.into_raw();

                    let buffer = Arc::new(Mutex::new(ImageBufferData {
                        width: decoded_width,
                        height: decoded_height,
                        pixels,
                    }));
                    cache.insert(img_id, Arc::clone(&buffer));
                    return Some(buffer);
                }
            }
        }

        Arc::new(Mutex::new(ImageBufferData {
            width,
            height,
            pixels,
        }))
    };

    cache.insert(img_id, Arc::clone(&buffer));
    Some(buffer)
}

pub fn clone_image_buffer(img_ref: &JvmStackValue, jvm: &JVM) -> Option<ImageBufferData> {
    let buffer = get_or_create_buffer(img_ref, jvm)?;
    let buffer = buffer.lock().unwrap();
    Some(buffer.clone())
}

fn get_int_field(image: &HeapObject, keys: &[&str]) -> Result<JvmStackValue, String> {
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

fn create_image_ii(args: &[JvmStackValue], jvm: &JVM) -> Result<Option<JvmStackValue>, String> {
    let width = match args.get(0) {
        Some(JvmStackValue::Int(w)) => w,
        _ => return Err("Image.createImage(II): missing/invalid width argument".into()),
    };
    let height = match args.get(1) {
        Some(JvmStackValue::Int(h)) => h,
        _ => return Err("Image.createImage(II): missing/invalid height argument".into()),
    };

    let mut state = jvm.state.lock();
    let mut instance = JvmObject {
        class_name: CLASS_NAME.to_string(),
        fields: HashMap::new(),
    };

    instance.fields.insert(
        "id".to_string(),
        JvmStackValue::Int(state.heap.len() as i32),
    );

    instance
        .fields
        .insert("buff".to_string(), JvmStackValue::Vector(Vec::new()));

    instance
        .fields
        .insert("width:I".to_string(), JvmStackValue::Int(*width));

    instance
        .fields
        .insert("height:I".to_string(), JvmStackValue::Int(*height));

    let image_id = state.heap.len() as u32;
    state.heap.push(HeapObject::Instance(instance));

    Ok(Some(JvmStackValue::ObjectRef(image_id)))
}
