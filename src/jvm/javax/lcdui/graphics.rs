use crate::{
    app::DRAW_STATE,
    jvm::jvm_core::{HeapObject, JVM, JvmStackValue},
};

pub const CLASS_NAME: &str = "javax/microedition/lcdui/Graphics";

pub fn handle_virtual_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("setColor", "(I)V") => {
            let color = match args.get(0) {
                Some(JvmStackValue::Int(c)) => *c,
                _ => return Err("Graphics.setColor: expected int argument".into()),
            };
            set_color_field(objectref, jvm, color);
            return Ok(None);
        }
        ("setColor", "(III)V") => {
            let r = match args.get(0) {
                Some(JvmStackValue::Int(c)) => (*c & 0xFF) as i32,
                _ => return Err("Graphics.setColor(III)V: expected int argument".into()),
            };
            let g = match args.get(1) {
                Some(JvmStackValue::Int(c)) => (*c & 0xFF) as i32,
                _ => return Err("Graphics.setColor(III)V: expected int argument".into()),
            };
            let b = match args.get(2) {
                Some(JvmStackValue::Int(c)) => (*c & 0xFF) as i32,
                _ => return Err("Graphics.setColor(III)V: expected int argument".into()),
            };

            let color = (r << 16) | (g << 8) | b;
            set_color_field(objectref, jvm, color);
            return Ok(None);
        }
        ("setStrokeStyle", "(I)V") => {
            let style = match args.get(0) {
                Some(JvmStackValue::Int(s)) => *s,
                _ => return Err("Graphics.setStrokeStyle: expected int argument".into()),
            };
            set_int_field(objectref, jvm, "strokeStyle", style);
            return Ok(None);
        }
        ("getStrokeStyle", "()I") => {
            return Ok(Some(JvmStackValue::Int(get_int_field(
                objectref,
                jvm,
                "strokeStyle",
                0,
            ))));
        }
        ("fillRect", "(IIII)V") => {
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);
            fill_rect(x, y, width, height, color);
            Ok(None)
        }
        ("drawRect", "(IIII)V") => {
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);
            let style = get_int_field(objectref, jvm, "strokeStyle", 0);
            draw_rect(x, y, width, height, color, style == 1);
            Ok(None)
        }
        ("drawLine", "(IIII)V") => {
            let x1 = get_int_arg(args, 0)?;
            let y1 = get_int_arg(args, 1)?;
            let x2 = get_int_arg(args, 2)?;
            let y2 = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);
            let style = get_int_field(objectref, jvm, "strokeStyle", 0);
            draw_line(x1, y1, x2, y2, color, style == 1);
            Ok(None)
        }
        ("drawRegion", "(Ljavax/microedition/lcdui/Image;IIIIIIII)V") => {
            let img_ref = args.get(0).ok_or("drawRegion: missing image")?;
            let x_src = get_int_arg(args, 1)?;
            let y_src = get_int_arg(args, 2)?;
            let width = get_int_arg(args, 3)?;
            let height = get_int_arg(args, 4)?;
            let transform = get_int_arg(args, 5)?;
            let x_dest = get_int_arg(args, 6)?;
            let y_dest = get_int_arg(args, 7)?;
            let anchor = get_int_arg(args, 8)?;

            draw_region(
                img_ref, x_src, y_src, width, height, transform, x_dest, y_dest, anchor, jvm,
            );
            Ok(None)
        }
        ("drawImage", "(Ljavax/microedition/lcdui/Image;III)V") => {
            let img_ref = args.get(0).ok_or("drawImage: missing image")?;
            let x = get_int_arg(args, 1)?;
            let y = get_int_arg(args, 2)?;
            let anchor = get_int_arg(args, 3)?;

            let (w, h) = get_image_dim(img_ref, jvm);
            draw_region(img_ref, 0, 0, w, h, 0, x, y, anchor, jvm);
            Ok(None)
        }
        ("fillTriangle", "(IIIIII)V") => {
            let x1 = get_int_arg(args, 0)?;
            let y1 = get_int_arg(args, 1)?;
            let x2 = get_int_arg(args, 2)?;
            let y2 = get_int_arg(args, 3)?;
            let x3 = get_int_arg(args, 4)?;
            let y3 = get_int_arg(args, 5)?;
            let color = get_color(objectref, jvm);
            fill_triangle(x1, y1, x2, y2, x3, y3, color);
            Ok(None)
        }
        ("drawArc", "(IIIIII)V") => {
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let startAngle = get_int_arg(args, 4)?;
            let arcAngle = get_int_arg(args, 5)?;

            let color = get_color(objectref, jvm);
            draw_arc(x, y, width, height, startAngle, arcAngle, color);

            Ok(None)
        }
        ("drawString", "(Ljava/lang/String;III)V") => todo!("Graphics.drawString"),
        ("drawSubstring", "(Ljava/lang/String;IIIII)V") => todo!("Graphics.drawSubstring"),
        ("setFont", "(Ljavax/microedition/lcdui/Font;)V") => todo!("Graphics.setFont"),
        ("setClip", "(IIII)V") => todo!("Graphics.setClip"),
        ("clipRect", "(IIII)V") => todo!("Graphics.clipRect"),
        ("translate", "(II)V") => todo!("Graphics.translate"),
        ("getClipX", "()I") => todo!("Graphics.getClipX"),
        ("getClipY", "()I") => todo!("Graphics.getClipY"),
        ("getClipWidth", "()I") => {
            let draw_state = DRAW_STATE.lock();
            Ok(Some(JvmStackValue::Int(draw_state.width as i32)))
        }
        ("getClipHeight", "()I") => {
            let draw_state = DRAW_STATE.lock();
            Ok(Some(JvmStackValue::Int(draw_state.height as i32)))
        }
        _ => Err(format!(
            "Unsupported Graphics instance method: {}{}",
            method_name, descriptor
        )),
    }
}

fn get_int_arg(args: &[JvmStackValue], index: usize) -> Result<i32, String> {
    match args.get(index) {
        Some(JvmStackValue::Int(v)) => Ok(*v),
        _ => Err(format!(
            "Graphics method: expected int argument at index {}",
            index
        )),
    }
}

fn get_color(objectref: &JvmStackValue, jvm: &JVM) -> [u8; 4] {
    let heap_idx = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return [0, 0, 0, 255],
    };

    let state = jvm.state.lock();
    if let Some(HeapObject::Instance(obj)) = state.heap.get(heap_idx) {
        if let Some(JvmStackValue::Int(c)) = obj.fields.get("color") {
            let r = ((c >> 16) & 0xFF) as u8;
            let g = ((c >> 8) & 0xFF) as u8;
            let b = (c & 0xFF) as u8;
            return [r, g, b, 255];
        }
    }
    [0, 0, 0, 255]
}

fn get_int_field(objectref: &JvmStackValue, jvm: &JVM, field_name: &str, default: i32) -> i32 {
    let heap_idx = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return default,
    };

    let state = jvm.state.lock();
    if let Some(HeapObject::Instance(obj)) = state.heap.get(heap_idx) {
        if let Some(JvmStackValue::Int(v)) = obj.fields.get(field_name) {
            return *v;
        }
    }
    default
}

fn set_int_field(objectref: &JvmStackValue, jvm: &JVM, field_name: &str, value: i32) {
    let heap_idx = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return,
    };

    let mut state = jvm.state.lock();
    if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(heap_idx) {
        obj.fields
            .insert(field_name.to_string(), JvmStackValue::Int(value));
    }
}

fn get_image_dim(img_ref: &JvmStackValue, jvm: &JVM) -> (i32, i32) {
    let img_id = match img_ref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return (0, 0),
    };
    let state = jvm.state.lock();
    if let Some(HeapObject::Instance(obj)) = state.heap.get(img_id) {
        let w = match obj.fields.get("width:I") {
            Some(JvmStackValue::Int(v)) => *v,
            _ => 0,
        };
        let h = match obj.fields.get("height:I") {
            Some(JvmStackValue::Int(v)) => *v,
            _ => 0,
        };
        return (w, h);
    }
    (0, 0)
}

fn set_color_field(objectref: &JvmStackValue, jvm: &JVM, color: i32) {
    set_int_field(objectref, jvm, "color", color);
}

fn fill_rect(x: i32, y: i32, width: i32, height: i32, color: [u8; 4]) {
    let mut draw_state = DRAW_STATE.lock();
    let dw = draw_state.width as i32;
    let dh = draw_state.height as i32;
    if let Some(pixels) = &mut draw_state.pixels {
        let frame = pixels.frame_mut();
        for iy in y..(y + height) {
            if iy < 0 || iy >= dh {
                continue;
            }
            for ix in x..(x + width) {
                if ix < 0 || ix >= dw {
                    continue;
                }
                let offset = ((iy * dw + ix) * 4) as usize;
                if offset + 4 <= frame.len() {
                    frame[offset..offset + 4].copy_from_slice(&color);
                }
            }
        }
    }
}

fn draw_rect(x: i32, y: i32, width: i32, height: i32, color: [u8; 4], dotted: bool) {
    draw_line(x, y, x + width, y, color, dotted);
    draw_line(x + width, y, x + width, y + height, color, dotted);
    draw_line(x + width, y + height, x, y + height, color, dotted);
    draw_line(x, y + height, x, y, color, dotted);
}

fn draw_line(x1: i32, y1: i32, x2: i32, y2: i32, color: [u8; 4], dotted: bool) {
    let mut draw_state = DRAW_STATE.lock();
    let dw = draw_state.width as i32;
    let dh = draw_state.height as i32;
    if let Some(pixels) = &mut draw_state.pixels {
        let frame = pixels.frame_mut();

        let dx = (x2 - x1).abs();
        let dy = (y2 - y1).abs();
        let sx = if x1 < x2 { 1 } else { -1 };
        let sy = if y1 < y2 { 1 } else { -1 };
        let mut err = dx - dy;

        let mut x = x1;
        let mut y = y1;
        let mut step = 0;

        loop {
            if !dotted || step % 4 < 2 {
                if x >= 0 && x < dw && y >= 0 && y < dh {
                    let offset = ((y * dw + x) * 4) as usize;
                    if offset + 4 <= frame.len() {
                        frame[offset..offset + 4].copy_from_slice(&color);
                    }
                }
            }
            step += 1;

            if x == x2 && y == y2 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }
}

fn draw_region(
    img_ref: &JvmStackValue,
    x_src: i32,
    y_src: i32,
    width: i32,
    height: i32,
    transform: i32,
    x_dest: i32,
    y_dest: i32,
    anchor: i32,
    jvm: &JVM,
) {
    let img_id = match img_ref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return,
    };

    let resource_data = {
        let state = jvm.state.lock();
        let Some(HeapObject::Instance(obj)) = state.heap.get(img_id) else {
            return;
        };
        let Some(JvmStackValue::String(path)) = obj.fields.get("path:Ljava/lang/String;") else {
            return;
        };
        let resource_name = path.strip_prefix('/').unwrap_or(path);
        state.resources.get(resource_name).cloned()
    };

    let Some(data) = resource_data else {
        return;
    };

    // Decode image (slow, but works for now)
    let Ok(img) = image::load_from_memory(&data) else {
        return;
    };
    let rgba = img.to_rgba8();

    let mut real_x = x_dest;
    let mut real_y = y_dest;

    // Anchor handling
    if anchor & 1 != 0 {
        // HCENTER
        real_x -= width / 2;
    } else if anchor & 8 != 0 {
        // RIGHT
        real_x -= width;
    }

    if anchor & 2 != 0 {
        // VCENTER
        real_y -= height / 2;
    } else if anchor & 32 != 0 {
        // BOTTOM
        real_y -= height;
    }

    let mut draw_state = DRAW_STATE.lock();
    let dw = draw_state.width as i32;
    let dh = draw_state.height as i32;

    if let Some(pixels) = &mut draw_state.pixels {
        let frame = pixels.frame_mut();

        for iy in 0..height {
            for ix in 0..width {
                let mut sx = ix;
                let mut sy = iy;

                // Transform handling
                match transform {
                    0 => {}                    // NONE
                    2 => sx = width - 1 - ix,  // MIRROR
                    1 => sy = height - 1 - iy, // MIRROR_ROT180 (Vertical flip)
                    3 => {
                        // ROT180
                        sx = width - 1 - ix;
                        sy = height - 1 - iy;
                    }
                    _ => {} // Fallback to none for now
                }

                let src_px_x = x_src + sx;
                let src_px_y = y_src + sy;

                if src_px_x < 0
                    || src_px_x >= rgba.width() as i32
                    || src_px_y < 0
                    || src_px_y >= rgba.height() as i32
                {
                    continue;
                }

                let px = rgba.get_pixel(src_px_x as u32, src_px_y as u32);
                if px[3] == 0 {
                    continue;
                } // Fully transparent

                let dest_px_x = real_x + ix;
                let dest_px_y = real_y + iy;

                if dest_px_x < 0 || dest_px_x >= dw || dest_px_y < 0 || dest_px_y >= dh {
                    continue;
                }

                let offset = ((dest_px_y * dw + dest_px_x) * 4) as usize;
                if offset + 4 <= frame.len() {
                    if px[3] == 255 {
                        frame[offset..offset + 4].copy_from_slice(&px.0);
                    } else {
                        // Blend
                        let alpha = px[3] as f32 / 255.0;
                        for i in 0..3 {
                            frame[offset + i] = ((px[i] as f32 * alpha)
                                + (frame[offset + i] as f32 * (1.0 - alpha)))
                                as u8;
                        }
                    }
                }
            }
        }
    }
}

fn fill_triangle(
    mut x1: i32,
    mut y1: i32,
    mut x2: i32,
    mut y2: i32,
    mut x3: i32,
    mut y3: i32,
    color: [u8; 4],
) {
    // Sort vertices by Y
    if y1 > y2 {
        std::mem::swap(&mut x1, &mut x2);
        std::mem::swap(&mut y1, &mut y2);
    }
    if y1 > y3 {
        std::mem::swap(&mut x1, &mut x3);
        std::mem::swap(&mut y1, &mut y3);
    }
    if y2 > y3 {
        std::mem::swap(&mut x2, &mut x3);
        std::mem::swap(&mut y2, &mut y3);
    }

    if y1 == y3 {
        return;
    } // Flat line

    let mut draw_state = DRAW_STATE.lock();
    let dw = draw_state.width as i32;
    let dh = draw_state.height as i32;
    if let Some(pixels) = &mut draw_state.pixels {
        let frame = pixels.frame_mut();

        if y2 == y3 {
            fill_bottom_flat_triangle(x1, y1, x2, y2, x3, y3, dw, dh, frame, color);
        } else if y1 == y2 {
            fill_top_flat_triangle(x1, y1, x2, y2, x3, y3, dw, dh, frame, color);
        } else {
            let x4 = x1 + ((y2 - y1) as f32 * (x3 - x1) as f32 / (y3 - y1) as f32) as i32;
            fill_bottom_flat_triangle(x1, y1, x2, y2, x4, y2, dw, dh, frame, color);
            fill_top_flat_triangle(x2, y2, x4, y2, x3, y3, dw, dh, frame, color);
        }
    }
}

fn fill_bottom_flat_triangle(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    x3: i32,
    y3: i32,
    dw: i32,
    dh: i32,
    frame: &mut [u8],
    color: [u8; 4],
) {
    let dy = (y2 - y1) as f32;
    if dy == 0.0 {
        return;
    }
    let invslope1 = (x2 - x1) as f32 / dy;
    let invslope2 = (x3 - x1) as f32 / dy;

    let mut curx1 = x1 as f32;
    let mut curx2 = x1 as f32;

    for scanline_y in y1..y2 {
        draw_horizontal_line(curx1 as i32, curx2 as i32, scanline_y, dw, dh, frame, color);
        curx1 += invslope1;
        curx2 += invslope2;
    }
}

fn fill_top_flat_triangle(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    x3: i32,
    y3: i32,
    dw: i32,
    dh: i32,
    frame: &mut [u8],
    color: [u8; 4],
) {
    let dy = (y3 - y1) as f32;
    if dy == 0.0 {
        return;
    }
    let invslope1 = (x3 - x1) as f32 / dy;
    let invslope2 = (x3 - x2) as f32 / dy;

    let mut curx1 = x1 as f32;
    let mut curx2 = x2 as f32;

    for scanline_y in y1..=y3 {
        draw_horizontal_line(curx1 as i32, curx2 as i32, scanline_y, dw, dh, frame, color);
        curx1 += invslope1;
        curx2 += invslope2;
    }
}

fn draw_horizontal_line(
    mut x1: i32,
    mut x2: i32,
    y: i32,
    dw: i32,
    dh: i32,
    frame: &mut [u8],
    color: [u8; 4],
) {
    if y < 0 || y >= dh {
        return;
    }
    if x1 > x2 {
        std::mem::swap(&mut x1, &mut x2);
    }

    // Add 1 to x2 to make it inclusive if needed, but J2ME fill usually includes the last pixel
    for x in x1..=x2 {
        if x < 0 || x >= dw {
            continue;
        }
        let offset = ((y * dw + x) * 4) as usize;
        if offset + 4 <= frame.len() {
            frame[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

fn draw_arc(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    start_angle: i32,
    arc_angle: i32,
    color: [u8; 4],
) {
    let mut draw_state = DRAW_STATE.lock();
    let dw = draw_state.width as i32;
    let dh = draw_state.height as i32;
    if let Some(pixels) = &mut draw_state.pixels {
        let frame = pixels.frame_mut();
        let cx = x + width / 2;
        let cy = y + height / 2;
        let rx = width / 2;
        let ry = height / 2;

        let start_rad = (start_angle as f32) * std::f32::consts::PI / 180.0;
        let end_rad = ((start_angle + arc_angle) as f32) * std::f32::consts::PI / 180.0;

        let steps = (arc_angle.abs() / 5).max(1) as usize; // Adjust step size for smoother arcs
        let angle_step = (end_rad - start_rad) / steps as f32;

        for i in 0..=steps {
            let angle = start_rad + i as f32 * angle_step;
            let px = cx + (rx as f32 * angle.cos()) as i32;
            let py = cy + (ry as f32 * angle.sin()) as i32;

            if px >= 0 && px < dw && py >= 0 && py < dh {
                let offset = ((py * dw + px) * 4) as usize;
                if offset + 4 <= frame.len() {
                    frame[offset..offset + 4].copy_from_slice(&color);
                }
            }
        }
    }
}
