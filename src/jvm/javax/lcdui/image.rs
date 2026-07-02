use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crate::jvm::jvm_core::{HeapObject, JvmObject, JvmStackValue, JVM};

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

pub fn clear_cache() {
    IMAGE_CACHE.lock().unwrap().clear();
}

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
        ("createImage", "([BII)Ljavax/microedition/lcdui/Image;") => {
            create_image_from_bytes(args, jvm)
        }
        ("createImage", "(Ljava/io/InputStream;)Ljavax/microedition/lcdui/Image;") => {
            create_image_from_input_stream(args, jvm)
        }
        ("createImage", "(Ljavax/microedition/lcdui/Image;)Ljavax/microedition/lcdui/Image;") => {
            create_image_from_image(args, jvm)
        }
        (
            "createImage",
            "(Ljavax/microedition/lcdui/Image;IIIII)Ljavax/microedition/lcdui/Image;",
        ) => create_image_from_region(args, jvm),
        ("createImage", "(II)Ljavax/microedition/lcdui/Image;") => create_image_ii(args, jvm),
        ("createRGBImage", "([IIIZ)Ljavax/microedition/lcdui/Image;") => {
            create_rgb_image(args, jvm)
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
    args: &[JvmStackValue],
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
        ("isMutable", "()Z") => Ok(Some(
            get_int_field(image, &["mutable:Z"]).unwrap_or(JvmStackValue::Int(0)),
        )),
        ("getRGB", "([IIIIIII)V") => {
            drop(state);
            get_rgb(JvmStackValue::ObjectRef(image_id), args, jvm)?;
            Ok(None)
        }
        ("getGraphics", "()Ljavax/microedition/lcdui/Graphics;") => {
            if !is_mutable_image(image) {
                return Err("java.lang.IllegalStateException: image is immutable".into());
            }

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

    allocate_image(jvm, width, height, false, fields, None).map(Some)
}

fn allocate_image(
    jvm: &JVM,
    width: i32,
    height: i32,
    mutable: bool,
    mut fields: HashMap<String, JvmStackValue>,
    pixels: Option<Vec<u8>>,
) -> Result<JvmStackValue, String> {
    if width < 0 || height < 0 {
        return Err(format!(
            "java.lang.IllegalArgumentException: invalid image size {}x{}",
            width, height
        ));
    }

    fields.insert("width:I".to_string(), JvmStackValue::Int(width));
    fields.insert("height:I".to_string(), JvmStackValue::Int(height));
    fields.insert(
        "mutable:Z".to_string(),
        JvmStackValue::Int(if mutable { 1 } else { 0 }),
    );

    let image_id = {
        let mut state = jvm.state.lock();
        let image_id = state.heap.len() as u32;
        fields.insert("id".to_string(), JvmStackValue::Int(image_id as i32));
        state.heap.push(HeapObject::Instance(JvmObject {
            class_name: CLASS_NAME.to_string(),
            fields,
        }));
        image_id
    };

    if let Some(pixels) = pixels {
        IMAGE_CACHE.lock().unwrap().insert(
            image_id as usize,
            Arc::new(Mutex::new(ImageBufferData {
                width,
                height,
                pixels,
            })),
        );
    }

    Ok(JvmStackValue::ObjectRef(image_id))
}

fn byte_values_to_bytes(values: &[JvmStackValue], context: &str) -> Result<Vec<u8>, String> {
    values
        .iter()
        .map(|value| match value {
            JvmStackValue::Byte(byte) => Ok(*byte),
            JvmStackValue::Int(int_value) => Ok(*int_value as u8),
            value => Err(format!(
                "{}: expected byte value, found {:?}",
                context, value
            )),
        })
        .collect()
}

fn int_arg(args: &[JvmStackValue], index: usize, context: &str) -> Result<i32, String> {
    match args.get(index) {
        Some(JvmStackValue::Int(value)) => Ok(*value),
        value => Err(format!("{}: invalid/missing int {:?}", context, value)),
    }
}

fn object_ref_arg(args: &[JvmStackValue], index: usize, context: &str) -> Result<usize, String> {
    match args.get(index) {
        Some(JvmStackValue::ObjectRef(id)) => Ok(*id as usize),
        Some(JvmStackValue::Null) => Err(format!("{}: NullPointerException", context)),
        value => Err(format!(
            "{}: expected object reference, found {:?}",
            context, value
        )),
    }
}

fn checked_range(
    len: usize,
    offset: i32,
    count: i32,
    context: &str,
) -> Result<std::ops::Range<usize>, String> {
    if offset < 0 || count < 0 {
        return Err(format!(
            "java.lang.IndexOutOfBoundsException: offset {}, length {}",
            offset, count
        ));
    }

    let offset = offset as usize;
    let count = count as usize;
    let Some(end) = offset.checked_add(count) else {
        return Err(format!(
            "java.lang.IndexOutOfBoundsException: {} range overflow",
            context
        ));
    };

    if end > len {
        return Err(format!(
            "java.lang.IndexOutOfBoundsException: {} range {}..{} out of bounds for length {}",
            context, offset, end, len
        ));
    }

    Ok(offset..end)
}

fn read_byte_array_range(
    jvm: &JVM,
    array_ref: &JvmStackValue,
    offset: i32,
    length: i32,
    context: &str,
) -> Result<Vec<u8>, String> {
    let array_id = match array_ref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        JvmStackValue::Null => return Err(format!("{}: NullPointerException", context)),
        value => {
            return Err(format!(
                "{}: expected byte array reference, found {:?}",
                context, value
            ));
        }
    };

    let bytes = {
        let state = jvm.state.lock();
        match state.heap.get(array_id) {
            Some(HeapObject::Array { element_type, data }) => {
                if element_type != "primitive_8" {
                    return Err(format!(
                        "{}: expected byte array, found array of type {}",
                        context, element_type
                    ));
                }
                byte_values_to_bytes(data, context)?
            }
            Some(_) => return Err(format!("{}: expected byte array", context)),
            None => {
                return Err(format!(
                    "{}: invalid byte array reference {}",
                    context, array_id
                ));
            }
        }
    };

    let range = checked_range(bytes.len(), offset, length, context)?;
    Ok(bytes[range].to_vec())
}

fn decode_image_bytes(bytes: &[u8], context: &str) -> Result<(i32, i32, Vec<u8>), String> {
    let decoded = ::image::load_from_memory(bytes)
        .map_err(|e| format!("{}: failed to decode encoded image bytes: {}", context, e))?;
    let rgba = decoded.to_rgba8();
    let width = rgba.width() as i32;
    let height = rgba.height() as i32;
    Ok((width, height, rgba.into_raw()))
}

fn create_image_from_bytes(
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let data = args
        .first()
        .ok_or("Image.createImage([BII): missing byte array")?;
    let offset = int_arg(args, 1, "Image.createImage([BII) offset")?;
    let length = int_arg(args, 2, "Image.createImage([BII) length")?;
    let bytes = read_byte_array_range(jvm, data, offset, length, "Image.createImage([BII)")?;
    let (width, height, pixels) = decode_image_bytes(&bytes, "Image.createImage([BII)")?;
    allocate_image(jvm, width, height, false, HashMap::new(), Some(pixels)).map(Some)
}

fn read_input_stream_bytes(stream_ref: &JvmStackValue, jvm: &JVM) -> Result<Vec<u8>, String> {
    let stream_id = match stream_ref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        JvmStackValue::Null => {
            return Err("Image.createImage(InputStream): NullPointerException".into())
        }
        value => {
            return Err(format!(
                "Image.createImage(InputStream): expected InputStream reference, found {:?}",
                value
            ));
        }
    };

    let state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get(stream_id) else {
        return Err("Image.createImage(InputStream): expected stream instance".into());
    };

    let pos = match obj.fields.get("jvm_pos") {
        Some(JvmStackValue::Int(value)) if *value >= 0 => *value as usize,
        Some(JvmStackValue::Int(_)) | None => 0,
        Some(value) => {
            return Err(format!(
                "Image.createImage(InputStream): invalid stream position {:?}",
                value
            ));
        }
    };

    let data = match obj.fields.get("jvm_data") {
        Some(JvmStackValue::Vector(values)) => {
            byte_values_to_bytes(values, "Image.createImage(InputStream)")
        }
        Some(value) => Err(format!(
            "Image.createImage(InputStream): invalid jvm_data field {:?}",
            value
        )),
        None => {
            let Some(JvmStackValue::String(path)) = obj.fields.get("jvm_res") else {
                return Err("Image.createImage(InputStream): stream has no backing bytes".into());
            };
            state.resources.get(path).cloned().ok_or_else(|| {
                format!(
                    "Image.createImage(InputStream): resource {} not found",
                    path
                )
            })
        }
    }?;

    Ok(data[pos.min(data.len())..].to_vec())
}

fn create_image_from_input_stream(
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let stream = args
        .first()
        .ok_or("Image.createImage(InputStream): missing stream")?;
    let bytes = read_input_stream_bytes(stream, jvm)?;
    let (width, height, pixels) = decode_image_bytes(&bytes, "Image.createImage(InputStream)")?;
    allocate_image(jvm, width, height, false, HashMap::new(), Some(pixels)).map(Some)
}

fn transform_dest_dimensions(width: i32, height: i32, transform: i32) -> (i32, i32) {
    match transform {
        4 | 5 | 6 | 7 => (height, width),
        _ => (width, height),
    }
}

fn transformed_source_point(
    dx: i32,
    dy: i32,
    width: i32,
    height: i32,
    transform: i32,
) -> (i32, i32) {
    match transform {
        0 => (dx, dy),
        1 => (dx, height - 1 - dy),
        2 => (width - 1 - dx, dy),
        3 => (width - 1 - dx, height - 1 - dy),
        4 => (dy, dx),
        5 => (dy, height - 1 - dx),
        6 => (width - 1 - dy, dx),
        7 => (width - 1 - dy, height - 1 - dx),
        _ => (dx, dy),
    }
}

fn create_image_from_image(
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let source_id = object_ref_arg(args, 0, "Image.createImage(Image) source")?;
    let source = JvmStackValue::ObjectRef(source_id as u32);
    let buffer = get_or_create_buffer(&source, jvm)
        .ok_or("Image.createImage(Image): failed to read source image")?;
    let guard = buffer.lock().unwrap();
    allocate_image(
        jvm,
        guard.width,
        guard.height,
        false,
        HashMap::new(),
        Some(guard.pixels.clone()),
    )
    .map(Some)
}

fn create_image_from_region(
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    let source_id = object_ref_arg(args, 0, "Image.createImage(Image,IIIII) source")?;
    let source = JvmStackValue::ObjectRef(source_id as u32);
    let x = int_arg(args, 1, "Image.createImage(Image,IIIII) x")?;
    let y = int_arg(args, 2, "Image.createImage(Image,IIIII) y")?;
    let width = int_arg(args, 3, "Image.createImage(Image,IIIII) width")?;
    let height = int_arg(args, 4, "Image.createImage(Image,IIIII) height")?;
    let transform = int_arg(args, 5, "Image.createImage(Image,IIIII) transform")?;

    if width <= 0 || height <= 0 {
        return Err(format!(
            "java.lang.IllegalArgumentException: invalid region size {}x{}",
            width, height
        ));
    }

    if !(0..=7).contains(&transform) {
        return Err(format!(
            "java.lang.IllegalArgumentException: invalid transform {}",
            transform
        ));
    }

    let buffer = get_or_create_buffer(&source, jvm)
        .ok_or("Image.createImage(Image,IIIII): failed to read source image")?;
    let guard = buffer.lock().unwrap();

    let Some(end_x) = x.checked_add(width) else {
        return Err("java.lang.IllegalArgumentException: source region overflows".into());
    };
    let Some(end_y) = y.checked_add(height) else {
        return Err("java.lang.IllegalArgumentException: source region overflows".into());
    };

    if x < 0 || y < 0 || end_x > guard.width || end_y > guard.height {
        return Err(format!(
            "java.lang.IllegalArgumentException: source region {},{} {}x{} outside image {}x{}",
            x, y, width, height, guard.width, guard.height
        ));
    }

    let (dest_w, dest_h) = transform_dest_dimensions(width, height, transform);
    let mut pixels = vec![0; (dest_w * dest_h * 4) as usize];
    let src_stride = (guard.width * 4) as usize;
    let dest_stride = (dest_w * 4) as usize;

    for dy in 0..dest_h {
        for dx in 0..dest_w {
            let (sx, sy) = transformed_source_point(dx, dy, width, height, transform);
            let src_offset = ((y + sy) as usize * src_stride) + ((x + sx) as usize * 4);
            let dest_offset = (dy as usize * dest_stride) + (dx as usize * 4);
            pixels[dest_offset..dest_offset + 4]
                .copy_from_slice(&guard.pixels[src_offset..src_offset + 4]);
        }
    }

    allocate_image(jvm, dest_w, dest_h, false, HashMap::new(), Some(pixels)).map(Some)
}

fn is_mutable_image(image: &HeapObject) -> bool {
    let HeapObject::Instance(obj) = image else {
        return false;
    };

    matches!(obj.fields.get("mutable:Z"), Some(JvmStackValue::Int(value)) if *value != 0)
}

fn rgba_to_argb(px: &[u8]) -> i32 {
    ((px[3] as i32) << 24) | ((px[0] as i32) << 16) | ((px[1] as i32) << 8) | px[2] as i32
}

fn get_rgb(image_ref: JvmStackValue, args: &[JvmStackValue], jvm: &JVM) -> Result<(), String> {
    let rgb_id = object_ref_arg(args, 0, "Image.getRGB rgbData")?;
    let offset = int_arg(args, 1, "Image.getRGB offset")?;
    let scanlength = int_arg(args, 2, "Image.getRGB scanlength")?;
    let x = int_arg(args, 3, "Image.getRGB x")?;
    let y = int_arg(args, 4, "Image.getRGB y")?;
    let width = int_arg(args, 5, "Image.getRGB width")?;
    let height = int_arg(args, 6, "Image.getRGB height")?;

    if width < 0 || height < 0 {
        return Err(format!(
            "java.lang.IllegalArgumentException: invalid region size {}x{}",
            width, height
        ));
    }

    if width == 0 || height == 0 {
        return Ok(());
    }

    let buffer =
        get_or_create_buffer(&image_ref, jvm).ok_or("Image.getRGB: failed to read source image")?;
    let guard = buffer.lock().unwrap();

    let Some(end_x) = x.checked_add(width) else {
        return Err("java.lang.IllegalArgumentException: source region overflows".into());
    };
    let Some(end_y) = y.checked_add(height) else {
        return Err("java.lang.IllegalArgumentException: source region overflows".into());
    };

    if x < 0 || y < 0 || end_x > guard.width || end_y > guard.height {
        return Err(format!(
            "java.lang.IllegalArgumentException: source region {},{} {}x{} outside image {}x{}",
            x, y, width, height, guard.width, guard.height
        ));
    }

    let mut writes = Vec::with_capacity((width * height) as usize);
    let src_stride = (guard.width * 4) as usize;
    for row in 0..height {
        for col in 0..width {
            let dest_index = offset as i64 + row as i64 * scanlength as i64 + col as i64;
            let src_offset = ((y + row) as usize * src_stride) + ((x + col) as usize * 4);
            writes.push((
                dest_index,
                rgba_to_argb(&guard.pixels[src_offset..src_offset + 4]),
            ));
        }
    }
    drop(guard);

    let mut state = jvm.state.lock();
    match state.heap.get_mut(rgb_id) {
        Some(HeapObject::Array { element_type, data }) => {
            if element_type != "primitive_10" {
                return Err(format!(
                    "Image.getRGB: expected int array, found array of type {}",
                    element_type
                ));
            }

            for (dest_index, _) in &writes {
                if *dest_index < 0 || *dest_index as usize >= data.len() {
                    return Err(format!(
                        "java.lang.ArrayIndexOutOfBoundsException: index {} out of bounds for length {}",
                        dest_index,
                        data.len()
                    ));
                }
            }

            for (dest_index, argb) in writes {
                data[dest_index as usize] = JvmStackValue::Int(argb);
            }

            Ok(())
        }
        Some(_) => Err("Image.getRGB: rgbData is not an array".into()),
        None => Err(format!(
            "Image.getRGB: invalid rgbData reference {}",
            rgb_id
        )),
    }
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
                if let Ok(decoded) = ::image::load_from_memory(data) {
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

    if *width <= 0 || *height <= 0 {
        return Err(format!(
            "java.lang.IllegalArgumentException: invalid image size {}x{}",
            width, height
        ));
    }

    let pixel_len = (*width as usize)
        .checked_mul(*height as usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| {
            "java.lang.IllegalArgumentException: image dimensions overflow".to_string()
        })?;

    let mut fields = HashMap::new();
    fields.insert("buff".to_string(), JvmStackValue::Vector(Vec::new()));

    allocate_image(jvm, *width, *height, true, fields, Some(vec![0; pixel_len])).map(Some)
}

fn create_rgb_image(args: &[JvmStackValue], jvm: &JVM) -> Result<Option<JvmStackValue>, String> {
    let rgb_ref = match args.get(0) {
        Some(JvmStackValue::ObjectRef(id)) => *id as usize,
        Some(JvmStackValue::Null) => return Err("java.lang.NullPointerException".into()),
        Some(value) => {
            return Err(format!(
                "Image.createRGBImage: expected int array, found {:?}",
                value
            ));
        }
        None => return Err("Image.createRGBImage: missing rgb array argument".into()),
    };

    let width = match args.get(1) {
        Some(JvmStackValue::Int(width)) => *width,
        Some(value) => {
            return Err(format!(
                "Image.createRGBImage: expected width int, found {:?}",
                value
            ));
        }
        None => return Err("Image.createRGBImage: missing width argument".into()),
    };

    let height = match args.get(2) {
        Some(JvmStackValue::Int(height)) => *height,
        Some(value) => {
            return Err(format!(
                "Image.createRGBImage: expected height int, found {:?}",
                value
            ));
        }
        None => return Err("Image.createRGBImage: missing height argument".into()),
    };

    let process_alpha = match args.get(3) {
        Some(JvmStackValue::Int(value)) => *value != 0,
        Some(value) => {
            return Err(format!(
                "Image.createRGBImage: expected boolean int, found {:?}",
                value
            ));
        }
        None => return Err("Image.createRGBImage: missing processAlpha argument".into()),
    };

    if width <= 0 || height <= 0 {
        return Err(format!(
            "java.lang.IllegalArgumentException: invalid image size {}x{}",
            width, height
        ));
    }

    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| {
            "java.lang.IllegalArgumentException: image dimensions overflow".to_string()
        })?;

    let rgb_values = {
        let state = jvm.state.lock();
        match state.heap.get(rgb_ref) {
            Some(HeapObject::Array { element_type, data }) => {
                if element_type != "primitive_10" {
                    return Err(format!(
                        "Image.createRGBImage: expected int array, found array of type {}",
                        element_type
                    ));
                }

                if data.len() < pixel_count {
                    return Err(format!(
                        "java.lang.ArrayIndexOutOfBoundsException: need {} pixels, array length {}",
                        pixel_count,
                        data.len()
                    ));
                }

                data.iter()
                    .take(pixel_count)
                    .map(|value| match value {
                        JvmStackValue::Int(argb) => Ok(*argb),
                        value => Err(format!(
                            "Image.createRGBImage: expected int pixel, found {:?}",
                            value
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            Some(_) => return Err("Image.createRGBImage: rgbData is not an array".into()),
            None => {
                return Err(format!(
                    "Image.createRGBImage: invalid rgb array reference {}",
                    rgb_ref
                ));
            }
        }
    };

    let mut pixels = Vec::with_capacity(pixel_count * 4);
    for argb in rgb_values {
        pixels.push(((argb >> 16) & 0xFF) as u8);
        pixels.push(((argb >> 8) & 0xFF) as u8);
        pixels.push((argb & 0xFF) as u8);
        pixels.push(if process_alpha {
            ((argb >> 24) & 0xFF) as u8
        } else {
            0xFF
        });
    }

    let mut fields = HashMap::new();
    fields.insert(
        "processAlpha:Z".to_string(),
        JvmStackValue::Int(if process_alpha { 1 } else { 0 }),
    );

    allocate_image(jvm, width, height, false, fields, Some(pixels)).map(Some)
}
