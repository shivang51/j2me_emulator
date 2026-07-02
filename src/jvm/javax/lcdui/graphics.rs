use std::collections::HashMap;

use crate::{
    app::DRAW_STATE,
    jvm::{
        javax::lcdui::image::get_or_create_buffer,
        jvm_core::{HeapObject, JvmStackValue, JVM},
    },
    profile::Profile,
};

pub const CLASS_NAME: &str = "javax/microedition/lcdui/Graphics";
pub const FONT_CLASS_NAME: &str = "javax/microedition/lcdui/Font";

const DEFAULT_FONT_HEIGHT: i32 = 16;
const FONT_SCALE: i32 = 2;
const GLYPH_WIDTH: i32 = 5;
const GLYPH_HEIGHT: i32 = 7;
const FONT_ADVANCE: i32 = GLYPH_WIDTH * FONT_SCALE + 2;

#[derive(Clone, Copy)]
struct ClipRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl ClipRect {
    fn right(self) -> i32 {
        self.x.saturating_add(self.width)
    }

    fn bottom(self) -> i32 {
        self.y.saturating_add(self.height)
    }

    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    fn intersect(self, other: ClipRect) -> ClipRect {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = self.right().min(other.right());
        let y2 = self.bottom().min(other.bottom());

        ClipRect {
            x: x1,
            y: y1,
            width: (x2 - x1).max(0),
            height: (y2 - y1).max(0),
        }
    }
}

pub fn handle_virtual_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("setColor", "(I)V") => {
            Profile::this("setColor(I)V");
            let color = match args.get(0) {
                Some(JvmStackValue::Int(c)) => *c,
                _ => return Err("Graphics.setColor: expected int argument".into()),
            };
            set_color_field(objectref, jvm, color);
            return Ok(None);
        }
        ("setColor", "(III)V") => {
            Profile::this("setColor(III)V");
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
            Profile::this("setStrokeStyle(I)V");
            let style = match args.get(0) {
                Some(JvmStackValue::Int(s)) => *s,
                _ => return Err("Graphics.setStrokeStyle: expected int argument".into()),
            };
            set_int_field(objectref, jvm, "strokeStyle", style);
            return Ok(None);
        }
        ("getStrokeStyle", "()I") => {
            Profile::this("getStrokeStyle()I");
            return Ok(Some(JvmStackValue::Int(get_int_field(
                objectref,
                jvm,
                "strokeStyle",
                0,
            ))));
        }
        ("fillRect", "(IIII)V") => {
            Profile::this("fillRect(IIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);
            fill_rect(
                objectref,
                jvm,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                width,
                height,
                color,
            );
            Ok(None)
        }
        ("drawRect", "(IIII)V") => {
            Profile::this("drawRect(IIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);
            let style = get_int_field(objectref, jvm, "strokeStyle", 0);
            draw_rect(
                objectref,
                jvm,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                width,
                height,
                color,
                style == 1,
            );
            Ok(None)
        }
        ("drawLine", "(IIII)V") => {
            Profile::this("drawLine(IIII)V");
            let x1 = get_int_arg(args, 0)?;
            let y1 = get_int_arg(args, 1)?;
            let x2 = get_int_arg(args, 2)?;
            let y2 = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);
            let style = get_int_field(objectref, jvm, "strokeStyle", 0);
            draw_line(
                objectref,
                jvm,
                x1 + get_translate_x(objectref, jvm),
                y1 + get_translate_y(objectref, jvm),
                x2 + get_translate_x(objectref, jvm),
                y2 + get_translate_y(objectref, jvm),
                color,
                style == 1,
            );
            Ok(None)
        }
        ("drawRegion", "(Ljavax/microedition/lcdui/Image;IIIIIIII)V") => {
            Profile::this("drawRegion(IIIIIIII)V");
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
                objectref,
                img_ref,
                x_src,
                y_src,
                width,
                height,
                transform,
                x_dest + get_translate_x(objectref, jvm),
                y_dest + get_translate_y(objectref, jvm),
                anchor,
                jvm,
            );
            Ok(None)
        }
        ("drawImage", "(Ljavax/microedition/lcdui/Image;III)V") => {
            Profile::this("drawImage(III)V");
            let img_ref = args.get(0).ok_or("drawImage: missing image")?;
            let x = get_int_arg(args, 1)?;
            let y = get_int_arg(args, 2)?;
            let anchor = get_int_arg(args, 3)?;

            let (w, h) = get_image_dim(img_ref, jvm);
            draw_region(
                objectref,
                img_ref,
                0,
                0,
                w,
                h,
                0,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                anchor,
                jvm,
            );
            Ok(None)
        }
        ("fillTriangle", "(IIIIII)V") => {
            Profile::this("fillTriangle(IIIIII)V");
            let x1 = get_int_arg(args, 0)?;
            let y1 = get_int_arg(args, 1)?;
            let x2 = get_int_arg(args, 2)?;
            let y2 = get_int_arg(args, 3)?;
            let x3 = get_int_arg(args, 4)?;
            let y3 = get_int_arg(args, 5)?;
            let color = get_color(objectref, jvm);
            fill_triangle(
                objectref,
                jvm,
                x1 + get_translate_x(objectref, jvm),
                y1 + get_translate_y(objectref, jvm),
                x2 + get_translate_x(objectref, jvm),
                y2 + get_translate_y(objectref, jvm),
                x3 + get_translate_x(objectref, jvm),
                y3 + get_translate_y(objectref, jvm),
                color,
            );
            Ok(None)
        }
        ("drawArc", "(IIIIII)V") => {
            Profile::this("drawArc(IIIIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let start_angle = get_int_arg(args, 4)?;
            let arc_angle = get_int_arg(args, 5)?;

            let color = get_color(objectref, jvm);
            draw_arc(
                objectref,
                jvm,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                width,
                height,
                start_angle,
                arc_angle,
                color,
            );

            Ok(None)
        }
        ("fillArc", "(IIIIII)V") => {
            Profile::this("fillArc(IIIIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let start_angle = get_int_arg(args, 4)?;
            let arc_angle = get_int_arg(args, 5)?;
            let color = get_color(objectref, jvm);

            fill_arc(
                objectref,
                jvm,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                width,
                height,
                start_angle,
                arc_angle,
                color,
            );

            Ok(None)
        }
        ("drawRoundRect", "(IIIIII)V") => {
            Profile::this("drawRoundRect(IIIIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let arc_width = get_int_arg(args, 4)?;
            let arc_height = get_int_arg(args, 5)?;
            let color = get_color(objectref, jvm);
            let style = get_int_field(objectref, jvm, "strokeStyle", 0);

            draw_round_rect(
                objectref,
                jvm,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                width,
                height,
                arc_width,
                arc_height,
                color,
                style == 1,
            );

            Ok(None)
        }
        ("fillRoundRect", "(IIIIII)V") => {
            Profile::this("fillRoundRect(IIIIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            let arc_width = get_int_arg(args, 4)?;
            let arc_height = get_int_arg(args, 5)?;
            let color = get_color(objectref, jvm);

            fill_round_rect(
                objectref,
                jvm,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                width,
                height,
                arc_width,
                arc_height,
                color,
            );

            Ok(None)
        }
        ("drawString", "(Ljava/lang/String;III)V") => {
            Profile::this("drawString(Ljava/lang/String;III)V");
            let text = get_string_arg(args, 0, jvm, "Graphics.drawString")?;
            let x = get_int_arg(args, 1)?;
            let y = get_int_arg(args, 2)?;
            let anchor = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);

            draw_text(
                objectref,
                jvm,
                &text,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                anchor,
                color,
            );

            Ok(None)
        }
        ("drawRGB", "([IIIIIIIZ)V") => {
            Profile::this("drawRGB([IIIIIIIZ)V");
            let rgb_ref = args.get(0).ok_or("drawRGB: missing rgbData")?;
            let offset = get_int_arg(args, 1)?;
            let scanline = get_int_arg(args, 2)?;
            let x = get_int_arg(args, 3)?;
            let y = get_int_arg(args, 4)?;
            let width = get_int_arg(args, 5)?;
            let height = get_int_arg(args, 6)?;
            let process_alpha = match args.get(7) {
                Some(JvmStackValue::Int(value)) => *value != 0,
                Some(value) => {
                    return Err(format!(
                        "Graphics.drawRGB: expected boolean int argument, found {:?}",
                        value
                    ));
                }
                None => return Err("Graphics.drawRGB: missing processAlpha argument".into()),
            };

            let rgb_values = read_rgb_int_array(rgb_ref, jvm)?;
            validate_draw_rgb_bounds(offset, scanline, width, height, rgb_values.len())?;

            draw_rgb(
                objectref,
                jvm,
                &rgb_values,
                offset,
                scanline,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                width,
                height,
                process_alpha,
            );

            Ok(None)
        }
        ("drawSubstring", "(Ljava/lang/String;IIIII)V") => {
            Profile::this("drawSubstring(Ljava/lang/String;IIIII)V");
            let text = get_string_arg(args, 0, jvm, "Graphics.drawSubstring")?;
            let offset = get_int_arg(args, 1)?;
            let len = get_int_arg(args, 2)?;
            let x = get_int_arg(args, 3)?;
            let y = get_int_arg(args, 4)?;
            let anchor = get_int_arg(args, 5)?;
            let substring = substring_chars(&text, offset, len, "Graphics.drawSubstring")?;
            let color = get_color(objectref, jvm);

            draw_text(
                objectref,
                jvm,
                &substring,
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                anchor,
                color,
            );

            Ok(None)
        }
        ("drawChar", "(CIII)V") => {
            Profile::this("drawChar(CIII)V");
            let ch = char::from_u32(get_int_arg(args, 0)? as u32).unwrap_or('\u{fffd}');
            let x = get_int_arg(args, 1)?;
            let y = get_int_arg(args, 2)?;
            let anchor = get_int_arg(args, 3)?;
            let color = get_color(objectref, jvm);

            draw_text(
                objectref,
                jvm,
                &ch.to_string(),
                x + get_translate_x(objectref, jvm),
                y + get_translate_y(objectref, jvm),
                anchor,
                color,
            );

            Ok(None)
        }
        ("setFont", "(Ljavax/microedition/lcdui/Font;)V") => {
            let font_ref = args
                .get(0)
                .ok_or_else(|| "Graphics.setFont: missing font argument".to_string())?;
            set_font_field(objectref, jvm, font_ref)?;
            Ok(None)
        }
        ("getFont", "()Ljavax/microedition/lcdui/Font;") => {
            let font_ref = get_or_create_font(objectref, jvm)?;
            Ok(Some(JvmStackValue::ObjectRef(font_ref)))
        }
        ("getColor", "()I") => Ok(Some(JvmStackValue::Int(get_color_int(objectref, jvm)))),
        ("getRedComponent", "()I") => Ok(Some(JvmStackValue::Int(
            (get_color_int(objectref, jvm) >> 16) & 0xFF,
        ))),
        ("getGreenComponent", "()I") => Ok(Some(JvmStackValue::Int(
            (get_color_int(objectref, jvm) >> 8) & 0xFF,
        ))),
        ("getBlueComponent", "()I") => Ok(Some(JvmStackValue::Int(
            get_color_int(objectref, jvm) & 0xFF,
        ))),
        ("setGrayScale", "(I)V") => {
            let value = get_int_arg(args, 0)?.clamp(0, 255);
            set_color_field(objectref, jvm, (value << 16) | (value << 8) | value);
            Ok(None)
        }
        ("getGrayScale", "()I") => {
            let color = get_color_int(objectref, jvm);
            let r = (color >> 16) & 0xFF;
            let g = (color >> 8) & 0xFF;
            let b = color & 0xFF;
            Ok(Some(JvmStackValue::Int((r * 30 + g * 59 + b * 11) / 100)))
        }
        ("setClip", "(IIII)V") => {
            Profile::this("setClip(IIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            set_clip_rect(objectref, jvm, x, y, width, height);
            Ok(None)
        }
        ("clipRect", "(IIII)V") => {
            Profile::this("clipRect(IIII)V");
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let width = get_int_arg(args, 2)?;
            let height = get_int_arg(args, 3)?;
            intersect_clip_rect(objectref, jvm, x, y, width, height);
            Ok(None)
        }
        ("translate", "(II)V") => {
            let x = get_int_arg(args, 0)?;
            let y = get_int_arg(args, 1)?;
            let translate_x = get_translate_x(objectref, jvm);
            let translate_y = get_translate_y(objectref, jvm);
            set_int_field(objectref, jvm, "translateX", translate_x + x);
            set_int_field(objectref, jvm, "translateY", translate_y + y);
            Ok(None)
        }
        ("getClipX", "()I") => Ok(Some(JvmStackValue::Int(
            get_clip_rect(objectref, jvm).x - get_translate_x(objectref, jvm),
        ))),
        ("getClipY", "()I") => Ok(Some(JvmStackValue::Int(
            get_clip_rect(objectref, jvm).y - get_translate_y(objectref, jvm),
        ))),
        ("getTranslateX", "()I") => Ok(Some(JvmStackValue::Int(get_translate_x(objectref, jvm)))),
        ("getTranslateY", "()I") => Ok(Some(JvmStackValue::Int(get_translate_y(objectref, jvm)))),
        ("getClipWidth", "()I") => Ok(Some(JvmStackValue::Int(
            get_clip_rect(objectref, jvm).width,
        ))),
        ("getClipHeight", "()I") => Ok(Some(JvmStackValue::Int(
            get_clip_rect(objectref, jvm).height,
        ))),
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

fn get_string_arg(
    args: &[JvmStackValue],
    index: usize,
    jvm: &JVM,
    context: &str,
) -> Result<String, String> {
    match args.get(index) {
        Some(JvmStackValue::String(value)) => Ok(value.clone()),
        Some(JvmStackValue::ObjectRef(id)) => {
            let state = jvm.state.lock();
            let Some(HeapObject::Instance(obj)) = state.heap.get(*id as usize) else {
                return Err(format!("{}: expected String object", context));
            };

            for field in ["value", "buffer", "text"] {
                if let Some(JvmStackValue::String(value)) = obj.fields.get(field) {
                    return Ok(value.clone());
                }
            }

            Ok(String::new())
        }
        Some(JvmStackValue::Null) => Err("java.lang.NullPointerException".into()),
        Some(value) => Err(format!("{}: expected String, found {:?}", context, value)),
        None => Err(format!("{}: missing String argument", context)),
    }
}

fn substring_chars(value: &str, offset: i32, len: i32, context: &str) -> Result<String, String> {
    if offset < 0 || len < 0 {
        return Err(format!(
            "java.lang.StringIndexOutOfBoundsException: {} offset {}, length {}",
            context, offset, len
        ));
    }

    let offset = offset as usize;
    let len = len as usize;
    let chars: Vec<char> = value.chars().collect();
    if offset > chars.len() || offset + len > chars.len() {
        return Err(format!(
            "java.lang.StringIndexOutOfBoundsException: {} offset {}, length {}, string length {}",
            context,
            offset,
            len,
            chars.len()
        ));
    }

    Ok(chars[offset..offset + len].iter().collect())
}

pub fn handle_font_static_method(
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("getDefaultFont", "()Ljavax/microedition/lcdui/Font;") => {
            Ok(Some(JvmStackValue::ObjectRef(allocate_font(jvm, 0, 0, 0))))
        }
        ("getFont", "(III)Ljavax/microedition/lcdui/Font;") => {
            let face = get_int_arg(args, 0)?;
            let style = get_int_arg(args, 1)?;
            let size = get_int_arg(args, 2)?;
            Ok(Some(JvmStackValue::ObjectRef(allocate_font(
                jvm, face, style, size,
            ))))
        }
        _ => Err(format!(
            "Unsupported Font static method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_font_virtual_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("getHeight", "()I") => Ok(Some(JvmStackValue::Int(get_font_height(objectref, jvm)))),
        ("charWidth", "(C)I") => Ok(Some(JvmStackValue::Int(font_char_width()))),
        ("stringWidth", "(Ljava/lang/String;)I") => {
            let text = get_string_arg(args, 0, jvm, "Font.stringWidth")?;
            Ok(Some(JvmStackValue::Int(text_width(&text))))
        }
        ("substringWidth", "(Ljava/lang/String;II)I") => {
            let text = get_string_arg(args, 0, jvm, "Font.substringWidth")?;
            let offset = get_int_arg(args, 1)?;
            let len = get_int_arg(args, 2)?;
            let substring = substring_chars(&text, offset, len, "Font.substringWidth")?;
            Ok(Some(JvmStackValue::Int(text_width(&substring))))
        }
        _ => Err(format!(
            "Unsupported Font instance method: {}{}",
            method_name, descriptor
        )),
    }
}

fn allocate_font(jvm: &JVM, face: i32, style: i32, size: i32) -> u32 {
    let mut fields = HashMap::new();
    fields.insert("face:I".to_string(), JvmStackValue::Int(face));
    fields.insert("style:I".to_string(), JvmStackValue::Int(style));
    fields.insert("size:I".to_string(), JvmStackValue::Int(size));
    fields.insert(
        "height:I".to_string(),
        JvmStackValue::Int(DEFAULT_FONT_HEIGHT),
    );

    let mut state = jvm.state.lock();
    JVM::allocate_internal(&mut state, FONT_CLASS_NAME.to_string(), fields)
}

fn get_font_height(objectref: &JvmStackValue, jvm: &JVM) -> i32 {
    let JvmStackValue::ObjectRef(id) = objectref else {
        return DEFAULT_FONT_HEIGHT;
    };

    let state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get(*id as usize) else {
        return DEFAULT_FONT_HEIGHT;
    };

    match obj.fields.get("height:I") {
        Some(JvmStackValue::Int(height)) => *height,
        _ => DEFAULT_FONT_HEIGHT,
    }
}

fn get_or_create_font(objectref: &JvmStackValue, jvm: &JVM) -> Result<u32, String> {
    let JvmStackValue::ObjectRef(graphics_id) = objectref else {
        return Err("Graphics.getFont: expected Graphics object".into());
    };

    {
        let state = jvm.state.lock();
        let Some(HeapObject::Instance(graphics)) = state.heap.get(*graphics_id as usize) else {
            return Err("Graphics.getFont: invalid Graphics object".into());
        };

        if let Some(JvmStackValue::ObjectRef(font_id)) =
            graphics.fields.get("font:Ljavax/microedition/lcdui/Font;")
        {
            if matches!(
                state.heap.get(*font_id as usize),
                Some(HeapObject::Instance(font)) if font.class_name == FONT_CLASS_NAME
            ) {
                return Ok(*font_id);
            }
        }
    }

    let font_id = allocate_font(jvm, 0, 0, 0);
    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(graphics)) = state.heap.get_mut(*graphics_id as usize) else {
        return Err("Graphics.getFont: invalid Graphics object".into());
    };
    graphics.fields.insert(
        "font:Ljavax/microedition/lcdui/Font;".to_string(),
        JvmStackValue::ObjectRef(font_id),
    );
    Ok(font_id)
}

fn set_font_field(
    objectref: &JvmStackValue,
    jvm: &JVM,
    font_ref: &JvmStackValue,
) -> Result<(), String> {
    let JvmStackValue::ObjectRef(graphics_id) = objectref else {
        return Err("Graphics.setFont: expected Graphics object".into());
    };

    let font_value = match font_ref {
        JvmStackValue::ObjectRef(id) => JvmStackValue::ObjectRef(*id),
        JvmStackValue::Null => JvmStackValue::ObjectRef(allocate_font(jvm, 0, 0, 0)),
        value => {
            return Err(format!(
                "Graphics.setFont: expected Font object, found {:?}",
                value
            ));
        }
    };

    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(graphics)) = state.heap.get_mut(*graphics_id as usize) else {
        return Err("Graphics.setFont: invalid Graphics object".into());
    };
    graphics.fields.insert(
        "font:Ljavax/microedition/lcdui/Font;".to_string(),
        font_value,
    );
    Ok(())
}

fn get_target_image_id(objectref: &JvmStackValue, jvm: &JVM) -> Option<usize> {
    let heap_idx = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return None,
    };

    let state = jvm.state.lock();
    let HeapObject::Instance(obj) = state.heap.get(heap_idx)? else {
        return None;
    };

    match obj.fields.get("targetImageId:I") {
        Some(JvmStackValue::Int(id)) => Some(*id as usize),
        _ => None,
    }
}

fn with_draw_target<R>(
    objectref: &JvmStackValue,
    jvm: &JVM,
    f: impl FnOnce(&mut [u8], i32, i32) -> R,
) -> Option<R> {
    if let Some(image_id) = get_target_image_id(objectref, jvm) {
        let image_ref = JvmStackValue::ObjectRef(image_id as u32);
        let buffer = get_or_create_buffer(&image_ref, jvm)?;
        let mut buffer = buffer.lock().unwrap();
        let width = buffer.width;
        let height = buffer.height;
        return Some(f(&mut buffer.pixels, width, height));
    }

    let mut draw_state = DRAW_STATE.lock();
    let width = draw_state.width as i32;
    let height = draw_state.height as i32;
    let pixels = draw_state.pixels.as_mut()?;
    let frame = pixels.frame_mut();
    Some(f(frame, width, height))
}

fn get_color(objectref: &JvmStackValue, jvm: &JVM) -> [u8; 4] {
    let c = get_color_int(objectref, jvm);
    [
        ((c >> 16) & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        (c & 0xFF) as u8,
        255,
    ]
}

fn get_color_int(objectref: &JvmStackValue, jvm: &JVM) -> i32 {
    let heap_idx = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        _ => return 0,
    };

    let state = jvm.state.lock();
    if let Some(HeapObject::Instance(obj)) = state.heap.get(heap_idx) {
        if let Some(JvmStackValue::Int(c)) = obj.fields.get("color") {
            return *c & 0x00FF_FFFF;
        }
    }
    0
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

fn get_target_dimensions(objectref: &JvmStackValue, jvm: &JVM) -> (i32, i32) {
    if let Some(image_id) = get_target_image_id(objectref, jvm) {
        return get_image_dim(&JvmStackValue::ObjectRef(image_id as u32), jvm);
    }

    let draw_state = DRAW_STATE.lock();
    (draw_state.width as i32, draw_state.height as i32)
}

fn full_target_clip(objectref: &JvmStackValue, jvm: &JVM) -> ClipRect {
    let (width, height) = get_target_dimensions(objectref, jvm);
    ClipRect {
        x: 0,
        y: 0,
        width: width.max(0),
        height: height.max(0),
    }
}

fn make_clip_rect(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> ClipRect {
    let translate_x = get_translate_x(objectref, jvm);
    let translate_y = get_translate_y(objectref, jvm);
    let requested = ClipRect {
        x: x.saturating_add(translate_x),
        y: y.saturating_add(translate_y),
        width: width.max(0),
        height: height.max(0),
    };

    requested.intersect(full_target_clip(objectref, jvm))
}

fn store_clip_rect(objectref: &JvmStackValue, jvm: &JVM, clip: ClipRect) {
    set_int_field(objectref, jvm, "clipX", clip.x);
    set_int_field(objectref, jvm, "clipY", clip.y);
    set_int_field(objectref, jvm, "clipWidth", clip.width);
    set_int_field(objectref, jvm, "clipHeight", clip.height);
}

fn set_clip_rect(objectref: &JvmStackValue, jvm: &JVM, x: i32, y: i32, width: i32, height: i32) {
    let clip = make_clip_rect(objectref, jvm, x, y, width, height);
    store_clip_rect(objectref, jvm, clip);
}

fn intersect_clip_rect(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    let requested = make_clip_rect(objectref, jvm, x, y, width, height);
    let current = get_clip_rect(objectref, jvm);
    store_clip_rect(objectref, jvm, current.intersect(requested));
}

fn get_clip_rect(objectref: &JvmStackValue, jvm: &JVM) -> ClipRect {
    let default_clip = full_target_clip(objectref, jvm);
    let x = get_int_field(objectref, jvm, "clipX", default_clip.x);
    let y = get_int_field(objectref, jvm, "clipY", default_clip.y);
    let width = get_int_field(objectref, jvm, "clipWidth", default_clip.width);
    let height = get_int_field(objectref, jvm, "clipHeight", default_clip.height);

    ClipRect {
        x,
        y,
        width: width.max(0),
        height: height.max(0),
    }
    .intersect(default_clip)
}

fn get_translate_x(objectref: &JvmStackValue, jvm: &JVM) -> i32 {
    get_int_field(objectref, jvm, "translateX", 0)
}

fn get_translate_y(objectref: &JvmStackValue, jvm: &JVM) -> i32 {
    get_int_field(objectref, jvm, "translateY", 0)
}

fn read_rgb_int_array(rgb_ref: &JvmStackValue, jvm: &JVM) -> Result<Vec<i32>, String> {
    let rgb_id = match rgb_ref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        JvmStackValue::Null => return Err("Graphics.drawRGB: rgbData is null".into()),
        value => {
            return Err(format!(
                "Graphics.drawRGB: expected int[] reference, found {:?}",
                value
            ));
        }
    };

    let state = jvm.state.lock();
    match state.heap.get(rgb_id) {
        Some(HeapObject::Array { element_type, data }) => {
            if element_type != "primitive_10" {
                return Err(format!(
                    "Graphics.drawRGB: expected int array, found array of type {}",
                    element_type
                ));
            }

            data.iter()
                .map(|value| match value {
                    JvmStackValue::Int(argb) => Ok(*argb),
                    value => Err(format!(
                        "Graphics.drawRGB: expected int pixel, found {:?}",
                        value
                    )),
                })
                .collect()
        }
        Some(_) => Err("Graphics.drawRGB: rgbData is not an array".into()),
        None => Err(format!(
            "Graphics.drawRGB: invalid rgb array reference {}",
            rgb_id
        )),
    }
}

fn validate_draw_rgb_bounds(
    offset: i32,
    scanline: i32,
    width: i32,
    height: i32,
    len: usize,
) -> Result<(), String> {
    let len = len as i64;

    for row in 0..height {
        let row_start = offset as i64 + row as i64 * scanline as i64;
        for col in 0..width {
            let index = row_start + col as i64;
            if index < 0 || index >= len {
                return Err(format!(
                    "java.lang.ArrayIndexOutOfBoundsException: rgbData index {} out of bounds for length {}",
                    index,
                    len
                ));
            }
        }
    }

    Ok(())
}

fn draw_rgb(
    objectref: &JvmStackValue,
    jvm: &JVM,
    rgb_values: &[i32],
    offset: i32,
    scanline: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    process_alpha: bool,
) {
    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
        let mut row_start = offset as i64;

        for row in 0..height {
            let dest_y = y + row;
            if dest_y < 0 || dest_y >= dh || dest_y < clip.y || dest_y >= clip.bottom() {
                row_start += scanline as i64;
                continue;
            }

            let mut src_index = row_start;
            for col in 0..width {
                let dest_x = x + col;
                if dest_x < 0 || dest_x >= dw || !clip.contains(dest_x, dest_y) {
                    src_index += 1;
                    continue;
                }

                let rgb_index = src_index as usize;
                if let Some(argb) = rgb_values.get(rgb_index) {
                    let alpha = if process_alpha {
                        ((argb >> 24) & 0xFF) as u8
                    } else {
                        0xFF
                    };

                    if alpha != 0 {
                        let offset = ((dest_y * dw + dest_x) * 4) as usize;
                        if offset + 4 <= frame.len() {
                            let src = [
                                ((argb >> 16) & 0xFF) as u8,
                                ((argb >> 8) & 0xFF) as u8,
                                (argb & 0xFF) as u8,
                                0xFF,
                            ];

                            if alpha == 255 {
                                frame[offset..offset + 4].copy_from_slice(&src);
                            } else {
                                let alpha_u32 = alpha as u32;
                                let inv_alpha = 255 - alpha_u32;

                                for channel in 0..3 {
                                    let src_c = src[channel] as u32;
                                    let dest_c = frame[offset + channel] as u32;
                                    frame[offset + channel] =
                                        ((src_c * alpha_u32 + dest_c * inv_alpha) / 255) as u8;
                                }

                                frame[offset + 3] = 255;
                            }
                        }
                    }
                }

                src_index += 1;
            }

            row_start += scanline as i64;
        }
    });
}

fn put_pixel(frame: &mut [u8], dw: i32, dh: i32, clip: ClipRect, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || x >= dw || y < 0 || y >= dh || !clip.contains(x, y) {
        return;
    }

    let offset = ((y * dw + x) * 4) as usize;
    if offset + 4 <= frame.len() {
        frame[offset..offset + 4].copy_from_slice(&color);
    }
}

fn fill_rect(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: [u8; 4],
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
        let x_end = x.saturating_add(width);
        let y_end = y.saturating_add(height);
        let start_y = y.max(0).max(clip.y);
        let end_y = y_end.min(dh).min(clip.bottom());
        let start_x = x.max(0).max(clip.x);
        let end_x = x_end.min(dw).min(clip.right());

        for iy in start_y..end_y {
            for ix in start_x..end_x {
                put_pixel(frame, dw, dh, clip, ix, iy, color);
            }
        }
    });
}

fn draw_rect(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: [u8; 4],
    dotted: bool,
) {
    draw_line(objectref, jvm, x, y, x + width, y, color, dotted);
    draw_line(
        objectref,
        jvm,
        x + width,
        y,
        x + width,
        y + height,
        color,
        dotted,
    );
    draw_line(
        objectref,
        jvm,
        x + width,
        y + height,
        x,
        y + height,
        color,
        dotted,
    );
    draw_line(objectref, jvm, x, y + height, x, y, color, dotted);
}

fn draw_line(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: [u8; 4],
    dotted: bool,
) {
    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
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
                put_pixel(frame, dw, dh, clip, x, y, color);
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
    });
}

fn draw_region(
    objectref: &JvmStackValue,
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
    let img_buffer = match get_or_create_buffer(img_ref, jvm) {
        Some(buf) => buf,
        None => {
            panic!("draw_region: failed to load image resource");
        }
    };
    let img_guard = img_buffer.lock().unwrap();
    let img_w = img_guard.width;
    let img_h = img_guard.height;
    let img_pixels = &img_guard.pixels;

    let (dest_w, dest_h) = match transform {
        4 | 5 | 6 | 7 => (height, width),
        _ => (width, height),
    };

    let mut real_x = x_dest;
    let mut real_y = y_dest;

    if anchor & 1 != 0 {
        // HCENTER
        real_x -= dest_w / 2;
    } else if anchor & 8 != 0 {
        // RIGHT
        real_x -= dest_w;
    }

    if anchor & 2 != 0 {
        // VCENTER
        real_y -= dest_h / 2;
    } else if anchor & 32 != 0 {
        // BOTTOM
        real_y -= dest_h;
    }

    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
        let src_stride = (img_w * 4) as usize;

        for dy in 0..dest_h {
            let dest_px_y = real_y + dy;
            if dest_px_y < 0 || dest_px_y >= dh || dest_px_y < clip.y || dest_px_y >= clip.bottom()
            {
                continue;
            }

            for dx in 0..dest_w {
                let dest_px_x = real_x + dx;
                if dest_px_x < 0 || dest_px_x >= dw || !clip.contains(dest_px_x, dest_px_y) {
                    continue;
                }

                let (sx, sy) = match transform {
                    0 => (dx, dy),                          // TRANS_NONE
                    1 => (dx, height - 1 - dy),             // TRANS_MIRROR_ROT180 (Vertical Flip)
                    2 => (width - 1 - dx, dy),              // TRANS_MIRROR (Horizontal Flip)
                    3 => (width - 1 - dx, height - 1 - dy), // TRANS_ROT180
                    4 => (dy, dx),                          // TRANS_MIRROR_ROT270
                    5 => (dy, height - 1 - dx),             // TRANS_ROT90 (90 deg CW)
                    6 => (width - 1 - dy, dx),              // TRANS_ROT270 (270 deg CW)
                    7 => (width - 1 - dy, height - 1 - dx), // TRANS_MIRROR_ROT90
                    _ => (dx, dy),                          // Fallback safety
                };

                let src_px_x = x_src + sx;
                let src_px_y = y_src + sy;

                if src_px_x < 0 || src_px_x >= img_w || src_px_y < 0 || src_px_y >= img_h {
                    continue;
                }

                let src_offset = (src_px_y as usize * src_stride) + (src_px_x as usize * 4);
                if src_offset + 4 > img_pixels.len() {
                    continue;
                }

                let px = &img_pixels[src_offset..src_offset + 4];
                let alpha = px[3];

                if alpha == 0 {
                    continue; // Fully transparent
                }

                let offset = ((dest_px_y * dw + dest_px_x) * 4) as usize;
                if offset + 4 <= frame.len() {
                    if alpha == 255 {
                        // opaque
                        frame[offset..offset + 4].copy_from_slice(px);
                    } else {
                        let alpha_u32 = alpha as u32;
                        let inv_alpha = 255 - alpha_u32;

                        for i in 0..3 {
                            let src_c = px[i] as u32;
                            let dest_c = frame[offset + i] as u32;

                            // alpha blending
                            frame[offset + i] =
                                ((src_c * alpha_u32 + dest_c * inv_alpha) / 255) as u8;
                        }

                        frame[offset + 3] = 255;
                    }
                }
            }
        }
    });
}

fn fill_triangle(
    objectref: &JvmStackValue,
    jvm: &JVM,
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

    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
        if y2 == y3 {
            fill_bottom_flat_triangle(x1, y1, x2, y2, x3, y3, dw, dh, clip, frame, color);
        } else if y1 == y2 {
            fill_top_flat_triangle(x1, y1, x2, y2, x3, y3, dw, dh, clip, frame, color);
        } else {
            let x4 = x1 + ((y2 - y1) as f32 * (x3 - x1) as f32 / (y3 - y1) as f32) as i32;
            fill_bottom_flat_triangle(x1, y1, x2, y2, x4, y2, dw, dh, clip, frame, color);
            fill_top_flat_triangle(x2, y2, x4, y2, x3, y3, dw, dh, clip, frame, color);
        }
    });
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
    clip: ClipRect,
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
        draw_horizontal_line(
            curx1 as i32,
            curx2 as i32,
            scanline_y,
            dw,
            dh,
            clip,
            frame,
            color,
        );
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
    clip: ClipRect,
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
        draw_horizontal_line(
            curx1 as i32,
            curx2 as i32,
            scanline_y,
            dw,
            dh,
            clip,
            frame,
            color,
        );
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
    clip: ClipRect,
    frame: &mut [u8],
    color: [u8; 4],
) {
    if y < 0 || y >= dh || y < clip.y || y >= clip.bottom() {
        return;
    }
    if x1 > x2 {
        std::mem::swap(&mut x1, &mut x2);
    }

    // Add 1 to x2 to make it inclusive if needed, but J2ME fill usually includes the last pixel
    for x in x1.max(0).max(clip.x)..=x2.min(dw - 1).min(clip.right() - 1) {
        put_pixel(frame, dw, dh, clip, x, y, color);
    }
}

fn draw_arc(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    start_angle: i32,
    arc_angle: i32,
    color: [u8; 4],
) {
    if width <= 0 || height <= 0 || arc_angle == 0 {
        return;
    }

    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
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

            put_pixel(frame, dw, dh, clip, px, py, color);
        }
    });
}

fn fill_arc(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    start_angle: i32,
    arc_angle: i32,
    color: [u8; 4],
) {
    if width <= 0 || height <= 0 || arc_angle == 0 {
        return;
    }

    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
        let cx = x as f32 + width as f32 / 2.0;
        let cy = y as f32 + height as f32 / 2.0;
        let rx = width as f32 / 2.0;
        let ry = height as f32 / 2.0;

        let start_y = y.max(0).max(clip.y);
        let end_y = y.saturating_add(height).min(dh).min(clip.bottom());
        let start_x = x.max(0).max(clip.x);
        let end_x = x.saturating_add(width).min(dw).min(clip.right());

        for py in start_y..end_y {
            for px in start_x..end_x {
                let dx = (px as f32 + 0.5 - cx) / rx;
                let dy = (py as f32 + 0.5 - cy) / ry;
                if dx * dx + dy * dy > 1.0 {
                    continue;
                }

                let angle = dy.atan2(dx).to_degrees();
                if angle_in_arc(angle, start_angle, arc_angle) {
                    put_pixel(frame, dw, dh, clip, px, py, color);
                }
            }
        }
    });
}

fn draw_round_rect(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    arc_width: i32,
    arc_height: i32,
    color: [u8; 4],
    dotted: bool,
) {
    if width < 0 || height < 0 {
        return;
    }

    let arc_width = arc_width.max(0).min(width);
    let arc_height = arc_height.max(0).min(height);
    if arc_width == 0 || arc_height == 0 {
        draw_rect(objectref, jvm, x, y, width, height, color, dotted);
        return;
    }

    let rx = arc_width / 2;
    let ry = arc_height / 2;
    let right = x + width;
    let bottom = y + height;

    draw_line(objectref, jvm, x + rx, y, right - rx, y, color, dotted);
    draw_line(
        objectref,
        jvm,
        x + rx,
        bottom,
        right - rx,
        bottom,
        color,
        dotted,
    );
    draw_line(objectref, jvm, x, y + ry, x, bottom - ry, color, dotted);
    draw_line(
        objectref,
        jvm,
        right,
        y + ry,
        right,
        bottom - ry,
        color,
        dotted,
    );

    draw_arc(objectref, jvm, x, y, arc_width, arc_height, 180, 90, color);
    draw_arc(
        objectref,
        jvm,
        right - arc_width,
        y,
        arc_width,
        arc_height,
        270,
        90,
        color,
    );
    draw_arc(
        objectref,
        jvm,
        right - arc_width,
        bottom - arc_height,
        arc_width,
        arc_height,
        0,
        90,
        color,
    );
    draw_arc(
        objectref,
        jvm,
        x,
        bottom - arc_height,
        arc_width,
        arc_height,
        90,
        90,
        color,
    );
}

fn fill_round_rect(
    objectref: &JvmStackValue,
    jvm: &JVM,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    arc_width: i32,
    arc_height: i32,
    color: [u8; 4],
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let rx = (arc_width.max(0).min(width) / 2).max(0);
    let ry = (arc_height.max(0).min(height) / 2).max(0);
    if rx == 0 || ry == 0 {
        fill_rect(objectref, jvm, x, y, width, height, color);
        return;
    }

    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
        let start_y = y.max(0).max(clip.y);
        let end_y = y.saturating_add(height).min(dh).min(clip.bottom());
        let start_x = x.max(0).max(clip.x);
        let end_x = x.saturating_add(width).min(dw).min(clip.right());

        for py in start_y..end_y {
            for px in start_x..end_x {
                let local_x = px - x;
                let local_y = py - y;
                if rounded_rect_contains(local_x, local_y, width, height, rx, ry) {
                    put_pixel(frame, dw, dh, clip, px, py, color);
                }
            }
        }
    });
}

fn rounded_rect_contains(px: i32, py: i32, width: i32, height: i32, rx: i32, ry: i32) -> bool {
    let cx = if px < rx {
        rx
    } else if px >= width - rx {
        width - rx - 1
    } else {
        return true;
    };

    let cy = if py < ry {
        ry
    } else if py >= height - ry {
        height - ry - 1
    } else {
        return true;
    };

    let dx = (px - cx) as f32 / rx.max(1) as f32;
    let dy = (py - cy) as f32 / ry.max(1) as f32;
    dx * dx + dy * dy <= 1.0
}

fn angle_in_arc(angle: f32, start_angle: i32, arc_angle: i32) -> bool {
    if arc_angle.abs() >= 360 {
        return true;
    }

    let angle = normalize_degrees(angle);
    let start = normalize_degrees(start_angle as f32);

    if arc_angle > 0 {
        let delta = normalize_degrees(angle - start);
        delta <= arc_angle as f32
    } else {
        let delta = normalize_degrees(start - angle);
        delta <= (-arc_angle) as f32
    }
}

fn normalize_degrees(angle: f32) -> f32 {
    angle.rem_euclid(360.0)
}

fn draw_text(
    objectref: &JvmStackValue,
    jvm: &JVM,
    text: &str,
    x: i32,
    y: i32,
    anchor: i32,
    color: [u8; 4],
) {
    if text.is_empty() {
        return;
    }

    let (x, y) = text_anchor_to_top_left(x, y, text_width(text), DEFAULT_FONT_HEIGHT, anchor);
    let clip = get_clip_rect(objectref, jvm);
    let _ = with_draw_target(objectref, jvm, |frame, dw, dh| {
        let mut cursor_x = x;
        for ch in text.chars() {
            draw_glyph(frame, dw, dh, clip, cursor_x, y, ch, color);
            cursor_x += FONT_ADVANCE;
        }
    });
}

fn text_anchor_to_top_left(x: i32, y: i32, width: i32, height: i32, anchor: i32) -> (i32, i32) {
    let mut x = x;
    let mut y = y;

    if anchor & 1 != 0 {
        x -= width / 2;
    } else if anchor & 8 != 0 {
        x -= width;
    }

    if anchor & 2 != 0 {
        y -= height / 2;
    } else if anchor & 32 != 0 {
        y -= height;
    } else if anchor & 64 != 0 {
        y -= height - 3;
    }

    (x, y)
}

fn text_width(text: &str) -> i32 {
    text.chars().count() as i32 * FONT_ADVANCE
}

fn font_char_width() -> i32 {
    FONT_ADVANCE
}

fn draw_glyph(
    frame: &mut [u8],
    dw: i32,
    dh: i32,
    clip: ClipRect,
    x: i32,
    y: i32,
    ch: char,
    color: [u8; 4],
) {
    let rows = glyph_rows(ch);
    for (row, bits) in rows.iter().enumerate() {
        for col in 0..GLYPH_WIDTH {
            let mask = 1 << (GLYPH_WIDTH - 1 - col);
            if bits & mask == 0 {
                continue;
            }

            for sy in 0..FONT_SCALE {
                for sx in 0..FONT_SCALE {
                    put_pixel(
                        frame,
                        dw,
                        dh,
                        clip,
                        x + col * FONT_SCALE + sx,
                        y + row as i32 * FONT_SCALE + sy,
                        color,
                    );
                }
            }
        }
    }
}

fn glyph_rows(ch: char) -> [u8; GLYPH_HEIGHT as usize] {
    match ch.to_ascii_uppercase() {
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11110, 0b00000, 0b00000, 0b00000,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        ':' => [
            0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        ' ' => [0; GLYPH_HEIGHT as usize],
        _ => [
            0b11111, 0b10001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
    }
}
