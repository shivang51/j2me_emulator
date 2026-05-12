use std::collections::HashMap;

use crate::jvm::jvm_core::{HeapObject, JVM, JvmObject, JvmStackValue, JvmState};

pub const CLASS_NAME: &str = "javax/microedition/lcdui/Display";
const SINGLETON_FIELD: &str =
    "javax/microedition/lcdui/Display.singleton:Ljavax/microedition/lcdui/Display;";
const CURRENT_FIELD: &str = "current:Ljavax/microedition/lcdui/Displayable;";

pub fn handle_static_method(
    method_name: &str,
    descriptor: &str,
    _args: &[JvmStackValue],
    jvm: &JVM,
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
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("setCurrent", "(Ljavax/microedition/lcdui/Displayable;)V") => {
            set_current(objectref, args, jvm)?;
            Ok(None)
        }
        ("getCurrent", "()Ljavax/microedition/lcdui/Displayable;") => {
            return get_displayable_obj(objectref, jvm);
        }
        _ => Err(format!(
            "Unsupported Display instance method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn get_displayable_obj(
    objectref: JvmStackValue,
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    get_displayable_obj_safe(objectref, jvm)
}

pub fn get_displayable_obj_safe(
    objectref: JvmStackValue,
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
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

    let state = jvm.state.lock();
    let display = state
        .heap
        .get(display_id as usize)
        .ok_or_else(|| format!("Display: invalid heap reference: {}", display_id))?;

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

pub fn get_display(jvm: &JVM) -> JvmStackValue {
    let mut state = jvm.state.lock();

    if let Some(existing) = state.static_fields.get(SINGLETON_FIELD) {
        return existing.clone();
    }

    let mut fields = HashMap::new();
    fields.insert(CURRENT_FIELD.to_string(), JvmStackValue::Null);

    state.heap.push(HeapObject::Instance(JvmObject {
        class_name: CLASS_NAME.to_string(),
        fields,
    }));

    let objectref = JvmStackValue::ObjectRef((state.heap.len() - 1) as u32);
    state
        .static_fields
        .insert(SINGLETON_FIELD.to_string(), objectref.clone());

    objectref
}

pub fn get_display_safe(jvm: &JVM) -> Result<JvmStackValue, String> {
    let mut state = jvm.state.lock();

    if let Some(existing) = state.static_fields.get(SINGLETON_FIELD) {
        return Ok(existing.clone());
    }

    let mut fields = HashMap::new();
    fields.insert(CURRENT_FIELD.to_string(), JvmStackValue::Null);

    state.heap.push(HeapObject::Instance(JvmObject {
        class_name: CLASS_NAME.to_string(),
        fields,
    }));

    let objectref = JvmStackValue::ObjectRef((state.heap.len() - 1) as u32);
    state
        .static_fields
        .insert(SINGLETON_FIELD.to_string(), objectref.clone());

    Ok(objectref)
}

fn set_current(objectref: JvmStackValue, args: &[JvmStackValue], jvm: &JVM) -> Result<(), String> {
    let current = args
        .first()
        .cloned()
        .ok_or_else(|| "Display.setCurrent: missing Displayable argument".to_string())?;

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

    let mut state = jvm.state.lock();
    let display = state
        .heap
        .get_mut(display_id as usize)
        .ok_or_else(|| format!("Display: invalid heap reference: {}", display_id))?;

    let HeapObject::Instance(obj) = display else {
        return Err("Display: expected instance object".into());
    };

    obj.fields.insert(CURRENT_FIELD.to_string(), current);

    Ok(())
}
