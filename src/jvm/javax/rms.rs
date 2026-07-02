use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use std::collections::hash_map::DefaultHasher;

use crate::jvm::jvm_core::{HeapObject, JVM, JvmObject, JvmStackValue};

pub const RECORD_STORE_CLASS_NAME: &str = "javax/microedition/rms/RecordStore";
pub const RECORD_ENUMERATION_CLASS_NAME: &str = "javax/microedition/rms/RecordEnumeration";

const STORE_MAGIC: &[u8; 8] = b"J2MERS1\0";
const MAX_STORE_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone)]
struct StoredRecordStore {
    name: String,
    version: i32,
    next_id: i32,
    last_modified: i64,
    records: BTreeMap<i32, Vec<u8>>,
}

impl StoredRecordStore {
    fn new(name: String) -> Self {
        Self {
            name,
            version: 0,
            next_id: 1,
            last_modified: current_time_millis(),
            records: BTreeMap::new(),
        }
    }

    fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.last_modified = current_time_millis();
    }

    fn size(&self) -> usize {
        self.records.values().map(Vec::len).sum()
    }
}

static RMS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn handle_static_method(
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("openRecordStore", "(Ljava/lang/String;Z)Ljavax/microedition/rms/RecordStore;") => {
            let name = get_string_arg(args, 0, "RecordStore.openRecordStore name")?;
            let create = get_bool_arg(args, 1, "RecordStore.openRecordStore create")?;
            open_record_store(jvm, own_namespace(jvm), name, create, 0, true).map(Some)
        }
        ("openRecordStore", "(Ljava/lang/String;ZIZ)Ljavax/microedition/rms/RecordStore;") => {
            let name = get_string_arg(args, 0, "RecordStore.openRecordStore name")?;
            let create = get_bool_arg(args, 1, "RecordStore.openRecordStore create")?;
            let authmode = get_int_arg(args, 2, "RecordStore.openRecordStore authmode")?;
            let _shared_writable = get_bool_arg(args, 3, "RecordStore.openRecordStore writable")?;
            open_record_store(jvm, own_namespace(jvm), name, create, authmode, true).map(Some)
        }
        (
            "openRecordStore",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljavax/microedition/rms/RecordStore;",
        ) => {
            let name = get_string_arg(args, 0, "RecordStore.openRecordStore name")?;
            let vendor = get_string_arg(args, 1, "RecordStore.openRecordStore vendor")?;
            let suite = get_string_arg(args, 2, "RecordStore.openRecordStore suite")?;
            open_record_store(jvm, shared_namespace(vendor, suite), name, false, 0, false).map(Some)
        }
        ("deleteRecordStore", "(Ljava/lang/String;)V") => {
            let name = get_string_arg(args, 0, "RecordStore.deleteRecordStore name")?;
            delete_record_store(&own_namespace(jvm), name)?;
            Ok(None)
        }
        ("listRecordStores", "()[Ljava/lang/String;") => {
            let names = list_record_stores(&own_namespace(jvm))?;
            if names.is_empty() {
                Ok(Some(JvmStackValue::Null))
            } else {
                Ok(Some(allocate_string_array(jvm, names)))
            }
        }
        _ => Err(format!(
            "Unsupported RecordStore static method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_record_store_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("closeRecordStore", "()V") => {
            set_store_open(objectref, jvm, false)?;
            Ok(None)
        }
        ("getName", "()Ljava/lang/String;") => {
            let (_, _, name) = record_store_identity(objectref, jvm, false)?;
            Ok(Some(JvmStackValue::String(name)))
        }
        ("getNumRecords", "()I") => {
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            Ok(Some(JvmStackValue::Int(store.records.len() as i32)))
        }
        ("getNextRecordID", "()I") => {
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            Ok(Some(JvmStackValue::Int(store.next_id)))
        }
        ("getVersion", "()I") => {
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            Ok(Some(JvmStackValue::Int(store.version)))
        }
        ("getLastModified", "()J") => {
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            Ok(Some(JvmStackValue::Long(store.last_modified)))
        }
        ("getSize", "()I") => {
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            Ok(Some(JvmStackValue::Int(store.size() as i32)))
        }
        ("getSizeAvailable", "()I") => {
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            let available = MAX_STORE_SIZE.saturating_sub(store.size());
            Ok(Some(JvmStackValue::Int(available as i32)))
        }
        ("getRecordSize", "(I)I") => {
            let record_id = get_int_arg(args, 0, "RecordStore.getRecordSize recordId")?;
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            let record = get_record(&store, record_id)?;
            Ok(Some(JvmStackValue::Int(record.len() as i32)))
        }
        ("getRecord", "(I)[B") => {
            let record_id = get_int_arg(args, 0, "RecordStore.getRecord recordId")?;
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            let record = get_record(&store, record_id)?;
            Ok(Some(allocate_byte_array(jvm, record)))
        }
        ("getRecord", "(I[BI)I") => {
            let record_id = get_int_arg(args, 0, "RecordStore.getRecord recordId")?;
            let offset = get_int_arg(args, 2, "RecordStore.getRecord offset")?;
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let store = load_required_store(&namespace, &name)?;
            let record = get_record(&store, record_id)?;
            write_record_into_byte_array(jvm, args.get(1), offset, record)?;
            Ok(Some(JvmStackValue::Int(record.len() as i32)))
        }
        ("addRecord", "([BII)I") => {
            let offset = get_int_arg(args, 1, "RecordStore.addRecord offset")?;
            let num_bytes = get_int_arg(args, 2, "RecordStore.addRecord numBytes")?;
            let data = read_record_bytes(jvm, args.first(), offset, num_bytes)?;
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            ensure_store_writable(objectref, jvm)?;

            let _guard = rms_guard();
            let mut store = load_required_store(&namespace, &name)?;
            let record_id = store.next_id;
            if record_id == i32::MAX {
                return Err(
                    "javax.microedition.rms.RecordStoreFullException: no record ids left"
                        .to_string(),
                );
            }
            store.next_id = record_id + 1;
            store.records.insert(record_id, data);
            store.touch();
            ensure_store_size(&store)?;
            save_store(&namespace, &store)?;
            Ok(Some(JvmStackValue::Int(record_id)))
        }
        ("setRecord", "(I[BII)V") => {
            let record_id = get_int_arg(args, 0, "RecordStore.setRecord recordId")?;
            let offset = get_int_arg(args, 2, "RecordStore.setRecord offset")?;
            let num_bytes = get_int_arg(args, 3, "RecordStore.setRecord numBytes")?;
            let data = read_record_bytes(jvm, args.get(1), offset, num_bytes)?;
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            ensure_store_writable(objectref, jvm)?;

            let _guard = rms_guard();
            let mut store = load_required_store(&namespace, &name)?;
            if !store.records.contains_key(&record_id) {
                return Err(invalid_record_id(record_id));
            }
            store.records.insert(record_id, data);
            store.touch();
            ensure_store_size(&store)?;
            save_store(&namespace, &store)?;
            Ok(None)
        }
        ("deleteRecord", "(I)V") => {
            let record_id = get_int_arg(args, 0, "RecordStore.deleteRecord recordId")?;
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            ensure_store_writable(objectref, jvm)?;

            let _guard = rms_guard();
            let mut store = load_required_store(&namespace, &name)?;
            if store.records.remove(&record_id).is_none() {
                return Err(invalid_record_id(record_id));
            }
            store.touch();
            save_store(&namespace, &store)?;
            Ok(None)
        }
        (
            "enumerateRecords",
            "(Ljavax/microedition/rms/RecordFilter;Ljavax/microedition/rms/RecordComparator;Z)Ljavax/microedition/rms/RecordEnumeration;",
        ) => {
            let (_, namespace, name) = record_store_identity(objectref, jvm, true)?;
            let filter = args.first().cloned().unwrap_or(JvmStackValue::Null);
            let comparator = args.get(1).cloned().unwrap_or(JvmStackValue::Null);
            let keep_updated = get_bool_arg(args, 2, "RecordStore.enumerateRecords keepUpdated")?;
            let enum_ref = allocate_record_enumeration(
                jvm,
                namespace,
                name,
                filter,
                comparator,
                keep_updated,
            )?;
            Ok(Some(enum_ref))
        }
        ("setMode", "(IZ)V") => {
            let authmode = get_int_arg(args, 0, "RecordStore.setMode authmode")?;
            let writable = get_bool_arg(args, 1, "RecordStore.setMode writable")?;
            set_store_mode(objectref, jvm, authmode, writable)?;
            Ok(None)
        }
        _ => Err(format!(
            "Unsupported RecordStore instance method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_record_enumeration_method(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    match (method_name, descriptor) {
        ("numRecords", "()I") => {
            refresh_if_kept_updated(objectref, jvm)?;
            let (_, ids, _, _) = enumeration_state(objectref, jvm)?;
            Ok(Some(JvmStackValue::Int(ids.len() as i32)))
        }
        ("hasNextElement", "()Z") | ("hasMoreElements", "()Z") => {
            refresh_if_kept_updated(objectref, jvm)?;
            let (_, ids, index, _) = enumeration_state(objectref, jvm)?;
            Ok(Some(JvmStackValue::Int(if index < ids.len() {
                1
            } else {
                0
            })))
        }
        ("hasPreviousElement", "()Z") => {
            refresh_if_kept_updated(objectref, jvm)?;
            let (_, _, index, _) = enumeration_state(objectref, jvm)?;
            Ok(Some(JvmStackValue::Int(if index > 0 { 1 } else { 0 })))
        }
        ("nextRecordId", "()I") => {
            refresh_if_kept_updated(objectref, jvm)?;
            let id = take_enumeration_id(objectref, jvm, true)?;
            Ok(Some(JvmStackValue::Int(id)))
        }
        ("previousRecordId", "()I") => {
            refresh_if_kept_updated(objectref, jvm)?;
            let id = take_enumeration_id(objectref, jvm, false)?;
            Ok(Some(JvmStackValue::Int(id)))
        }
        ("nextRecord", "()[B") | ("nextElement", "()Ljava/lang/Object;") => {
            refresh_if_kept_updated(objectref, jvm)?;
            let id = take_enumeration_id(objectref, jvm, true)?;
            let record = enumeration_record_bytes(objectref, jvm, id)?;
            Ok(Some(allocate_byte_array(jvm, &record)))
        }
        ("previousRecord", "()[B") => {
            refresh_if_kept_updated(objectref, jvm)?;
            let id = take_enumeration_id(objectref, jvm, false)?;
            let record = enumeration_record_bytes(objectref, jvm, id)?;
            Ok(Some(allocate_byte_array(jvm, &record)))
        }
        ("reset", "()V") => {
            set_enumeration_index(objectref, jvm, 0)?;
            Ok(None)
        }
        ("rebuild", "()V") => {
            rebuild_enumeration(objectref, jvm, true)?;
            Ok(None)
        }
        ("destroy", "()V") => {
            set_enumeration_destroyed(objectref, jvm)?;
            Ok(None)
        }
        ("keepUpdated", "(Z)V") => {
            Err("RecordEnumeration.keepUpdated(Z): missing boolean argument".into())
        }
        ("isKeptUpdated", "()Z") => {
            let (_, _, _, keep_updated) = enumeration_state(objectref, jvm)?;
            Ok(Some(JvmStackValue::Int(if keep_updated { 1 } else { 0 })))
        }
        _ => Err(format!(
            "Unsupported RecordEnumeration method: {}{}",
            method_name, descriptor
        )),
    }
}

pub fn handle_record_enumeration_method_with_args(
    objectref: &JvmStackValue,
    method_name: &str,
    descriptor: &str,
    args: &[JvmStackValue],
    jvm: &JVM,
) -> Result<Option<JvmStackValue>, String> {
    if method_name == "keepUpdated" && descriptor == "(Z)V" {
        let keep_updated = get_bool_arg(args, 0, "RecordEnumeration.keepUpdated keepUpdated")?;
        set_enumeration_keep_updated(objectref, jvm, keep_updated)?;
        return Ok(None);
    }

    handle_record_enumeration_method(objectref, method_name, descriptor, jvm)
}

fn open_record_store(
    jvm: &JVM,
    namespace: String,
    name: &str,
    create: bool,
    authmode: i32,
    writable: bool,
) -> Result<JvmStackValue, String> {
    validate_store_name(name)?;

    let _guard = rms_guard();
    let store = match load_store(&namespace, name)? {
        Some(store) => store,
        None if create => {
            let store = StoredRecordStore::new(name.to_string());
            save_store(&namespace, &store)?;
            store
        }
        None => return Err(record_store_not_found(name)),
    };

    let mut fields = HashMap::new();
    fields.insert(
        "rms_namespace".to_string(),
        JvmStackValue::String(namespace.to_string()),
    );
    fields.insert("rms_name".to_string(), JvmStackValue::String(store.name));
    fields.insert("rms_open".to_string(), JvmStackValue::Int(1));
    fields.insert("rms_authmode".to_string(), JvmStackValue::Int(authmode));
    fields.insert(
        "rms_writable".to_string(),
        JvmStackValue::Int(if writable { 1 } else { 0 }),
    );

    let mut state = jvm.state.lock();
    let object_ref =
        JVM::allocate_internal(&mut state, RECORD_STORE_CLASS_NAME.to_string(), fields);
    Ok(JvmStackValue::ObjectRef(object_ref))
}

fn allocate_record_enumeration(
    jvm: &JVM,
    namespace: String,
    store_name: String,
    filter: JvmStackValue,
    comparator: JvmStackValue,
    keep_updated: bool,
) -> Result<JvmStackValue, String> {
    let mut fields = HashMap::new();
    fields.insert(
        "rms_namespace".to_string(),
        JvmStackValue::String(namespace),
    );
    fields.insert("rms_name".to_string(), JvmStackValue::String(store_name));
    fields.insert("rms_filter".to_string(), filter);
    fields.insert("rms_comparator".to_string(), comparator);
    fields.insert(
        "rms_keep_updated".to_string(),
        JvmStackValue::Int(if keep_updated { 1 } else { 0 }),
    );
    fields.insert("rms_destroyed".to_string(), JvmStackValue::Int(0));
    fields.insert("rms_index".to_string(), JvmStackValue::Int(0));
    fields.insert("rms_ids".to_string(), JvmStackValue::Vector(Vec::new()));

    let object_ref = {
        let mut state = jvm.state.lock();
        JVM::allocate_internal(
            &mut state,
            RECORD_ENUMERATION_CLASS_NAME.to_string(),
            fields,
        )
    };
    let object = JvmStackValue::ObjectRef(object_ref);
    rebuild_enumeration(&object, jvm, true)?;
    Ok(object)
}

fn rebuild_enumeration(
    objectref: &JvmStackValue,
    jvm: &JVM,
    reset_index: bool,
) -> Result<(), String> {
    let (object_id, namespace, store_name, filter, comparator, old_index) =
        enumeration_identity(objectref, jvm)?;
    let store = load_required_store(&namespace, &store_name)?;
    let mut records: Vec<(i32, Vec<u8>)> = Vec::new();

    for (id, data) in store.records {
        if record_matches_filter(&filter, &data, jvm)? {
            records.push((id, data));
        }
    }

    sort_records(&mut records, &comparator, jvm)?;
    let ids: Vec<JvmStackValue> = records
        .into_iter()
        .map(|(id, _)| JvmStackValue::Int(id))
        .collect();
    let new_index = if reset_index {
        0
    } else {
        old_index.min(ids.len())
    };

    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get_mut(object_id) else {
        return Err(record_store_exception(
            "RecordEnumeration object is not an instance",
        ));
    };
    obj.fields
        .insert("rms_ids".to_string(), JvmStackValue::Vector(ids));
    obj.fields.insert(
        "rms_index".to_string(),
        JvmStackValue::Int(new_index as i32),
    );
    Ok(())
}

fn refresh_if_kept_updated(objectref: &JvmStackValue, jvm: &JVM) -> Result<(), String> {
    let keep_updated = {
        let (object_id, _, _, _, _, _) = enumeration_identity(objectref, jvm)?;
        let state = jvm.state.lock();
        match state.heap.get(object_id) {
            Some(HeapObject::Instance(obj)) => {
                matches!(obj.fields.get("rms_keep_updated"), Some(JvmStackValue::Int(v)) if *v != 0)
            }
            _ => false,
        }
    };

    if keep_updated {
        rebuild_enumeration(objectref, jvm, false)?;
    }
    Ok(())
}

fn record_matches_filter(filter: &JvmStackValue, record: &[u8], jvm: &JVM) -> Result<bool, String> {
    if matches!(filter, JvmStackValue::Null) {
        return Ok(true);
    }

    let class_name = object_class_name(filter, jvm)?;
    let byte_array = allocate_byte_array(jvm, record);
    let mut stack = Vec::new();
    JVM::execute_method(
        filter.clone(),
        &class_name,
        "matches",
        "([B)Z",
        &[byte_array],
        jvm,
        &mut stack,
    )
    .map_err(|e| record_store_exception(&format!("RecordFilter.matches failed: {}", e)))?;

    match stack.pop() {
        Some(JvmStackValue::Int(value)) => Ok(value != 0),
        value => Err(record_store_exception(&format!(
            "RecordFilter.matches returned invalid value {:?}",
            value
        ))),
    }
}

fn sort_records(
    records: &mut Vec<(i32, Vec<u8>)>,
    comparator: &JvmStackValue,
    jvm: &JVM,
) -> Result<(), String> {
    if matches!(comparator, JvmStackValue::Null) {
        return Ok(());
    }

    let mut sorted: Vec<(i32, Vec<u8>)> = Vec::new();
    for record in records.drain(..) {
        let mut insert_at = sorted.len();
        for (idx, existing) in sorted.iter().enumerate() {
            if compare_records(comparator, &record.1, &existing.1, jvm)? < 0 {
                insert_at = idx;
                break;
            }
        }
        sorted.insert(insert_at, record);
    }
    *records = sorted;
    Ok(())
}

fn compare_records(
    comparator: &JvmStackValue,
    left: &[u8],
    right: &[u8],
    jvm: &JVM,
) -> Result<i32, String> {
    let class_name = object_class_name(comparator, jvm)?;
    let left_array = allocate_byte_array(jvm, left);
    let right_array = allocate_byte_array(jvm, right);
    let mut stack = Vec::new();
    JVM::execute_method(
        comparator.clone(),
        &class_name,
        "compare",
        "([B[B)I",
        &[left_array, right_array],
        jvm,
        &mut stack,
    )
    .map_err(|e| record_store_exception(&format!("RecordComparator.compare failed: {}", e)))?;

    match stack.pop() {
        Some(JvmStackValue::Int(value)) => Ok(value),
        value => Err(record_store_exception(&format!(
            "RecordComparator.compare returned invalid value {:?}",
            value
        ))),
    }
}

fn object_class_name(objectref: &JvmStackValue, jvm: &JVM) -> Result<String, String> {
    let id = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
        value => {
            return Err(record_store_exception(&format!(
                "expected object reference, found {:?}",
                value
            )));
        }
    };

    let state = jvm.state.lock();
    match state.heap.get(id) {
        Some(HeapObject::Instance(obj)) => Ok(obj.class_name.clone()),
        Some(_) => Err(record_store_exception(
            "object reference is not an instance",
        )),
        None => Err(record_store_exception(&format!(
            "invalid object reference {}",
            id
        ))),
    }
}

fn enumeration_record_bytes(
    objectref: &JvmStackValue,
    jvm: &JVM,
    record_id: i32,
) -> Result<Vec<u8>, String> {
    let (_, namespace, store_name, _, _, _) = enumeration_identity(objectref, jvm)?;
    let store = load_required_store(&namespace, &store_name)?;
    Ok(get_record(&store, record_id)?.to_vec())
}

fn take_enumeration_id(objectref: &JvmStackValue, jvm: &JVM, forward: bool) -> Result<i32, String> {
    let (object_id, ids, index, _) = enumeration_state(objectref, jvm)?;
    let new_index;
    let id;

    if forward {
        if index >= ids.len() {
            return Err(invalid_record_id(-1));
        }
        id = ids[index];
        new_index = index + 1;
    } else {
        if index == 0 {
            return Err(invalid_record_id(-1));
        }
        new_index = index - 1;
        id = ids[new_index];
    }

    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get_mut(object_id) else {
        return Err(record_store_exception(
            "RecordEnumeration object is not an instance",
        ));
    };
    obj.fields.insert(
        "rms_index".to_string(),
        JvmStackValue::Int(new_index as i32),
    );

    Ok(id)
}

fn set_enumeration_index(objectref: &JvmStackValue, jvm: &JVM, index: i32) -> Result<(), String> {
    let (object_id, _, _, _) = enumeration_state(objectref, jvm)?;
    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get_mut(object_id) else {
        return Err(record_store_exception(
            "RecordEnumeration object is not an instance",
        ));
    };
    obj.fields
        .insert("rms_index".to_string(), JvmStackValue::Int(index.max(0)));
    Ok(())
}

fn set_enumeration_keep_updated(
    objectref: &JvmStackValue,
    jvm: &JVM,
    keep_updated: bool,
) -> Result<(), String> {
    let (object_id, _, _, _) = enumeration_state(objectref, jvm)?;
    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get_mut(object_id) else {
        return Err(record_store_exception(
            "RecordEnumeration object is not an instance",
        ));
    };
    obj.fields.insert(
        "rms_keep_updated".to_string(),
        JvmStackValue::Int(if keep_updated { 1 } else { 0 }),
    );
    Ok(())
}

fn set_enumeration_destroyed(objectref: &JvmStackValue, jvm: &JVM) -> Result<(), String> {
    let object_id = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
        value => {
            return Err(record_store_exception(&format!(
                "RecordEnumeration expected object reference, found {:?}",
                value
            )));
        }
    };
    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get_mut(object_id) else {
        return Err(record_store_exception(
            "RecordEnumeration object is not an instance",
        ));
    };
    obj.fields
        .insert("rms_destroyed".to_string(), JvmStackValue::Int(1));
    obj.fields
        .insert("rms_ids".to_string(), JvmStackValue::Vector(Vec::new()));
    obj.fields
        .insert("rms_index".to_string(), JvmStackValue::Int(0));
    Ok(())
}

fn enumeration_state(
    objectref: &JvmStackValue,
    jvm: &JVM,
) -> Result<(usize, Vec<i32>, usize, bool), String> {
    let (object_id, _, _, _, _, _) = enumeration_identity(objectref, jvm)?;
    let state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get(object_id) else {
        return Err(record_store_exception(
            "RecordEnumeration object is not an instance",
        ));
    };

    let ids = match obj.fields.get("rms_ids") {
        Some(JvmStackValue::Vector(values)) => values
            .iter()
            .map(|value| match value {
                JvmStackValue::Int(id) => Ok(*id),
                value => Err(record_store_exception(&format!(
                    "RecordEnumeration invalid id value {:?}",
                    value
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    let index = match obj.fields.get("rms_index") {
        Some(JvmStackValue::Int(index)) if *index >= 0 => *index as usize,
        _ => 0,
    };
    let keep_updated =
        matches!(obj.fields.get("rms_keep_updated"), Some(JvmStackValue::Int(v)) if *v != 0);

    Ok((object_id, ids, index, keep_updated))
}

fn enumeration_identity(
    objectref: &JvmStackValue,
    jvm: &JVM,
) -> Result<(usize, String, String, JvmStackValue, JvmStackValue, usize), String> {
    let object_id = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
        value => {
            return Err(record_store_exception(&format!(
                "RecordEnumeration expected object reference, found {:?}",
                value
            )));
        }
    };

    let state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get(object_id) else {
        return Err(record_store_exception(
            "RecordEnumeration object is not an instance",
        ));
    };

    if matches!(obj.fields.get("rms_destroyed"), Some(JvmStackValue::Int(v)) if *v != 0) {
        return Err(record_store_exception("RecordEnumeration is destroyed"));
    }

    let namespace = read_string_field(obj, "rms_namespace")?;
    let store_name = read_string_field(obj, "rms_name")?;
    let filter = obj
        .fields
        .get("rms_filter")
        .cloned()
        .unwrap_or(JvmStackValue::Null);
    let comparator = obj
        .fields
        .get("rms_comparator")
        .cloned()
        .unwrap_or(JvmStackValue::Null);
    let index = match obj.fields.get("rms_index") {
        Some(JvmStackValue::Int(index)) if *index >= 0 => *index as usize,
        _ => 0,
    };

    Ok((object_id, namespace, store_name, filter, comparator, index))
}

fn record_store_identity(
    objectref: &JvmStackValue,
    jvm: &JVM,
    require_open: bool,
) -> Result<(usize, String, String), String> {
    let object_id = match objectref {
        JvmStackValue::ObjectRef(id) => *id as usize,
        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
        value => {
            return Err(record_store_exception(&format!(
                "RecordStore expected object reference, found {:?}",
                value
            )));
        }
    };

    let state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get(object_id) else {
        return Err(record_store_exception(
            "RecordStore object is not an instance",
        ));
    };

    if require_open && !matches!(obj.fields.get("rms_open"), Some(JvmStackValue::Int(v)) if *v != 0)
    {
        return Err(record_store_not_open());
    }

    Ok((
        object_id,
        read_string_field(obj, "rms_namespace")?,
        read_string_field(obj, "rms_name")?,
    ))
}

fn set_store_open(objectref: &JvmStackValue, jvm: &JVM, open: bool) -> Result<(), String> {
    let (object_id, _, _) = record_store_identity(objectref, jvm, false)?;
    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get_mut(object_id) else {
        return Err(record_store_exception(
            "RecordStore object is not an instance",
        ));
    };
    obj.fields.insert(
        "rms_open".to_string(),
        JvmStackValue::Int(if open { 1 } else { 0 }),
    );
    Ok(())
}

fn set_store_mode(
    objectref: &JvmStackValue,
    jvm: &JVM,
    authmode: i32,
    writable: bool,
) -> Result<(), String> {
    let (object_id, _, _) = record_store_identity(objectref, jvm, true)?;
    let mut state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get_mut(object_id) else {
        return Err(record_store_exception(
            "RecordStore object is not an instance",
        ));
    };
    obj.fields
        .insert("rms_authmode".to_string(), JvmStackValue::Int(authmode));
    obj.fields.insert(
        "rms_shared_writable".to_string(),
        JvmStackValue::Int(if writable { 1 } else { 0 }),
    );
    Ok(())
}

fn ensure_store_writable(objectref: &JvmStackValue, jvm: &JVM) -> Result<(), String> {
    let (object_id, _, _) = record_store_identity(objectref, jvm, true)?;
    let state = jvm.state.lock();
    let Some(HeapObject::Instance(obj)) = state.heap.get(object_id) else {
        return Err(record_store_exception(
            "RecordStore object is not an instance",
        ));
    };
    if matches!(obj.fields.get("rms_writable"), Some(JvmStackValue::Int(0))) {
        return Err(record_store_exception("RecordStore is read-only"));
    }
    Ok(())
}

fn read_string_field(obj: &JvmObject, field: &str) -> Result<String, String> {
    match obj.fields.get(field) {
        Some(JvmStackValue::String(value)) => Ok(value.clone()),
        value => Err(record_store_exception(&format!(
            "missing or invalid field {}: {:?}",
            field, value
        ))),
    }
}

fn get_record(store: &StoredRecordStore, record_id: i32) -> Result<&[u8], String> {
    store
        .records
        .get(&record_id)
        .map(Vec::as_slice)
        .ok_or_else(|| invalid_record_id(record_id))
}

fn read_record_bytes(
    jvm: &JVM,
    array_arg: Option<&JvmStackValue>,
    offset: i32,
    num_bytes: i32,
) -> Result<Vec<u8>, String> {
    if offset < 0 || num_bytes < 0 {
        return Err(format!(
            "java.lang.ArrayIndexOutOfBoundsException: offset {}, length {}",
            offset, num_bytes
        ));
    }

    if matches!(array_arg, Some(JvmStackValue::Null)) {
        if num_bytes == 0 {
            return Ok(Vec::new());
        }
        return Err("java.lang.NullPointerException".into());
    }

    let array_ref = match array_arg {
        Some(JvmStackValue::ObjectRef(id)) => *id as usize,
        Some(value) => {
            return Err(record_store_exception(&format!(
                "expected byte array, found {:?}",
                value
            )));
        }
        None if num_bytes == 0 => return Ok(Vec::new()),
        None => return Err("java.lang.NullPointerException".into()),
    };

    let offset = offset as usize;
    let num_bytes = num_bytes as usize;
    let state = jvm.state.lock();
    match state.heap.get(array_ref) {
        Some(HeapObject::Array { element_type, data }) => {
            if element_type != "primitive_8" {
                return Err(record_store_exception(&format!(
                    "expected byte array, found array of type {}",
                    element_type
                )));
            }

            let end = offset.checked_add(num_bytes).ok_or_else(|| {
                "java.lang.ArrayIndexOutOfBoundsException: source range overflow".to_string()
            })?;
            if end > data.len() {
                return Err(format!(
                    "java.lang.ArrayIndexOutOfBoundsException: source range {}..{} out of bounds for length {}",
                    offset,
                    end,
                    data.len()
                ));
            }

            data[offset..end]
                .iter()
                .map(|value| match value {
                    JvmStackValue::Byte(byte) => Ok(*byte),
                    JvmStackValue::Int(value) => Ok(*value as u8),
                    value => Err(record_store_exception(&format!(
                        "expected byte value, found {:?}",
                        value
                    ))),
                })
                .collect()
        }
        Some(_) => Err(record_store_exception("record data is not an array")),
        None => Err(record_store_exception(&format!(
            "invalid byte array reference {}",
            array_ref
        ))),
    }
}

fn write_record_into_byte_array(
    jvm: &JVM,
    array_arg: Option<&JvmStackValue>,
    offset: i32,
    record: &[u8],
) -> Result<(), String> {
    if offset < 0 {
        return Err(format!(
            "java.lang.ArrayIndexOutOfBoundsException: offset {}",
            offset
        ));
    }

    let array_ref = match array_arg {
        Some(JvmStackValue::ObjectRef(id)) => *id as usize,
        Some(JvmStackValue::Null) | None => return Err("java.lang.NullPointerException".into()),
        Some(value) => {
            return Err(record_store_exception(&format!(
                "expected byte array, found {:?}",
                value
            )));
        }
    };

    let offset = offset as usize;
    let mut state = jvm.state.lock();
    match state.heap.get_mut(array_ref) {
        Some(HeapObject::Array { element_type, data }) => {
            if element_type != "primitive_8" {
                return Err(record_store_exception(&format!(
                    "expected byte array, found array of type {}",
                    element_type
                )));
            }

            let end = offset.checked_add(record.len()).ok_or_else(|| {
                "java.lang.ArrayIndexOutOfBoundsException: destination range overflow".to_string()
            })?;
            if end > data.len() {
                return Err(format!(
                    "java.lang.ArrayIndexOutOfBoundsException: destination range {}..{} out of bounds for length {}",
                    offset,
                    end,
                    data.len()
                ));
            }

            for (slot, byte) in data[offset..end].iter_mut().zip(record.iter()) {
                *slot = JvmStackValue::Int((*byte as i8) as i32);
            }
            Ok(())
        }
        Some(_) => Err(record_store_exception("destination is not an array")),
        None => Err(record_store_exception(&format!(
            "invalid byte array reference {}",
            array_ref
        ))),
    }
}

fn allocate_byte_array(jvm: &JVM, bytes: &[u8]) -> JvmStackValue {
    let data = bytes
        .iter()
        .map(|byte| JvmStackValue::Int((*byte as i8) as i32))
        .collect();
    let mut state = jvm.state.lock();
    state.heap.push(HeapObject::Array {
        element_type: "primitive_8".to_string(),
        data,
    });
    JvmStackValue::ObjectRef((state.heap.len() - 1) as u32)
}

fn allocate_string_array(jvm: &JVM, strings: Vec<String>) -> JvmStackValue {
    let data = strings.into_iter().map(JvmStackValue::String).collect();
    let mut state = jvm.state.lock();
    state.heap.push(HeapObject::Array {
        element_type: "java/lang/String".to_string(),
        data,
    });
    JvmStackValue::ObjectRef((state.heap.len() - 1) as u32)
}

fn get_string_arg<'a>(
    args: &'a [JvmStackValue],
    index: usize,
    context: &str,
) -> Result<&'a str, String> {
    match args.get(index) {
        Some(JvmStackValue::String(value)) => Ok(value),
        Some(JvmStackValue::Null) => Err("java.lang.NullPointerException".into()),
        value => Err(record_store_exception(&format!(
            "{}: expected String, found {:?}",
            context, value
        ))),
    }
}

fn get_int_arg(args: &[JvmStackValue], index: usize, context: &str) -> Result<i32, String> {
    match args.get(index) {
        Some(JvmStackValue::Int(value)) => Ok(*value),
        value => Err(record_store_exception(&format!(
            "{}: expected int, found {:?}",
            context, value
        ))),
    }
}

fn get_bool_arg(args: &[JvmStackValue], index: usize, context: &str) -> Result<bool, String> {
    Ok(get_int_arg(args, index, context)? != 0)
}

fn validate_store_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 32 {
        return Err(format!(
            "java.lang.IllegalArgumentException: invalid record store name '{}'",
            name
        ));
    }
    Ok(())
}

fn ensure_store_size(store: &StoredRecordStore) -> Result<(), String> {
    if store.size() > MAX_STORE_SIZE {
        Err("javax.microedition.rms.RecordStoreFullException: record store is full".to_string())
    } else {
        Ok(())
    }
}

fn own_namespace(jvm: &JVM) -> String {
    if let Some(jar) = jvm.loaded_jar.as_ref() {
        let vendor = if jar.manifest.vendor.is_empty() {
            "unknown-vendor"
        } else {
            &jar.manifest.vendor
        };
        let suite = if jar.manifest.name.is_empty() {
            &jar.manifest.main_class
        } else {
            &jar.manifest.name
        };
        return shared_namespace(vendor, suite);
    }

    "unknown-vendor/unknown-suite".to_string()
}

fn shared_namespace(vendor: &str, suite: &str) -> String {
    format!("{}/{}", vendor, suite)
}

fn rms_guard() -> std::sync::MutexGuard<'static, ()> {
    RMS_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn load_required_store(namespace: &str, name: &str) -> Result<StoredRecordStore, String> {
    load_store(namespace, name)?.ok_or_else(|| record_store_not_found(name))
}

fn load_store(namespace: &str, name: &str) -> Result<Option<StoredRecordStore>, String> {
    let path = store_path(namespace, name);
    if !path.exists() {
        return Ok(None);
    }
    read_store_file(&path).map(Some)
}

fn save_store(namespace: &str, store: &StoredRecordStore) -> Result<(), String> {
    let dir = namespace_dir(namespace);
    fs::create_dir_all(&dir).map_err(|e| {
        record_store_exception(&format!("failed to create RMS directory {:?}: {}", dir, e))
    })?;

    let path = store_path(namespace, &store.name);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(STORE_MAGIC);
    write_u16(&mut bytes, store.name.len() as u16);
    bytes.extend_from_slice(store.name.as_bytes());
    write_i32(&mut bytes, store.version);
    write_i32(&mut bytes, store.next_id);
    write_i64(&mut bytes, store.last_modified);
    write_u32(&mut bytes, store.records.len() as u32);
    for (id, data) in &store.records {
        write_i32(&mut bytes, *id);
        write_u32(&mut bytes, data.len() as u32);
        bytes.extend_from_slice(data);
    }

    fs::write(&path, bytes).map_err(|e| {
        record_store_exception(&format!("failed to write RMS store {:?}: {}", path, e))
    })
}

fn delete_record_store(namespace: &str, name: &str) -> Result<(), String> {
    validate_store_name(name)?;
    let _guard = rms_guard();
    let path = store_path(namespace, name);
    if !path.exists() {
        return Err(record_store_not_found(name));
    }
    fs::remove_file(&path).map_err(|e| {
        record_store_exception(&format!("failed to delete RMS store {:?}: {}", path, e))
    })
}

fn list_record_stores(namespace: &str) -> Result<Vec<String>, String> {
    let _guard = rms_guard();
    let dir = namespace_dir(namespace);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut names = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| {
        record_store_exception(&format!("failed to read RMS directory {:?}: {}", dir, e))
    })?;
    for entry in entries {
        let entry = entry
            .map_err(|e| record_store_exception(&format!("failed to read RMS entry: {}", e)))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rms") {
            continue;
        }

        if let Ok(store) = read_store_file(&path) {
            names.push(store.name);
        }
    }
    names.sort();
    Ok(names)
}

fn read_store_file(path: &Path) -> Result<StoredRecordStore, String> {
    let bytes = fs::read(path).map_err(|e| {
        record_store_exception(&format!("failed to read RMS store {:?}: {}", path, e))
    })?;
    let mut pos = 0;
    if take(&bytes, &mut pos, STORE_MAGIC.len())? != STORE_MAGIC {
        return Err(record_store_exception(&format!(
            "invalid RMS store header in {:?}",
            path
        )));
    }

    let name_len = read_u16(&bytes, &mut pos)? as usize;
    let name = String::from_utf8(take(&bytes, &mut pos, name_len)?.to_vec())
        .map_err(|e| record_store_exception(&format!("invalid RMS store name: {}", e)))?;
    let version = read_i32(&bytes, &mut pos)?;
    let next_id = read_i32(&bytes, &mut pos)?;
    let last_modified = read_i64(&bytes, &mut pos)?;
    let record_count = read_u32(&bytes, &mut pos)? as usize;
    let mut records = BTreeMap::new();

    for _ in 0..record_count {
        let id = read_i32(&bytes, &mut pos)?;
        let len = read_u32(&bytes, &mut pos)? as usize;
        records.insert(id, take(&bytes, &mut pos, len)?.to_vec());
    }

    Ok(StoredRecordStore {
        name,
        version,
        next_id: next_id.max(1),
        last_modified,
        records,
    })
}

fn rms_root() -> PathBuf {
    if let Ok(path) = env::var("J2ME_RMS_DIR") {
        return PathBuf::from(path);
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".j2me_rms")
}

fn namespace_dir(namespace: &str) -> PathBuf {
    rms_root().join(file_segment(namespace))
}

fn store_path(namespace: &str, name: &str) -> PathBuf {
    namespace_dir(namespace).join(format!("{}.rms", file_segment(name)))
}

fn file_segment(value: &str) -> String {
    let mut safe = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    if safe.is_empty() {
        safe.push_str("unnamed");
    }

    if safe.len() > 64 {
        safe.truncate(64);
    }

    format!("{}_{:016x}", safe, stable_hash(value))
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn take<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8], String> {
    let end = pos
        .checked_add(len)
        .ok_or_else(|| record_store_exception("RMS store offset overflow"))?;
    if end > bytes.len() {
        return Err(record_store_exception("truncated RMS store"));
    }
    let slice = &bytes[*pos..end];
    *pos = end;
    Ok(slice)
}

fn read_u16(bytes: &[u8], pos: &mut usize) -> Result<u16, String> {
    Ok(u16::from_be_bytes(take(bytes, pos, 2)?.try_into().unwrap()))
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
    Ok(u32::from_be_bytes(take(bytes, pos, 4)?.try_into().unwrap()))
}

fn read_i32(bytes: &[u8], pos: &mut usize) -> Result<i32, String> {
    Ok(i32::from_be_bytes(take(bytes, pos, 4)?.try_into().unwrap()))
}

fn read_i64(bytes: &[u8], pos: &mut usize) -> Result<i64, String> {
    Ok(i64::from_be_bytes(take(bytes, pos, 8)?.try_into().unwrap()))
}

fn write_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn write_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn write_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn write_i64(bytes: &mut Vec<u8>, value: i64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn current_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn record_store_exception(message: &str) -> String {
    format!("javax.microedition.rms.RecordStoreException: {}", message)
}

fn record_store_not_found(name: &str) -> String {
    format!(
        "javax.microedition.rms.RecordStoreNotFoundException: {}",
        name
    )
}

fn record_store_not_open() -> String {
    "javax.microedition.rms.RecordStoreNotOpenException".to_string()
}

fn invalid_record_id(record_id: i32) -> String {
    format!(
        "javax.microedition.rms.InvalidRecordIDException: {}",
        record_id
    )
}
