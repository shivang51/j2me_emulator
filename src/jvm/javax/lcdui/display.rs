use std::collections::HashMap;

use crate::jvm::jvm_core::{HeapObject, JVM, JvmObject, JvmStackValue};

const CLASS_NAME: &str = "javax/microedition/lcdui/Display";
const SINGLETON_FIELD: &str =
    "javax/microedition/lcdui/Display.singleton:Ljavax/microedition/lcdui/Display;";
const CURRENT_FIELD: &str = "current:Ljavax/microedition/lcdui/Displayable;";

pub fn handle_static_method(
    method_name: &str,
    descriptor: &str,
    _args: &[JvmStackValue],
    jvm: &mut JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        (
            "getDisplay",
            "(Ljavax/microedition/midlet/MIDlet;)Ljavax/microedition/lcdui/Display;",
        ) => Ok(Some(get_display(jvm))),
        _ => Err(format!(
            "Unsupported Display static method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_virtual_method(
    objectref: JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &mut JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("setCurrent", "(Ljavax/microedition/lcdui/Displayable;)V") => {
            set_current(objectref, args, jvm)?;
            Ok(None)
        }
        ("getCurrent", "()Ljavax/microedition/lcdui/Displayable;") => {
            let display = get_display_object(objectref, jvm)?;
            let HeapObject::Instance(obj) = display else {
                return Err("Display: expected instance object".into());
            };

            Ok(Some(
                obj.fields
                    .get(CURRENT_FIELD)
                    .cloned()
                    .unwrap_or(JvmStackValue::Null),
            ))
        }
        _ => Err(format!(
            "Unsupported Display instance method: {}{}",
            method_name, descriptor
        )),
    }
}

fn get_display(jvm: &mut JVM) -> JvmStackValue {
    if let Some(existing) = jvm.static_fields.get(SINGLETON_FIELD) {
        return existing.clone();
    }

    let mut fields = HashMap::new();
    fields.insert(CURRENT_FIELD.to_string(), JvmStackValue::Null);

    jvm.heap.push(HeapObject::Instance(JvmObject {
        class_name: CLASS_NAME.to_string(),
        fields,
    }));

    let objectref = JvmStackValue::ObjectRef((jvm.heap.len() - 1) as u32);
    jvm.static_fields
        .insert(SINGLETON_FIELD.to_string(), objectref.clone());

    objectref
}

fn set_current(
    objectref: JvmStackValue,
    args: &[JvmStackValue],
    jvm: &mut JVM,
) -> Result<(), String> {
    let current = args
        .first()
        .cloned()
        .ok_or_else(|| "Display.setCurrent: missing Displayable argument".to_string())?;
    let display = get_display_object(objectref, jvm)?;

    let HeapObject::Instance(obj) = display else {
        return Err("Display: expected instance object".into());
    };

    obj.fields.insert(CURRENT_FIELD.to_string(), current);

    Ok(())
}

fn get_display_object(objectref: JvmStackValue, jvm: &mut JVM) -> Result<&mut HeapObject, String> {
    let display_id = match objectref {
        JvmStackValue::ObjectRef(id) => id,
        JvmStackValue::Null => return Err("Display: NullPointerException".into()),
        value => {
            return Err(format!(
                "Display: expected object reference, found {:?}",
                value
            ));
        }
    };

    jvm.heap
        .get_mut(display_id as usize)
        .ok_or_else(|| format!("Display: invalid heap reference: {}", display_id))
}
