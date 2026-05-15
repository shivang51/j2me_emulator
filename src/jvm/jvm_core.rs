use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::panic;
use std::sync::Arc;

use classfile_parser::constant_info::{ConstantInfo, FieldRefConstant, MethodRefConstant};

use crate::{
    jvm::javax::{
        lcdui::{display, game::game_canvas, graphics, image},
        midlet,
    },
    services::jar_extractor::JarFileData,
};

fn is_debug_enabled() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("JVM_DEBUG").is_ok())
}

macro_rules! jvm_debug {
    ($($arg:tt)*) => {
        if is_debug_enabled() {
            let thread_id = std::thread::current().id();
            print!("[{:?}] ", thread_id);
            println!($($arg)*);
        }
    };
}

#[derive(Debug, Clone)]
pub enum JvmStackValue {
    Byte(u8),
    Int(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    ObjectRef(u32),
    String(String),             // Or a pointer to a Heap-allocated string
    Vector(Vec<JvmStackValue>), // For array representations
    Null,
}

#[derive(Debug, Clone)]
pub struct JvmObject {
    pub class_name: String,
    pub fields: HashMap<String, JvmStackValue>,
}

#[derive(Debug, Clone)]
pub enum HeapObject {
    Instance(JvmObject),
    Array {
        element_type: String,
        data: Vec<JvmStackValue>,
    },
}

#[derive(Debug)]
pub struct JvmState {
    pub static_fields: HashMap<String, JvmStackValue>,
    pub heap: Vec<HeapObject>,
    pub classes: HashMap<String, classfile_parser::ClassFile>,
    pub resources: HashMap<String, Vec<u8>>,
    pub initialized_classes: HashSet<String>,
}

pub type SharedJvmState = Arc<Mutex<JvmState>>;

pub struct JVM {
    pub loaded_jar: Option<JarFileData>,
    pub state: SharedJvmState,
    pub thread_handles: Arc<Mutex<HashMap<u32, std::thread::JoinHandle<()>>>>,
}

impl Clone for JVM {
    fn clone(&self) -> Self {
        JVM {
            loaded_jar: self.loaded_jar.clone(),
            state: Arc::clone(&self.state),
            thread_handles: Arc::clone(&self.thread_handles),
        }
    }
}

#[derive(Debug)]
pub struct Code {
    #[allow(dead_code)]
    pub max_stack: u16,
    pub max_locals: u16,
    pub code: Vec<u8>,
}

impl JVM {
    pub fn new() -> Self {
        let mut state = JvmState {
            static_fields: HashMap::new(),
            heap: Vec::new(),
            classes: HashMap::new(),
            resources: HashMap::new(),
            initialized_classes: HashSet::new(),
        };

        state.static_fields.insert(
            "java/lang/System.out:Ljava/io/PrintStream;".to_string(),
            JvmStackValue::ObjectRef(999),
        );

        // creating java/lang/runtime instance

        let runtime_instance = JvmObject {
            class_name: "java/lang/Runtime".to_string(),
            fields: HashMap::new(),
        };
        state.heap.push(HeapObject::Instance(runtime_instance));
        state.static_fields.insert(
            "java/lang/Runtime.getRuntime:()Ljava/lang/Runtime;".to_string(),
            JvmStackValue::ObjectRef(0),
        );

        JVM {
            loaded_jar: None,
            state: Arc::new(Mutex::new(state)),
            thread_handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn run_jar(&mut self, data: JarFileData) -> Result<Option<JvmStackValue>, String> {
        self.loaded_jar = Some(data.clone());

        let main_class_name = data.manifest.main_class.replace('.', "/");

        {
            let mut state = self.state.lock();
            state.resources = data.resources;

            for (res_name, res_data) in &state.resources {
                println!("Loaded resource: {} ({} bytes)", res_name, res_data.len());
            }
        }

        for class in data.classes {
            let res = classfile_parser::class_parser(&class.content);
            if let Err(e) = res {
                return Err(format!(
                    "Failed to parse class file {}: {:?}",
                    class.name, e
                ));
            }

            let (_, parsed_class) = res.unwrap();
            let class_name = JVM::get_class_name(&parsed_class)?;
            let res1 = self.add_class(parsed_class);
            if let Err(e) = res1 {
                return Err(format!("Failed to add class {}: {}", class_name, e));
            }
        }

        println!("Running main class: {}", main_class_name);

        let main_class = {
            let state = self.state.lock();
            state
                .classes
                .get(&main_class_name)
                .ok_or_else(|| format!("Main class not found: {}", main_class_name))?
                .clone()
        };

        return self.execute_class(main_class, Some("startApp".into()));
    }

    pub fn add_class(&self, class: classfile_parser::ClassFile) -> Result<(), String> {
        let class_name = JVM::get_class_name(&class)?;
        let pool = &class.const_pool;
        let mut static_entries: Vec<(String, JvmStackValue)> = Vec::new();
        for field in &class.fields {
            let is_static = field
                .access_flags
                .contains(classfile_parser::field_info::FieldAccessFlags::STATIC);
            if !is_static {
                continue;
            }

            let name = JVM::resolve_utf8(field.name_index, pool);
            let desc = JVM::resolve_utf8(field.descriptor_index, pool);
            let key = format!("{}.{}:{}", class_name, name, desc);

            let default_val = if desc.starts_with('L') || desc.starts_with('[') {
                JvmStackValue::Null
            } else if desc == "F" {
                JvmStackValue::Float(0.0)
            } else if desc == "D" {
                JvmStackValue::Double(0.0)
            } else if desc == "J" {
                JvmStackValue::Long(0)
            } else {
                JvmStackValue::Int(0)
            };

            static_entries.push((key, default_val));
        }

        let mut state = self.state.lock();
        state.classes.insert(class_name.clone(), class.clone());
        for (k, v) in static_entries {
            state.static_fields.insert(k, v);
        }
        Ok(())
    }

    fn ensure_class_initialized(&self, class_name: &str) -> Result<(), String> {
        let class_data = {
            let state = self.state.lock();
            if state.initialized_classes.contains(class_name) {
                return Ok(());
            }

            state
                .classes
                .get(class_name)
                .cloned()
                .ok_or_else(|| format!("Class not found: {}", class_name))?
        };

        let has_clinit = JVM::find_method_in_class(&class_data, "<clinit>", "()V").is_some();

        {
            let mut state = self.state.lock();
            state.initialized_classes.insert(class_name.to_string());
        }

        if !has_clinit {
            return Ok(());
        }

        let mut caller_stack = Vec::new();
        JVM::execute_method(
            JvmStackValue::Null,
            class_name,
            "<clinit>",
            "()V",
            &[],
            self,
            &mut caller_stack,
        )
        .map_err(|e| format!("Class initialization failed for {}: {}", class_name, e))?;

        Ok(())
    }

    // Pass a class with a main method or entry point
    // By default: main method is with name `main`,
    // but we can pass our own
    pub fn execute_class(
        &self,
        class: classfile_parser::ClassFile,
        main_method_name: Option<String>,
    ) -> Result<Option<JvmStackValue>, String> {
        let pool = &class.const_pool;

        let main_name = main_method_name.unwrap_or_else(|| "main".to_string());

        for method in &class.methods {
            if let ConstantInfo::Utf8(name_info) = &pool[method.name_index as usize - 1] {
                if name_info.utf8_string == main_name {
                    println!("Found main method!");
                    for att in &method.attributes {
                        if let ConstantInfo::Utf8(att_name_info) =
                            &pool[att.attribute_name_index as usize - 1]
                        {
                            if att_name_info.utf8_string == "Code" {
                                println!("Found Code attribute for main method!");
                                let (_, code_attr) =
                                    classfile_parser::attribute_info::code_attribute_parser(
                                        att.info.as_slice(),
                                    )
                                    .map_err(|e| {
                                        format!("Failed to parse Code attribute: {:?}", e)
                                    })?;

                                let mut locals =
                                    vec![JvmStackValue::Null; code_attr.max_locals as usize];
                                if main_name != "main" {
                                    println!(
                                        "Executing entry point method '{}' instead of 'main'",
                                        main_name
                                    );
                                    let class_name = JVM::get_class_name(&class)?;
                                    let class_ref = self.allocate(class_name.to_string());
                                    let objectref = JvmStackValue::ObjectRef(class_ref);
                                    let mut constructor_stack = Vec::new();

                                    JVM::execute_method(
                                        objectref.clone(),
                                        &class_name,
                                        "<init>",
                                        "()V",
                                        &[],
                                        self,
                                        &mut constructor_stack,
                                    )?;

                                    if !locals.is_empty() {
                                        locals[0] = objectref;
                                    }
                                }

                                return JVM::run_frame(&code_attr.code, pool, &mut locals, self);
                            }
                        }
                    }
                }
            }
        }

        return Err(format!(
            "main method not found in class {}",
            JVM::get_class_name(&class)?,
        ));
    }

    pub fn run_frame(
        bytecode: &[u8],
        cp: &[ConstantInfo],
        locals: &mut Vec<JvmStackValue>,
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        let mut pc = 0;
        let mut stack: Vec<JvmStackValue> = Vec::new();
        let mut op_count = 0;

        while pc < bytecode.len() {
            let opcode = bytecode[pc];

            op_count += 1;
            if op_count % 1000 == 0 {
                std::thread::yield_now();
            }

            // debug-out added this line for easy finding this line ;)
            jvm_debug!(
                "PC: {}, Opcode: {:02X}, Stack: {:?}, Locals: {:?}",
                pc,
                opcode,
                stack,
                locals
            );

            // https://docs.oracle.com/javase/specs/jvms/se8/html/jvms-6.html

            match opcode {
                0x01 => {
                    // aconst_null
                    stack.push(JvmStackValue::Null);
                    pc += 1;
                }
                0x02 => {
                    // iconst_m1
                    stack.push(JvmStackValue::Int(-1));
                    pc += 1;
                }
                0x03 => {
                    // iconst_0
                    stack.push(JvmStackValue::Int(0));
                    pc += 1;
                }
                0x04 => {
                    // iconst_1
                    stack.push(JvmStackValue::Int(1));
                    pc += 1;
                }
                0x05 => {
                    // iconst_2
                    stack.push(JvmStackValue::Int(2));
                    pc += 1;
                }
                0x06 => {
                    // iconst_3
                    stack.push(JvmStackValue::Int(3));
                    pc += 1;
                }
                0x07 => {
                    // iconst_4
                    stack.push(JvmStackValue::Int(4));
                    pc += 1;
                }
                0x08 => {
                    // iconst_5
                    stack.push(JvmStackValue::Int(5));
                    pc += 1;
                }
                0x09 => {
                    // lconst_0
                    stack.push(JvmStackValue::Long(0));
                    pc += 1;
                }
                0x0A => {
                    // lconst_1
                    stack.push(JvmStackValue::Long(1));
                    pc += 1;
                }
                0x10 => {
                    // bipush
                    let byte_val = bytecode[pc + 1] as i8;
                    stack.push(JvmStackValue::Int(byte_val as i32));
                    pc += 2;
                }
                0x11 => {
                    // sipush
                    let short_val = i16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]);
                    stack.push(JvmStackValue::Int(short_val as i32));
                    pc += 3;
                }
                0x14 => {
                    // ldc2_w for long and double constants
                    let idx_bytes = [bytecode[pc + 1], bytecode[pc + 2]];
                    let cp_idx = u16::from_be_bytes(idx_bytes) as usize;

                    let entry = cp.get(cp_idx - 1).ok_or("Invalid CP index for ldc2_w")?;

                    match entry {
                        ConstantInfo::Long(long_info) => {
                            stack.push(JvmStackValue::Long(long_info.value));
                        }
                        ConstantInfo::Double(double_info) => {
                            stack.push(JvmStackValue::Double(double_info.value));
                        }
                        _ => {
                            return Err(format!(
                                "ldc2_w: Expected Long or Double at CP index {}, found {:?}",
                                cp_idx, entry
                            )
                            .into());
                        }
                    }
                    pc += 3;
                }
                0x1a..=0x2d => {
                    // load_n (iload_n, lload_n, fload_n, dload_n, aload_n)
                    let index = if opcode <= 0x1d {
                        opcode - 0x1a
                    } else if opcode <= 0x21 {
                        opcode - 0x1e
                    } else if opcode <= 0x25 {
                        opcode - 0x22
                    } else if opcode <= 0x29 {
                        opcode - 0x26
                    } else {
                        opcode - 0x2a
                    } as usize;

                    if index >= locals.len() {
                        stack.push(JvmStackValue::Null);
                    } else {
                        stack.push(locals[index].clone());
                    }
                    pc += 1;
                }
                0x2e => {
                    // iaload
                    let index = match stack.pop().ok_or("iaload: Stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("iaload: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("iaload: Stack underflow (arrayref)")?;
                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("iaload: arrayref is not a reference".into()),
                    };

                    {
                        let state = jvm.state.lock();
                        match state.heap.get(heap_idx) {
                            Some(HeapObject::Array { data, .. }) => {
                                if index < 0 || index as usize >= data.len() {
                                    return Err(format!(
                                        "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds",
                                        index
                                    ));
                                }

                                match &data[index as usize] {
                                    JvmStackValue::Int(value) => {
                                        stack.push(JvmStackValue::Int(*value))
                                    }
                                    value => {
                                        return Err(format!(
                                            "iaload: expected Int, found {:?}",
                                            value
                                        ));
                                    }
                                }
                            }
                            _ => return Err("iaload: object is not an array".into()),
                        }
                    }

                    pc += 1;
                }
                0x32 => {
                    // aaload
                    let index = match stack.pop().ok_or("aaload: Stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("aaload: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("aaload: Stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => {
                            return Err(
                                format!("aaload: expected reference, got {:?}", arrayref).into()
                            );
                        }
                    };

                    {
                        let state = jvm.state.lock();
                        let heap_obj = state
                            .heap
                            .get(heap_idx)
                            .ok_or("aaload: invalid heap reference")?;

                        jvm_debug!(
                            "aaload: arrayref points to heap index {}, heap object: {:?}",
                            heap_idx,
                            heap_obj
                        );

                        match heap_obj {
                            HeapObject::Array { data, .. } => {
                                if index < 0 || index as usize >= data.len() {
                                    return Err(format!("java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds", index).into());
                                }

                                jvm_debug!(
                                    "aaload: Retrieved value from array at index {}: | data: {:?}",
                                    index,
                                    data,
                                );

                                let value = data[index as usize].clone();

                                match value {
                                    JvmStackValue::ObjectRef(_)
                                    | JvmStackValue::Null
                                    | JvmStackValue::String(_)
                                    | JvmStackValue::Vector(_) => stack.push(value),
                                    _ => {
                                        return Err(
                                            "aaload: component at index is not a reference".into(),
                                        );
                                    }
                                }
                            }
                            _ => return Err("aaload: object is not an array".into()),
                        }
                    }
                    pc += 1;
                }
                0x33 => {
                    // baload
                    let index = match stack.pop().ok_or("baload: Stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("baload: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("baload: Stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("baload: arrayref is not a reference".into()),
                    };

                    {
                        let state = jvm.state.lock();
                        match state.heap.get(heap_idx) {
                            Some(HeapObject::Array { element_type, data }) => {
                                if index < 0 || index as usize >= data.len() {
                                    return Err(format!(
                                        "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds",
                                        index
                                    ));
                                }

                                let loaded = match &data[index as usize] {
                                    JvmStackValue::Byte(byte_value) => i32::from(*byte_value as i8),
                                    JvmStackValue::Int(int_value)
                                        if element_type == "primitive_4"
                                            || element_type == "primitive_8" =>
                                    {
                                        ((*int_value as u8) as i8) as i32
                                    }
                                    other => {
                                        return Err(format!(
                                            "baload: expected Byte or Int, found {:?}",
                                            other
                                        ));
                                    }
                                };

                                stack.push(JvmStackValue::Int(loaded));
                            }
                            _ => return Err("baload: object is not an array".into()),
                        }
                    }

                    pc += 1;
                }
                0x3b..=0x4a => {
                    // istore_n, lstore_n, fstore_n, dstore_n
                    let index = if opcode <= 0x3e {
                        opcode - 0x3b
                    } else if opcode <= 0x42 {
                        opcode - 0x3f
                    } else if opcode <= 0x46 {
                        opcode - 0x43
                    } else {
                        opcode - 0x47
                    };

                    let val = stack.pop().ok_or("store_n: stack underflow")?;
                    let idx = index as usize;
                    if locals.len() <= idx {
                        locals.resize(idx + 1, JvmStackValue::Null);
                    }
                    locals[idx] = val;
                    pc += 1;
                }
                0x4b..=0x4e => {
                    // astore_n
                    let idx = (opcode - 0x4b) as usize;
                    let val = stack.pop().ok_or("astore_n: stack underflow")?;
                    if locals.len() <= idx {
                        locals.resize(idx + 1, JvmStackValue::Null);
                    }
                    locals[idx] = val;
                    pc += 1;
                }
                0x4f => {
                    // iastore
                    let value = match stack.pop().ok_or("iastore: stack underflow (value)")? {
                        JvmStackValue::Int(value) => value,
                        value => return Err(format!("iastore: value is not an int: {:?}", value)),
                    };
                    let index = match stack.pop().ok_or("iastore: stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("iastore: index is not an int".into()),
                    };
                    let arrayref = stack.pop().ok_or("iastore: stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("iastore: arrayref is not a reference".into()),
                    };

                    match jvm.state.lock().heap.get_mut(heap_idx) {
                        Some(HeapObject::Array { data, .. }) => {
                            if index < 0 || index as usize >= data.len() {
                                return Err(format!(
                                    "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds for length {}",
                                    index,
                                    data.len()
                                ));
                            }

                            data[index as usize] = JvmStackValue::Int(value);
                        }
                        _ => return Err("iastore: object is not an array".into()),
                    }

                    pc += 1;
                }
                0x5a => {
                    // dup_x1
                    let value1 = stack.pop().ok_or("dup_x1: stack underflow (value1)")?;
                    let value2 = stack.pop().ok_or("dup_x1: stack underflow (value2)")?;

                    stack.push(value1.clone());
                    stack.push(value2);
                    stack.push(value1);

                    pc += 1;
                }
                0x50 => {
                    // lastore
                    let value = stack.pop().ok_or("lastore: stack underflow (value)")?;
                    let index = match stack.pop().ok_or("lastore: stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("lastore: index is not an int".into()),
                    };
                    let arrayref = stack.pop().ok_or("lastore: stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("lastore: arrayref is not a reference".into()),
                    };

                    match jvm.state.lock().heap.get_mut(heap_idx) {
                        Some(HeapObject::Array { data, .. }) => {
                            if index < 0 || index as usize >= data.len() {
                                return Err(format!(
                                    "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds for length {}",
                                    index, data.len()
                                ).into());
                            }

                            data[index as usize] = value;
                        }
                        _ => return Err("lastore: object is not an array".into()),
                    }

                    pc += 1;
                }
                0x53 => {
                    // aastore
                    let value = stack.pop().ok_or("aastore: stack underflow (value)")?;
                    let index = match stack.pop().ok_or("aastore: stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("aastore: index is not an int".into()),
                    };
                    let arrayref = stack.pop().ok_or("aastore: stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("aastore: arrayref is not a reference".into()),
                    };

                    match jvm.state.lock().heap.get_mut(heap_idx) {
                        Some(HeapObject::Array { data, .. }) => {
                            if index < 0 || index as usize >= data.len() {
                                return Err(format!(
                    "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds for length {}",
                    index, data.len()
                ).into());
                            }

                            data[index as usize] = value;
                        }
                        Some(HeapObject::Instance(_)) => {
                            return Err("aastore: object is not an array".into());
                        }
                        None => return Err("aastore: invalid heap reference".into()),
                    }

                    pc += 1;
                }
                0x54 => {
                    // bastore

                    let value = match stack.pop().ok_or("bastore: stack underflow (value)")? {
                        JvmStackValue::Int(i) => i,
                        val => return Err(format!("bastore: value {:?} is not an int", val)),
                    };

                    let index = match stack.pop().ok_or("bastore: stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("bastore: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("bastore: stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("bastore: arrayref is not a reference".into()),
                    };

                    match jvm.state.lock().heap.get_mut(heap_idx) {
                        Some(HeapObject::Array { element_type, data }) => {
                            if element_type != "primitive_8" {
                                return Err(format!(
                                    "bastore: expected byte array, found array of type {}",
                                    element_type
                                )
                                .into());
                            }

                            if index < 0 || index as usize >= data.len() {
                                return Err(format!(
                                    "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds for length {}",
                                    index, data.len()
                                ).into());
                            }

                            data[index as usize] = JvmStackValue::Int(value);
                        }
                        _ => return Err("bastore: object is not an array".into()),
                    }

                    pc += 1;
                }
                0x57 => {
                    // pop
                    stack.pop().ok_or("pop: Stack underflow")?;
                    pc += 1;
                }
                0x59 => {
                    // dup - Duplicate the top value on the stack

                    let top_value = stack.last().cloned().ok_or("dup: Stack underflow")?;

                    stack.push(top_value);

                    pc += 1;
                }
                0x60 => {
                    // iadd
                    let val2 = stack.pop().ok_or("iadd: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("iadd: Stack underflow for val1")?;

                    if let (JvmStackValue::Int(i1), JvmStackValue::Int(i2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Int(i1 + i2));
                    } else {
                        return Err(format!(
                            "iadd: Expected two Ints on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x61 => {
                    // ladd
                    let val2 = stack.pop().ok_or("ladd: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("ladd: Stack underflow for val1")?;

                    if let (JvmStackValue::Long(l1), JvmStackValue::Long(l2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Long(l1 + l2));
                    } else {
                        return Err(format!(
                            "ladd: Expected two Longs on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x62 => {
                    // fadd
                    let val2 = stack.pop().ok_or("fadd: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("fadd: Stack underflow for val1")?;

                    if let (JvmStackValue::Float(f1), JvmStackValue::Float(f2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Float(f1 + f2));
                    } else {
                        return Err(format!(
                            "fadd: Expected two Floats on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x63 => {
                    // dadd
                    let val2 = stack.pop().ok_or("dadd: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("dadd: Stack underflow for val1")?;

                    if let (JvmStackValue::Double(d1), JvmStackValue::Double(d2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Double(d1 + d2));
                    } else {
                        return Err(format!(
                            "dadd: Expected two Doubles on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x64 => {
                    // isub
                    let val2 = stack.pop().ok_or("isub: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("isub: Stack underflow for val1")?;

                    if let (JvmStackValue::Int(i1), JvmStackValue::Int(i2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Int(i1 - i2));
                    } else {
                        return Err(format!(
                            "isub: Expected two Ints on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x65 => {
                    // lsub
                    let val2 = stack.pop().ok_or("lsub: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("lsub: Stack underflow for val1")?;

                    if let (JvmStackValue::Long(l1), JvmStackValue::Long(l2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Long(l1 - l2));
                    } else {
                        return Err(format!(
                            "lsub: Expected two Longs on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x68 => {
                    // imul
                    let val2 = stack.pop().ok_or("imul: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("imul: Stack underflow for val1")?;

                    if let (JvmStackValue::Int(i1), JvmStackValue::Int(i2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Int(i1.wrapping_mul(i2)));
                    } else {
                        return Err(format!(
                            "imul: Expected two Ints on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x69 => {
                    // lmul
                    let val2 = stack.pop().ok_or("lmul: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("lmul: Stack underflow for val1")?;

                    if let (JvmStackValue::Long(l1), JvmStackValue::Long(l2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Long(l1 * l2));
                    } else {
                        return Err(format!(
                            "lmul: Expected two Longs on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }

                    pc += 1;
                }
                0x6c => {
                    // idiv
                    let val2 = stack.pop().ok_or("idiv: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("idiv: Stack underflow for val1")?;

                    if let (JvmStackValue::Int(i1), JvmStackValue::Int(i2)) =
                        (val1.clone(), val2.clone())
                    {
                        if i2 == 0 {
                            return Err("java.lang.ArithmeticException: Division by zero".into());
                        }
                        stack.push(JvmStackValue::Int(i1 / i2));
                    } else {
                        return Err(format!(
                            "idiv: Expected two Ints on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }

                    pc += 1;
                }
                0x6d => {
                    //ldiv
                    let val2 = stack.pop().ok_or("ldiv: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("ldiv: Stack underflow for val1")?;

                    if let (JvmStackValue::Long(l1), JvmStackValue::Long(l2)) =
                        (val1.clone(), val2.clone())
                    {
                        if l2 == 0 {
                            return Err("java.lang.ArithmeticException: Division by zero".into());
                        }
                        stack.push(JvmStackValue::Long(l1 / l2));
                    } else {
                        return Err(format!(
                            "ldiv: Expected two Longs on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }

                    pc += 1;
                }
                0x80 => {
                    //ior
                    let val2 = stack.pop().ok_or("ior: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("ior: Stack underflow for val1")?;

                    if let (JvmStackValue::Int(i1), JvmStackValue::Int(i2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Int(i1 | i2));
                    } else {
                        return Err(format!(
                            "ior: Expected two Ints on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x84 => {
                    // iinc
                    let index = bytecode[pc + 1] as usize;
                    let constant = bytecode[pc + 2] as i8 as i32;

                    if index >= locals.len() {
                        return Err(format!("iinc: invalid local index {}", index).into());
                    }

                    match &mut locals[index] {
                        JvmStackValue::Int(value) => *value += constant,
                        value => {
                            return Err(format!(
                                "iinc: expected Int in local {}, found {:?}",
                                index, value
                            )
                            .into());
                        }
                    }

                    pc += 3;
                }
                0x88 => {
                    // l2i
                    let val = stack.pop().ok_or("l2i: Stack underflow")?;
                    if let JvmStackValue::Long(l) = val.clone() {
                        stack.push(JvmStackValue::Int(l as i32));
                    } else {
                        return Err(format!("l2i: Expected Long on stack, found {:?}", val).into());
                    }
                    pc += 1;
                }
                0x94 => {
                    // lcmp
                    let val2 = stack.pop().ok_or("lcmp: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("lcmp: Stack underflow for val1")?;

                    if let (JvmStackValue::Long(l1), JvmStackValue::Long(l2)) =
                        (val1.clone(), val2.clone())
                    {
                        let result = if l1 < l2 {
                            -1
                        } else if l1 > l2 {
                            1
                        } else {
                            0
                        };
                        stack.push(JvmStackValue::Int(result));
                    } else {
                        return Err(format!(
                            "lcmp: Expected two Longs on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }

                    pc += 1;
                }
                0x99..=0x9E => {
                    // ifeq, ifne, iflt, ifge, ifgt, ifle
                    let opcode = bytecode[pc];

                    let offset =
                        (((bytecode[pc + 1] as i16) << 8) | (bytecode[pc + 2] as i16)) as i32;

                    let val = match stack.pop().ok_or("if<cond>: stack underflow")? {
                        JvmStackValue::Int(v) => v,
                        _ => return Err("if<cond>: expected Int on stack".into()),
                    };

                    let condition_met = match opcode {
                        0x99 => val == 0, // ifeq
                        0x9A => val != 0, // ifne
                        0x9B => val < 0,  // iflt
                        0x9C => val >= 0, // ifge
                        0x9D => val > 0,  // ifgt
                        0x9E => val <= 0, // ifle
                        _ => unreachable!(),
                    };

                    if condition_met {
                        pc = (pc as i32 + offset) as usize;
                        continue;
                    } else {
                        pc += 3;
                    }
                }
                0x9F..=0xA4 => {
                    //if_icmp<cond>
                    let opcode = bytecode[pc];

                    let offset =
                        (((bytecode[pc + 1] as i16) << 8) | (bytecode[pc + 2] as i16)) as i32;

                    let val2 = match stack
                        .pop()
                        .ok_or("if_icmp<cond>: stack underflow for val2")?
                    {
                        JvmStackValue::Int(v) => v,
                        _ => return Err("if_icmp<cond>: expected Int for val2".into()),
                    };

                    let val1 = match stack
                        .pop()
                        .ok_or("if_icmp<cond>: stack underflow for val1")?
                    {
                        JvmStackValue::Int(v) => v,
                        _ => return Err("if_icmp<cond>: expected Int for val1".into()),
                    };

                    let condition_met = match opcode {
                        0x9F => val1 == val2, // if_icmpeq
                        0xA0 => val1 != val2, // if_icmpne
                        0xA1 => val1 < val2,  // if_icmplt
                        0xA2 => val1 >= val2, // if_icmpge
                        0xA3 => val1 > val2,  // if_icmpgt
                        0xA4 => val1 <= val2, // if_icmple
                        _ => unreachable!(),
                    };

                    if condition_met {
                        pc = (pc as i32 + offset) as usize;
                        continue;
                    } else {
                        pc += 3;
                    }
                }
                0xA7 => {
                    // goto
                    let offset =
                        (((bytecode[pc + 1] as i16) << 8) | (bytecode[pc + 2] as i16)) as i32;
                    pc = (pc as i32 + offset) as usize;
                }
                0xB0 => {
                    // areturn
                    let return_val = stack.pop().ok_or("areturn: Stack underflow")?;
                    return Ok(Some(return_val));
                }
                0xB2 => {
                    // getstatic
                    let idx_bytes = [bytecode[pc + 1], bytecode[pc + 2]];
                    let cp_idx = u16::from_be_bytes(idx_bytes);

                    let ConstantInfo::FieldRef(field_ref) = &cp[cp_idx as usize - 1] else {
                        return Err(format!(
                            "Expected FieldRef at CP index {}, found {:?}",
                            cp_idx,
                            cp[cp_idx as usize - 1]
                        ));
                    };

                    let key = JVM::get_field_key(field_ref, cp);
                    let class_name = key
                        .rsplit_once('.')
                        .map(|(class_name, _)| class_name.to_string())
                        .ok_or_else(|| format!("Invalid static field key: {}", key))?;

                    jvm.ensure_class_initialized(&class_name)?;

                    let val = {
                        let state = jvm.state.lock();
                        state
                            .static_fields
                            .get(&key)
                            .cloned()
                            .ok_or_else(|| format!("Static field not found: {}", key))
                    };

                    if let Err(e) = &val {
                        println!("Error: {}", e);
                        let state = jvm.state.lock();
                        println!("Available static fields:");
                        for (k, v) in &state.static_fields {
                            println!("{}: {:?}", k, v);
                            let k_matches = k == &key;
                            println!("match: {:?}", k_matches);
                        }
                        println!("-----------------------------");
                        return Err(e.clone());
                    }

                    stack.push(val.unwrap());

                    pc += 3;
                }
                0xB3 => {
                    // putstatic
                    let idx_bytes = [bytecode[pc + 1], bytecode[pc + 2]];
                    let cp_idx = u16::from_be_bytes(idx_bytes);

                    let ConstantInfo::FieldRef(field_ref) = &cp[cp_idx as usize - 1] else {
                        return Err(format!(
                            "Expected FieldRef at CP index {}, found {:?}",
                            cp_idx,
                            cp[cp_idx as usize - 1]
                        ));
                    };

                    let key = JVM::get_field_key(field_ref, cp);
                    let class_name = key
                        .rsplit_once('.')
                        .map(|(class_name, _)| class_name.to_string())
                        .ok_or_else(|| format!("Invalid static field key: {}", key))?;

                    jvm.ensure_class_initialized(&class_name)?;

                    let value = stack.pop().ok_or("putstatic: Stack underflow")?;

                    {
                        let mut state = jvm.state.lock();
                        state.static_fields.insert(key, value);
                    }

                    pc += 3;
                }
                0x12 => {
                    // ldc

                    let cp_index = bytecode[pc + 1] as usize;
                    let entry = cp
                        .get(
                            cp_index
                                .checked_sub(1)
                                .ok_or_else(|| "Invalid CP index 0 for LDC".to_string())?,
                        )
                        .ok_or_else(|| format!("Invalid CP index for LDC: {}", cp_index))?;

                    stack.push(match entry {
                        ConstantInfo::Integer(int_info) => JvmStackValue::Int(int_info.value),
                        ConstantInfo::Float(float_info) => JvmStackValue::Float(float_info.value),
                        ConstantInfo::String(str_info) => {
                            if let ConstantInfo::Utf8(utf8_info) =
                                &cp[str_info.string_index as usize - 1]
                            {
                                JvmStackValue::String(utf8_info.utf8_string.clone())
                            } else {
                                return Err(format!(
                                    "Expected Utf8 for String constant at CP index {}, found {:?}",
                                    str_info.string_index,
                                    cp[str_info.string_index as usize - 1]
                                ));
                            }
                        }
                        ConstantInfo::Utf8(utf8_info) => {
                            JvmStackValue::String(utf8_info.utf8_string.clone())
                        }
                        _ => {
                            return Err(format!(
                                "Unsupported constant type for LDC at CP index {}: {:?}",
                                cp_index, entry
                            ));
                        }
                    });
                    pc += 2;
                }
                0xB4 => {
                    // getfield
                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    let ConstantInfo::FieldRef(field_ref) = &cp[cp_index - 1] else {
                        return Err("getfield: expected FieldRef".into());
                    };

                    let field_name = JVM::resolve_field_name(field_ref, cp);

                    let objectref = stack.pop().ok_or("getfield: stack underflow")?;

                    let heap_idx = match objectref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("getfield: objectref is not a reference".into()),
                    };

                    let field_value = {
                        let state = jvm.state.lock();
                        let obj = state
                            .heap
                            .get(heap_idx)
                            .ok_or_else(|| format!("Invalid heap access at index {}", heap_idx))?;

                        if let HeapObject::Instance(obj) = obj {
                            obj.fields
                                .get(&field_name)
                                .ok_or_else(|| {
                                    format!(
                                        "Field '{}' not found in object of class '{}'",
                                        field_name, obj.class_name
                                    )
                                })?
                                .clone()
                        } else {
                            return Err("getfield: Heap object is not an instance".into());
                        }
                    };

                    stack.push(field_value);

                    pc += 3;
                }
                0xB5 => {
                    // putfield
                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    let ConstantInfo::FieldRef(field_ref) = &cp[cp_index - 1] else {
                        return Err("putfield: expected FieldRef".into());
                    };

                    let field_name = JVM::resolve_field_name(field_ref, cp);

                    let value = stack.pop().ok_or("putfield: stack underflow (value)")?;

                    let objectref = stack.pop().ok_or("putfield: stack underflow (objectref)")?;

                    let heap_idx = match objectref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("putfield: objectref is not a reference".into()),
                    };

                    {
                        let mut state = jvm.state.lock();
                        let obj = state
                            .heap
                            .get_mut(heap_idx)
                            .ok_or_else(|| format!("Invalid heap access at index {}", heap_idx))?;

                        if let HeapObject::Instance(obj) = obj {
                            obj.fields.insert(field_name, value);
                        } else {
                            return Err("putfield: Heap object is not an instance".into());
                        }
                    }

                    pc += 3;
                }
                0xB6 => {
                    // invokevirtual

                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    // resolve method info
                    let ConstantInfo::MethodRef(m_ref) = &cp[cp_index - 1] else {
                        return Err("invokevirtual: expected MethodRef".into());
                    };
                    let (class_name, method_name, descriptor) =
                        JVM::resolve_method_identity(m_ref, cp);

                    // get count of args
                    let arg_count = JVM::count_arguments(&descriptor);

                    // get args from stack
                    let mut args = Vec::new();
                    for _ in 0..arg_count {
                        args.push(stack.pop().ok_or("Stack underflow: missing arguments")?);
                    }

                    args.reverse(); // Maintain original order: [arg1, arg2, ...]

                    let objectref = stack.pop().ok_or("Stack underflow: missing objectref")?;

                    //  if objectref is null, throw NullPointerException
                    if let JvmStackValue::Null = objectref {
                        return Err("0xB6 - java.lang.NullPointerException".into());
                    }

                    if let JvmStackValue::ObjectRef(999) = objectref {
                        JVM::handle_native_printstream(&method_name, &args);
                    }
                    if class_name == "java/lang/String" {
                        let res = JVM::execute_method(
                            objectref,
                            &class_name,
                            &method_name,
                            &descriptor,
                            &args,
                            jvm,
                            &mut stack,
                        );

                        if let Err(e) = res {
                            return Err(format!("Error executing method: {}", e).into());
                        }
                    } else if class_name == "java/lang/StringBuffer" {
                        if let JvmStackValue::ObjectRef(id) = &objectref {
                            args.insert(0, objectref.clone());
                            let res = {
                                let mut state = jvm.state.lock();
                                let heap_obj = state.heap.get_mut(*id as usize).ok_or_else(|| {
                                    format!(
                                        "invokevirtual on java/lang/StringBuffer with id {}, but no object found in heap",
                                        id
                                    )
                                })?;
                                JVM::handle_str_buffer_fns(heap_obj, &method_name, &args)
                            };

                            if let Err(e) = res {
                                return Err(
                                    format!("Error handling StringBuffer method: {}", e).into()
                                );
                            }

                            if let Some(return_val) = res.unwrap() {
                                stack.push(return_val);
                            }
                        }
                    } else {
                        let actual_class_name = {
                            let state = jvm.state.lock();
                            if let HeapObject::Instance(obj) = &state.heap[match objectref {
                                JvmStackValue::ObjectRef(id) => id as usize,
                                _ => {
                                    return Err(
                                        "invokevirtual: objectref is not a reference".into()
                                    );
                                }
                            }] {
                                obj.class_name.clone()
                            } else {
                                class_name
                            }
                        };

                        let res = JVM::execute_method(
                            objectref,
                            &actual_class_name,
                            &method_name,
                            &descriptor,
                            &args,
                            jvm,
                            &mut stack,
                        );

                        if let Err(e) = res {
                            return Err(format!("Error executing method: {}", e).into());
                        }
                    }

                    pc += 3;
                }
                0xB7 => {
                    // invokespecial
                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    let (class_name, method_name, descriptor) = match &cp[cp_index - 1] {
                        ConstantInfo::MethodRef(m_ref) => JVM::resolve_method_identity(m_ref, cp),
                        _ => {
                            return Err(format!(
                                "invokespecial: expected MethodRef at index {}",
                                cp_index
                            )
                            .into());
                        }
                    };

                    let arg_count = JVM::count_arguments(&descriptor);

                    let mut args = Vec::new();
                    for _ in 0..arg_count {
                        args.push(stack.pop().ok_or("Stack underflow popping arguments")?);
                    }
                    args.reverse(); // Restore original argument order

                    let objectref = stack.pop().ok_or("Stack underflow popping objectref")?;

                    if let JvmStackValue::Null = objectref {
                        return Err("0xB7 - java.lang.NullPointerException".into());
                    }

                    if class_name != "java/lang/Object" || method_name != "<init>" {
                        // Execute the targeted method.
                        // In a full VM, this creates a new Frame.
                        jvm_debug!(
                            "invokespecial executing: {}.{}{}",
                            class_name,
                            method_name,
                            descriptor
                        );

                        let res = JVM::execute_method(
                            objectref,
                            &class_name,
                            &method_name,
                            &descriptor,
                            &args,
                            jvm,
                            &mut stack,
                        );

                        if let Err(e) = res {
                            return Err(format!("Error executing method: {}", e).into());
                        }
                    }

                    pc += 3;
                }
                0xB8 => {
                    // invokestatic
                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    let (class_name, method_name, descriptor) = match &cp[cp_index - 1] {
                        ConstantInfo::MethodRef(m_ref) => JVM::resolve_method_identity(m_ref, cp),
                        _ => {
                            return Err(format!(
                                "invokestatic: expected MethodRef at index {} but found {:?}",
                                cp_index,
                                cp[cp_index - 1]
                            )
                            .into());
                        }
                    };

                    let arg_count = JVM::count_arguments(&descriptor);

                    let mut args = Vec::new();

                    for _ in 0..arg_count {
                        args.push(stack.pop().ok_or("Stack underflow popping arguments")?);
                    }

                    args.reverse(); // Restore original argument order
                    let res = JVM::execute_static_method(
                        &class_name,
                        &method_name,
                        &descriptor,
                        &args,
                        jvm,
                        &mut stack,
                    );

                    if let Err(e) = res {
                        return Err(format!("Error executing static method: {}", e).into());
                    }

                    pc += 3;
                }
                0xC0 => {
                    // checkcast
                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    let ConstantInfo::Class(class_info) = &cp[cp_index - 1] else {
                        return Err(format!(
                            "checkcast: expected Class constant at index {}, found {:?}",
                            cp_index,
                            cp[cp_index - 1]
                        )
                        .into());
                    };

                    let target_class_name = JVM::resolve_utf8(class_info.name_index, cp);

                    let objectref = stack.last().cloned().ok_or("checkcast: stack underflow")?;
                    if let JvmStackValue::Null = objectref {
                        // null can be cast to any reference type and stays on the stack.
                        pc += 3;
                        continue;
                    }

                    let heap_idx = match objectref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        _ => return Err("checkcast: expected reference on stack".into()),
                    };

                    let actual_type_name = {
                        let state = jvm.state.lock();
                        match &state.heap[heap_idx] {
                            HeapObject::Instance(obj) => obj.class_name.clone(),
                            HeapObject::Array { element_type, .. } => {
                                JVM::array_runtime_type_name(element_type)
                            }
                        }
                    };

                    let cast_ok = {
                        let state = jvm.state.lock();
                        JVM::can_cast_reference_type(&state, &actual_type_name, &target_class_name)
                    };

                    if !cast_ok {
                        return Err(format!(
                            "java.lang.ClassCastException: cannot cast {} to {}",
                            actual_type_name, target_class_name
                        )
                        .into());
                    }

                    pc += 3;
                }
                0xC6 | 0xC7 => {
                    // ifnull, ifnonnull
                    let offset =
                        (((bytecode[pc + 1] as i16) << 8) | (bytecode[pc + 2] as i16)) as i32;
                    let value = stack.pop().ok_or("ifnull/ifnonnull: stack underflow")?;

                    let is_null = matches!(value, JvmStackValue::Null);
                    let should_branch = match opcode {
                        0xC6 => is_null,
                        0xC7 => !is_null,
                        _ => unreachable!(),
                    };

                    if should_branch {
                        pc = (pc as i32 + offset) as usize;
                    } else {
                        pc += 3;
                    }
                }
                0xBB => {
                    // new (object creation)

                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    let ConstantInfo::Class(class_info) = &cp[cp_index - 1] else {
                        return Err(format!("new: Index #{} is not a Class", cp_index).into());
                    };

                    let ConstantInfo::Utf8(utf8_info) = &cp[class_info.name_index as usize - 1]
                    else {
                        return Err("new: Could not resolve class name string".into());
                    };

                    let class_name = utf8_info.utf8_string.clone();

                    let objectref = jvm.allocate(class_name);

                    stack.push(JvmStackValue::ObjectRef(objectref));

                    pc += 3;
                }
                0xBC => {
                    // newarray
                    let atype = bytecode[pc + 1];

                    let count = match stack.pop().ok_or("newarray: stack underflow")? {
                        JvmStackValue::Int(c) => c,
                        _ => return Err("newarray: expected Int for count".into()),
                    };

                    if count < 0 {
                        return Err("java.lang.NegativeArraySizeException".into());
                    }

                    let default_value = match atype {
                        4 | 5 | 8 | 9 | 10 => JvmStackValue::Int(0),
                        6 => JvmStackValue::Float(0.0),
                        7 => JvmStackValue::Double(0.0),
                        11 => JvmStackValue::Long(0),
                        _ => return Err(format!("newarray: invalid atype {}", atype).into()),
                    };

                    let array_obj = HeapObject::Array {
                        element_type: format!("primitive_{}", atype),
                        data: vec![default_value; count as usize],
                    };

                    let array_ref = {
                        let mut state = jvm.state.lock();
                        state.heap.push(array_obj);
                        (state.heap.len() - 1) as u32
                    };

                    stack.push(JvmStackValue::ObjectRef(array_ref));

                    pc += 2; // newarray is a 2-byte instruction
                }
                0xBD => {
                    // anewarray
                    let cp_index =
                        (((bytecode[pc + 1] as u16) << 8) | (bytecode[pc + 2] as u16)) as usize;

                    let component_type = match &cp[cp_index - 1] {
                        ConstantInfo::Class(class_info) => {
                            JVM::resolve_utf8(class_info.name_index, cp)
                        }
                        _ => return Err("anewarray: expected Class constant".into()),
                    };

                    let count = match stack.pop().ok_or("anewarray: stack underflow")? {
                        JvmStackValue::Int(c) => c,
                        _ => return Err("anewarray: expected Int for count".into()),
                    };

                    if count < 0 {
                        return Err("java.lang.NegativeArraySizeException".into());
                    }

                    jvm_debug!(
                        "anewarray: component type = {} | count = {}",
                        component_type,
                        count
                    );

                    let mut default_val = JvmStackValue::Null;

                    if component_type.starts_with("javax/") {
                        let mut state = jvm.state.lock();
                        let obj = JvmObject {
                            class_name: component_type.clone(),
                            fields: HashMap::new(),
                        };

                        state.heap.push(HeapObject::Instance(obj));
                        let id = (state.heap.len() - 1) as u32;

                        default_val = JvmStackValue::ObjectRef(id);
                    }

                    let array_obj = HeapObject::Array {
                        element_type: component_type.clone(),
                        data: vec![default_val; count as usize],
                    };

                    let array_ref = {
                        let mut state = jvm.state.lock();
                        state.heap.push(array_obj);
                        (state.heap.len() - 1) as u32
                    };

                    stack.push(JvmStackValue::ObjectRef(array_ref));

                    pc += 3;
                }
                0xC5 => {
                    // multianewarray
                    let cp_index =
                        (((bytecode[pc + 1] as u16) << 8) | (bytecode[pc + 2] as u16)) as usize;
                    let dimensions = bytecode[pc + 3] as usize;

                    if dimensions == 0 {
                        return Err("multianewarray: dimensions must be at least 1".into());
                    }

                    let array_type = match &cp[cp_index - 1] {
                        ConstantInfo::Class(class_info) => {
                            JVM::resolve_utf8(class_info.name_index, cp)
                        }
                        _ => return Err("multianewarray: expected Class constant".into()),
                    };

                    if !array_type.starts_with('[') {
                        return Err(format!(
                            "multianewarray: resolved type {} is not an array type",
                            array_type
                        ));
                    }

                    if JVM::array_type_rank(&array_type) < dimensions {
                        return Err(format!(
                            "multianewarray: type {} does not have {} dimensions",
                            array_type, dimensions
                        ));
                    }

                    let mut counts = Vec::with_capacity(dimensions);
                    for _ in 0..dimensions {
                        let count = match stack.pop().ok_or("multianewarray: stack underflow")? {
                            JvmStackValue::Int(i) => i,
                            value => {
                                return Err(format!(
                                    "multianewarray: dimension count is not an int: {:?}",
                                    value
                                ));
                            }
                        };

                        if count < 0 {
                            return Err("java.lang.NegativeArraySizeException".into());
                        }

                        counts.push(count);
                    }
                    counts.reverse();

                    let array_ref = {
                        let mut state = jvm.state.lock();
                        JVM::allocate_multianewarray(&mut state, &array_type, &counts)?
                    };

                    stack.push(JvmStackValue::ObjectRef(array_ref));

                    pc += 4;
                }
                0xBE => {
                    // arraylength
                    let arrayref = stack.pop().ok_or("arraylength: stack underflow")?;

                    jvm_debug!("arraylength: arrayref = {:?}", arrayref);

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("arraylength: expected reference".into()),
                    };

                    {
                        let state = jvm.state.lock();
                        match state.heap.get(heap_idx) {
                            Some(HeapObject::Array { data, .. }) => {
                                stack.push(JvmStackValue::Int(data.len() as i32));
                            }
                            Some(HeapObject::Instance(_)) => {
                                return Err("IncompatibleClassChangeError: expected array".into());
                            }
                            None => return Err("arraylength: invalid heap reference".into()),
                        }
                    }

                    pc += 1;
                }
                0xB1 => {
                    // return
                    jvm_debug!("Execution finished normally.");
                    return Ok(None);
                }
                0x85 => {
                    // i2l
                    let val = stack.pop().ok_or("i2l: stack underflow")?;
                    if let JvmStackValue::Int(i) = val {
                        stack.push(JvmStackValue::Long(i as i64));
                    } else {
                        return Err("i2l: expected Int".into());
                    }
                    pc += 1;
                }
                0x70 => {
                    // irem
                    let val2 = stack.pop().ok_or("irem: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("irem: Stack underflow for val1")?;

                    if let (JvmStackValue::Int(i1), JvmStackValue::Int(i2)) =
                        (val1.clone(), val2.clone())
                    {
                        if i2 == 0 {
                            return Err("java.lang.ArithmeticException: Division by zero".into());
                        }
                        stack.push(JvmStackValue::Int(i1 % i2));
                    } else {
                        return Err(format!(
                            "irem: Expected two Ints on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }

                    pc += 1;
                }
                0x71 => {
                    // lrem
                    let val2 = stack.pop().ok_or("lrem: Stack underflow for val2")?;
                    let val1 = stack.pop().ok_or("lrem: Stack underflow for val1")?;

                    if let (JvmStackValue::Long(l1), JvmStackValue::Long(l2)) =
                        (val1.clone(), val2.clone())
                    {
                        if l2 == 0 {
                            return Err("java.lang.ArithmeticException: Division by zero".into());
                        }
                        stack.push(JvmStackValue::Long(l1 % l2));
                    } else {
                        return Err(format!(
                            "lrem: Expected two Longs on stack, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }

                    pc += 1;
                }
                0x74 => {
                    // ineg
                    let val = stack.pop().ok_or("ineg: stack underflow")?;
                    if let JvmStackValue::Int(i) = val {
                        stack.push(JvmStackValue::Int(i.wrapping_neg()));
                    } else {
                        return Err(format!("ineg: expected Int, found {:?}", val).into());
                    }
                    pc += 1;
                }
                0x78 => {
                    // ishl
                    let val2 = stack.pop().ok_or("ishl: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("ishl: stack underflow (val1)")?;

                    if let (JvmStackValue::Int(v1), JvmStackValue::Int(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        let s = (v2 & 0x1f) as u32;
                        stack.push(JvmStackValue::Int(v1.wrapping_shl(s)));
                    } else {
                        return Err(format!(
                            "ishl: expected two Ints, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x7E => {
                    // iand
                    let val2 = stack.pop().ok_or("iand: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("iand: stack underflow (val1)")?;

                    if let (JvmStackValue::Int(v1), JvmStackValue::Int(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        let s = v1 & v2;
                        stack.push(JvmStackValue::Int(s));
                    } else {
                        return Err(format!(
                            "iand: expected two Ints, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1
                }
                0x15 | 0x16 | 0x17 | 0x18 | 0x19 => {
                    // iload, lload, fload, dload, aload
                    let index = bytecode[pc + 1] as usize;
                    if index >= locals.len() {
                        return Err(format!("load: Invalid local index {}", index).into());
                    }
                    stack.push(locals[index].clone());
                    pc += 2;
                }
                0xAA => {
                    // tableswitch
                    let opcode_pc = pc;
                    let aligned_pc = (pc + 4) & !3;

                    if aligned_pc + 12 > bytecode.len() {
                        return Err("tableswitch: bytecode truncated".into());
                    }

                    let default_offset = i32::from_be_bytes([
                        bytecode[aligned_pc],
                        bytecode[aligned_pc + 1],
                        bytecode[aligned_pc + 2],
                        bytecode[aligned_pc + 3],
                    ]);
                    let low = i32::from_be_bytes([
                        bytecode[aligned_pc + 4],
                        bytecode[aligned_pc + 5],
                        bytecode[aligned_pc + 6],
                        bytecode[aligned_pc + 7],
                    ]);
                    let high = i32::from_be_bytes([
                        bytecode[aligned_pc + 8],
                        bytecode[aligned_pc + 9],
                        bytecode[aligned_pc + 10],
                        bytecode[aligned_pc + 11],
                    ]);

                    let index = match stack.pop().ok_or("tableswitch: stack underflow")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("tableswitch: expected Int on stack".into()),
                    };

                    let offset = if index < low || index > high {
                        default_offset
                    } else {
                        let table_index = (index - low) as usize;
                        let entry_pc = aligned_pc + 12 + table_index * 4;
                        if entry_pc + 4 > bytecode.len() {
                            return Err("tableswitch: jump table truncated".into());
                        }
                        i32::from_be_bytes([
                            bytecode[entry_pc],
                            bytecode[entry_pc + 1],
                            bytecode[entry_pc + 2],
                            bytecode[entry_pc + 3],
                        ])
                    };

                    pc = (opcode_pc as i32 + offset) as usize;
                    continue;
                }
                0xAB => {
                    // lookupswitch
                    let opcode_pc = pc;
                    let aligned_pc = (pc + 4) & !3;

                    if aligned_pc + 8 > bytecode.len() {
                        return Err("lookupswitch: bytecode truncated".into());
                    }

                    let default_offset = i32::from_be_bytes([
                        bytecode[aligned_pc],
                        bytecode[aligned_pc + 1],
                        bytecode[aligned_pc + 2],
                        bytecode[aligned_pc + 3],
                    ]);
                    let npairs = i32::from_be_bytes([
                        bytecode[aligned_pc + 4],
                        bytecode[aligned_pc + 5],
                        bytecode[aligned_pc + 6],
                        bytecode[aligned_pc + 7],
                    ]);

                    let key = match stack.pop().ok_or("lookupswitch: stack underflow")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("lookupswitch: expected Int on stack".into()),
                    };

                    let mut offset = default_offset;
                    for i in 0..npairs {
                        let pair_pc = aligned_pc + 8 + (i as usize * 8);
                        if pair_pc + 8 > bytecode.len() {
                            return Err("lookupswitch: pairs truncated".into());
                        }
                        let match_val = i32::from_be_bytes([
                            bytecode[pair_pc],
                            bytecode[pair_pc + 1],
                            bytecode[pair_pc + 2],
                            bytecode[pair_pc + 3],
                        ]);
                        if match_val == key {
                            offset = i32::from_be_bytes([
                                bytecode[pair_pc + 4],
                                bytecode[pair_pc + 5],
                                bytecode[pair_pc + 6],
                                bytecode[pair_pc + 7],
                            ]);
                            break;
                        }
                    }

                    pc = (opcode_pc as i32 + offset) as usize;
                    continue;
                }
                0xAC | 0xAF => {
                    // ireturn
                    let val = stack.pop().ok_or("return: Stack underflow")?;

                    jvm_debug!("Execution finished with return value: {:?}", val);
                    return Ok(Some(val));
                }
                0x36 | 0x37 | 0x38 | 0x39 | 0x3a => {
                    // istore, lstore, fstore, dstore, astore
                    let index = bytecode[pc + 1] as usize;
                    let val = stack.pop().ok_or("store: stack underflow")?;
                    if locals.len() <= index {
                        locals.resize(index + 1, JvmStackValue::Null);
                    }
                    locals[index] = val;
                    pc += 2;
                }
                _ => {
                    println!("Unknown Opcode: {:02X}", opcode);
                    panic!("Unknown Opcode: {:02X}", opcode);
                }
            }
        }

        return Ok(None);
    }

    fn get_field_key(field: &FieldRefConstant, pool: &[ConstantInfo]) -> String {
        let ConstantInfo::Class(class_info) = &pool[field.class_index as usize - 1] else {
            panic!()
        };
        let ConstantInfo::Utf8(class_utf8) = &pool[class_info.name_index as usize - 1] else {
            panic!()
        };

        let ConstantInfo::NameAndType(nt_info) = &pool[field.name_and_type_index as usize - 1]
        else {
            panic!()
        };
        let ConstantInfo::Utf8(name_utf8) = &pool[nt_info.name_index as usize - 1] else {
            panic!()
        };
        let ConstantInfo::Utf8(desc_utf8) = &pool[nt_info.descriptor_index as usize - 1] else {
            panic!()
        };

        format!(
            "{}.{}:{}",
            class_utf8.utf8_string, name_utf8.utf8_string, desc_utf8.utf8_string
        )
    }

    fn count_arguments(descriptor: &str) -> usize {
        // A simplified parser: count types inside the parentheses
        // (Ljava/lang/String;I)V -> 2
        let mut count = 0;
        let mut in_params = false;
        let mut chars = descriptor.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '(' => in_params = true,
                ')' => break,
                'L' => {
                    // Object type: consume until ';'
                    if in_params {
                        count += 1;
                    }
                    while chars.next() != Some(';') {}
                }
                '[' => { // Array: consume next type but don't count it as a separate arg
                    // The array itself is one objectref
                }
                'J' | 'D' => {
                    // Long or Double take two slots in some VM contexts,
                    // but for our Value stack, we treat them as 1 item.
                    if in_params {
                        count += 1;
                    }
                }
                'I' | 'F' | 'B' | 'C' | 'S' | 'Z' => {
                    if in_params {
                        count += 1;
                    }
                }
                _ => {}
            }
        }
        count
    }

    fn resolve_method_identity(
        m: &MethodRefConstant,
        pool: &[ConstantInfo],
    ) -> (String, String, String) {
        let ConstantInfo::Class(class_info) = &pool[m.class_index as usize - 1] else {
            panic!()
        };
        let ConstantInfo::Utf8(class_utf8) = &pool[class_info.name_index as usize - 1] else {
            panic!()
        };

        let ConstantInfo::NameAndType(nt_info) = &pool[m.name_and_type_index as usize - 1] else {
            panic!()
        };
        let ConstantInfo::Utf8(name_utf8) = &pool[nt_info.name_index as usize - 1] else {
            panic!()
        };
        let ConstantInfo::Utf8(desc_utf8) = &pool[nt_info.descriptor_index as usize - 1] else {
            panic!()
        };

        (
            class_utf8.utf8_string.clone(),
            name_utf8.utf8_string.clone(),
            desc_utf8.utf8_string.clone(),
        )
    }

    fn handle_vector_fns(
        object_ref: &mut HeapObject,
        method: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        let heap_obj = if let HeapObject::Instance(obj) = object_ref {
            obj
        } else {
            return Err("Expected instance for Vector object".into());
        };

        let vector: &mut Vec<JvmStackValue> = {
            if let JvmStackValue::Vector(vec) = heap_obj.fields.get_mut("container").unwrap() {
                vec
            } else {
                return Err("Vector instance missing 'container' field".into());
            }
        };

        match method {
            "addElement" => {
                assert_eq!(
                    args.len(),
                    1,
                    "Vector.addElement expected 1 argument, got {}",
                    args.len()
                );
                let val = args[0].clone();
                vector.push(val);
                Ok(None)
            }
            "elementAt" => {
                let Some(JvmStackValue::Int(index)) = args.get(0) else {
                    return Err(format!(
                        "Vector.elementAt(I): invalid arg {:?}",
                        args.get(0)
                    ));
                };

                let index = *index as usize;
                let value = vector
                    .get(index)
                    .ok_or_else(|| format!("Vector.elementAt: index out of bounds: {}", index))?
                    .clone();

                Ok(Some(value))
            }
            "size" => Ok(Some(JvmStackValue::Int(vector.len() as i32))),
            "removeAllElements" => {
                vector.clear();
                Ok(None)
            }
            "<init>" => {
                // Initialize the 'container' field to an empty vector
                heap_obj
                    .fields
                    .insert("container".to_string(), JvmStackValue::Vector(Vec::new()));
                Ok(None)
            }
            _ => {
                println!("[-] Unknown Vector method: {} | args = {:?}", method, args);
                panic!();
            }
        }
    }

    fn handle_native_printstream(method: &str, args: &[JvmStackValue]) {
        match method {
            "println" => {
                if let Some(val) = args.get(0) {
                    match val {
                        JvmStackValue::String(s) => println!("{}", s),
                        JvmStackValue::Int(i) => println!("{}", i),
                        JvmStackValue::Float(f) => println!("{}", f),
                        JvmStackValue::Long(l) => println!("{}", l),
                        JvmStackValue::Double(d) => println!("{}", d),
                        _ => println!("{:?}", val),
                    }
                } else {
                    println!();
                }
            }
            "print" => {
                if let Some(val) = args.get(0) {
                    match val {
                        JvmStackValue::String(s) => print!("{}", s),
                        JvmStackValue::Int(i) => print!("{}", i),
                        JvmStackValue::Float(f) => print!("{}", f),
                        JvmStackValue::Long(l) => print!("{}", l),
                        JvmStackValue::Double(d) => print!("{}", d),
                        _ => print!("{:?}", val),
                    }
                }
            }
            _ => println!("Native PrintStream called unknown method: {}", method),
        }
    }

    pub fn execute_method(
        objectref: JvmStackValue,
        class_name: &str,
        method_name: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
        caller_stack: &mut Vec<JvmStackValue>,
    ) -> Result<(), String> {
        jvm_debug!(
            "Executing method: {}.{}{} with args {:?}",
            class_name,
            method_name,
            descriptor,
            args
        );
        if class_name.starts_with("javax/microedition") {
            if class_name == image::CLASS_NAME {
                let return_value =
                    image::handle_virtual_method(objectref, method_name, descriptor, jvm);

                let res = match return_value {
                    Ok(val) => val,
                    Err(e) => {
                        return Err(format!("Error handling Image method: {}", e).into());
                    }
                };

                if let Some(val) = res {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == display::CLASS_NAME {
                let return_value =
                    display::handle_virtual_method(objectref, method_name, descriptor, args, jvm);

                if let Err(e) = &return_value {
                    return Err(format!("Error handling Display method: {}", e).into());
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == midlet::CLASS_NAME {
                let return_value = midlet::handle_virtual_method(method_name, descriptor, args);

                if let Err(e) = &return_value {
                    return Err(format!("Error handling MIDlet method: {}", e).into());
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == game_canvas::CLASS_NAME
                || class_name == "javax/microedition/lcdui/Canvas"
            {
                let return_value = {
                    let mut state = jvm.state.lock();
                    let heap_idx = if let JvmStackValue::ObjectRef(id) = objectref {
                        id as usize
                    } else {
                        return Err(
                            "Canvas/GameCanvas method call with non-reference object".into()
                        );
                    };

                    // Temporarily replace the object with a dummy to avoid multiple mutable borrows
                    let mut obj = std::mem::replace(
                        &mut state.heap[heap_idx],
                        HeapObject::Array {
                            element_type: "".into(),
                            data: vec![],
                        },
                    );
                    let res = if let HeapObject::Instance(ref mut inst) = obj {
                        game_canvas::handle_virtual_method(
                            inst,
                            method_name,
                            descriptor,
                            args,
                            &jvm,
                        )
                    } else {
                        Err(
                            "Canvas/GameCanvas method call on non-instance object or invalid ref"
                                .into(),
                        )
                    };
                    // Put it back!
                    state.heap[heap_idx] = obj;
                    res
                };

                if let Err(e) = &return_value {
                    return Err(format!("Error handling Canvas/GameCanvas method: {}", e).into());
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == graphics::CLASS_NAME {
                let return_value =
                    graphics::handle_virtual_method(&objectref, method_name, descriptor, args, jvm);

                if let Err(e) = &return_value {
                    return Err(format!("Error handling Graphics method: {}", e).into());
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            panic!(
                "[-] ExeVirtualMethod: Skipping virtual method call to {}.{}{}",
                class_name, method_name, descriptor
            );
        } else if class_name == "java/io/PrintStream" {
            JVM::handle_native_printstream(method_name, args);
            return Ok(());
        } else if class_name == "java/lang/String" {
            let return_value = JVM::handle_string_fns(objectref, method_name, descriptor, args)?;

            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Thread" && method_name == "<init>" {
            return Ok(());
        } else if class_name == "java/util/Vector" {
            let this_id = match objectref {
                JvmStackValue::ObjectRef(id) => id,
                _ => return Err("Vector: NullPointerException".into()),
            };

            let res = {
                let mut state = jvm.state.lock();
                let object_ref = state
                    .heap
                    .get_mut(this_id as usize)
                    .ok_or_else(|| format!("Invalid heap reference: {}", this_id))?;
                JVM::handle_vector_fns(object_ref, method_name, args)
            };

            if let Err(e) = res {
                return Err(format!("Error handling Vector method: {}", e).into());
            }

            let return_value = res.unwrap();
            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/StringBuffer" {
            let this_id = match objectref {
                JvmStackValue::ObjectRef(id) => id,
                _ => return Err("StringBuffer: NullPointerException".into()),
            };

            let mut call_args: Vec<JvmStackValue> = args.into();
            call_args.insert(0, objectref.clone());

            let res = {
                let mut state = jvm.state.lock();
                let object_ref = state
                    .heap
                    .get_mut(this_id as usize)
                    .ok_or_else(|| format!("Invalid heap reference: {}", this_id))?;
                JVM::handle_str_buffer_fns(object_ref, method_name, &call_args)
            };

            if let Err(e) = res {
                return Err(format!("Error handling StringBuffer method: {}", e).into());
            }

            let return_value = res.unwrap();
            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Runtime" {
            let res = JVM::handle_runtime_fns(method_name, descriptor, args);
            if let Err(e) = res {
                return Err(format!("Error handling Runtime method: {}", e).into());
            }
            if let Some(val) = res.unwrap() {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Class" {
            let res = JVM::handle_class_fns(method_name, descriptor, args, jvm);
            if let Err(e) = res {
                return Err(format!("Error handling Class method: {}", e).into());
            }
            if let Some(val) = res.unwrap() {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/io/ByteArrayInputStream" {
            let mut call_args: Vec<JvmStackValue> = args.into();
            call_args.insert(0, objectref.clone());

            let res =
                JVM::handle_byte_array_input_stream_fns(method_name, descriptor, &call_args, &jvm);

            if let Err(e) = res {
                return Err(format!("Error handling ByteArrayInputStream method: {}", e).into());
            }
            if let Some(val) = res.unwrap() {
                caller_stack.push(val);
            }

            return Ok(());
        } else if method_name == "getClass" && descriptor == "()Ljava/lang/Class;" {
            // Handle getClass() for any object by returning a dummy Class reference
            let return_value = {
                let mut state = jvm.state.lock();
                let heap_idx = if let JvmStackValue::ObjectRef(id) = objectref {
                    id as usize
                } else {
                    return Err("getClass: expected object reference".into());
                };

                // Create a dummy Class object with the class name as a field
                let class_name_str = if let HeapObject::Instance(inst) = &state.heap[heap_idx] {
                    inst.class_name.clone()
                } else {
                    return Err("getClass: expected instance object".into());
                };

                let class_obj = JvmObject {
                    class_name: "java/lang/Class".to_string(),
                    fields: {
                        let mut f = HashMap::new();
                        f.insert("name".to_string(), JvmStackValue::String(class_name_str));
                        f
                    },
                };

                state.heap.push(HeapObject::Instance(class_obj));
                let class_ref = (state.heap.len() - 1) as u32;

                JvmStackValue::ObjectRef(class_ref)
            };

            caller_stack.push(return_value);
            return Ok(());
        }

        let Some((_resolved_class_name, const_pool, code_attr)) =
            JVM::find_method_code_in_hierarchy(jvm, class_name, method_name, descriptor)?
        else {
            if JVM::class_extends(jvm, class_name, game_canvas::CLASS_NAME)
                || JVM::class_extends(jvm, class_name, "javax/microedition/lcdui/Canvas")
            {
                let return_value = {
                    let mut state = jvm.state.lock();
                    let heap_idx = if let JvmStackValue::ObjectRef(id) = objectref {
                        id as usize
                    } else {
                        return Err(
                            "Canvas/GameCanvas method call with non-reference object".into()
                        );
                    };

                    let mut obj = std::mem::replace(
                        &mut state.heap[heap_idx],
                        HeapObject::Array {
                            element_type: "".into(),
                            data: vec![],
                        },
                    );
                    let res = if let HeapObject::Instance(ref mut inst) = obj {
                        game_canvas::handle_virtual_method(
                            inst,
                            method_name,
                            descriptor,
                            args,
                            &jvm,
                        )
                    } else {
                        Err(
                            "Canvas/GameCanvas method call on non-instance object or invalid ref"
                                .into(),
                        )
                    };
                    state.heap[heap_idx] = obj;
                    res?
                };

                if let Some(val) = return_value {
                    caller_stack.push(val);
                }

                return Ok(());
            }

            let is_thread_start = method_name == "start" && descriptor == "()V";
            let is_thread_set_priority = method_name == "setPriority" && descriptor == "(I)V";
            let is_thread_join = method_name == "join" && descriptor == "()V";
            let is_thread_is_alive = method_name == "isAlive" && descriptor == "()Z";
            let is_thread_yield = method_name == "yield" && descriptor == "()V";

            if JVM::class_extends(jvm, class_name, "java/lang/Thread") {
                if is_thread_start {
                    // Spawn a real OS thread
                    let jvm_clone = jvm.clone();
                    let objectref_clone = objectref.clone();
                    let class_name_owned = class_name.to_string();

                    let obj_id = match &objectref {
                        JvmStackValue::ObjectRef(id) => *id,
                        _ => 0,
                    };

                    let handle = std::thread::spawn(move || {
                        println!("Spawned a thread for {}", class_name_owned);
                        let result = JVM::execute_method(
                            objectref_clone,
                            &class_name_owned,
                            "run",
                            "()V",
                            &[],
                            &jvm_clone,
                            &mut Vec::new(),
                        );
                        if let Err(e) = result {
                            eprintln!(
                                "[JVM Thread Error] {}.run() failed: {}",
                                class_name_owned, e
                            );
                            panic!("Thread execution failed");
                        }
                    });

                    // Store handle for join()
                    jvm.thread_handles.lock().insert(obj_id, handle);

                    return Ok(());
                } else if is_thread_yield {
                    return Ok(());
                } else if is_thread_join {
                    let obj_id = match &objectref {
                        JvmStackValue::ObjectRef(id) => *id,
                        _ => return Err("Thread.join: not an object ref".into()),
                    };
                    let handle = jvm.thread_handles.lock().remove(&obj_id);
                    if let Some(h) = handle {
                        h.join()
                            .map_err(|_| "Thread.join: thread panicked".to_string())?;
                    }
                    return Ok(());
                } else if is_thread_is_alive {
                    let obj_id = match &objectref {
                        JvmStackValue::ObjectRef(id) => *id,
                        _ => return Err("Thread.isAlive: not an object ref".into()),
                    };
                    let is_alive = {
                        let handles = jvm.thread_handles.lock();
                        if let Some(handle) = handles.get(&obj_id) {
                            !handle.is_finished()
                        } else {
                            false
                        }
                    };
                    caller_stack.push(JvmStackValue::Int(if is_alive { 1 } else { 0 }));
                    return Ok(());
                } else if is_thread_set_priority {
                    return Ok(());
                }
            }

            return Err(format!(
                "[ExecMethod] Method not found: {}.{}{}",
                class_name, method_name, descriptor
            ));
        };

        let mut locals = vec![JvmStackValue::Null; code_attr.max_locals as usize];

        if !locals.is_empty() {
            locals[0] = objectref.clone();
        }

        let mut local_idx = 1;
        for arg in args {
            if local_idx < locals.len() {
                locals[local_idx] = arg.clone();
            }

            match arg {
                JvmStackValue::Long(_) | JvmStackValue::Double(_) => {
                    local_idx += 2;
                }
                _ => {
                    local_idx += 1;
                }
            }
        }

        let return_value = JVM::run_frame(&code_attr.code, &const_pool, &mut locals, jvm)?;

        if let Some(val) = return_value {
            caller_stack.push(val);
        }

        Ok(())
    }

    fn get_code_attribute(
        method: &classfile_parser::method_info::MethodInfo,
        pool: &[ConstantInfo],
    ) -> Option<Code> {
        for attr in &method.attributes {
            let ConstantInfo::Utf8(name_info) = &pool[attr.attribute_name_index as usize - 1]
            else {
                continue;
            };

            if name_info.utf8_string == "Code" {
                // classfile-parser doesn't always auto-parse the attribute body,
                // so we use its internal parser to turn the raw bytes into a CodeAttribute.
                if let Ok((_, code_attr)) =
                    classfile_parser::attribute_info::code_attribute_parser(&attr.info)
                {
                    return Some(Code {
                        max_stack: code_attr.max_stack,
                        max_locals: code_attr.max_locals,
                        code: code_attr.code,
                    });
                }
            }
        }

        None
    }

    fn find_method_in_class<'a>(
        class: &'a classfile_parser::ClassFile,
        method_name: &str,
        descriptor: &str,
    ) -> Option<&'a classfile_parser::method_info::MethodInfo> {
        let pool = &class.const_pool;

        for method in &class.methods {
            let ConstantInfo::Utf8(name_info) = &pool[method.name_index as usize - 1] else {
                continue;
            };

            let ConstantInfo::Utf8(desc_info) = &pool[method.descriptor_index as usize - 1] else {
                continue;
            };

            if name_info.utf8_string == method_name && desc_info.utf8_string == descriptor {
                return Some(method);
            }
        }
        None
    }

    fn find_method_code_in_hierarchy(
        jvm: &JVM,
        class_name: &str,
        method_name: &str,
        descriptor: &str,
    ) -> Result<Option<(String, Vec<ConstantInfo>, Code)>, String> {
        let mut current_class = Some(class_name.to_string());
        let state = jvm.state.lock();

        while let Some(name) = current_class {
            let Some(class_data) = state.classes.get(&name) else {
                return Ok(None);
            };

            if let Some(method) = JVM::find_method_in_class(class_data, method_name, descriptor) {
                let code_attr = JVM::get_code_attribute(method, &class_data.const_pool)
                    .ok_or_else(|| {
                        format!(
                            "Method has no Code attribute: {}.{}{}",
                            name, method_name, descriptor
                        )
                    })?;

                return Ok(Some((name, class_data.const_pool.clone(), code_attr)));
            }

            current_class = JVM::get_super_class_name(class_data);
        }

        Ok(None)
    }

    fn class_extends(jvm: &JVM, class_name: &str, target_class_name: &str) -> bool {
        let mut current_class = Some(class_name.to_string());
        let state = jvm.state.lock();

        while let Some(name) = current_class {
            if name == target_class_name {
                return true;
            }

            let Some(class_data) = state.classes.get(&name) else {
                return false;
            };

            current_class = JVM::get_super_class_name(class_data);
        }

        false
    }

    fn class_extends_in_state(state: &JvmState, class_name: &str, target_class_name: &str) -> bool {
        let mut current_class = Some(class_name.to_string());

        while let Some(name) = current_class {
            if name == target_class_name {
                return true;
            }

            let Some(class_data) = state.classes.get(&name) else {
                return false;
            };

            current_class = JVM::get_super_class_name(class_data);
        }

        false
    }

    fn class_implements_interface_in_state(
        state: &JvmState,
        class_name: &str,
        target_interface_name: &str,
    ) -> bool {
        let Some(class_data) = state.classes.get(class_name) else {
            return false;
        };

        if class_data
            .access_flags
            .contains(classfile_parser::ClassAccessFlags::INTERFACE)
        {
            if class_name == target_interface_name {
                return true;
            }

            for interface_idx in &class_data.interfaces {
                let Some(ConstantInfo::Class(interface_class)) =
                    class_data.const_pool.get(*interface_idx as usize - 1)
                else {
                    continue;
                };

                let interface_name =
                    JVM::resolve_utf8(interface_class.name_index, &class_data.const_pool);
                if interface_name == target_interface_name
                    || JVM::class_implements_interface_in_state(
                        state,
                        &interface_name,
                        target_interface_name,
                    )
                {
                    return true;
                }
            }

            return false;
        }

        for interface_idx in &class_data.interfaces {
            let Some(ConstantInfo::Class(interface_class)) =
                class_data.const_pool.get(*interface_idx as usize - 1)
            else {
                continue;
            };

            let interface_name =
                JVM::resolve_utf8(interface_class.name_index, &class_data.const_pool);
            if interface_name == target_interface_name
                || JVM::class_implements_interface_in_state(
                    state,
                    &interface_name,
                    target_interface_name,
                )
            {
                return true;
            }
        }

        if let Some(super_name) = JVM::get_super_class_name(class_data) {
            return JVM::class_implements_interface_in_state(
                state,
                &super_name,
                target_interface_name,
            );
        }

        false
    }

    fn is_interface_in_state(state: &JvmState, class_name: &str) -> bool {
        state
            .classes
            .get(class_name)
            .map(|class_data| {
                class_data
                    .access_flags
                    .contains(classfile_parser::ClassAccessFlags::INTERFACE)
            })
            .unwrap_or(false)
    }

    fn array_runtime_type_name(element_type: &str) -> String {
        if element_type.starts_with("primitive_") {
            match element_type {
                "primitive_4" => "[Z".to_string(),
                "primitive_5" => "[C".to_string(),
                "primitive_6" => "[F".to_string(),
                "primitive_7" => "[D".to_string(),
                "primitive_8" => "[B".to_string(),
                "primitive_9" => "[S".to_string(),
                "primitive_10" => "[I".to_string(),
                "primitive_11" => "[J".to_string(),
                _ => format!("[?{}", element_type),
            }
        } else if element_type.starts_with('[') {
            format!("[{}", element_type)
        } else {
            format!("[L{};", element_type)
        }
    }

    fn array_component_type(array_type: &str) -> Option<&str> {
        array_type.strip_prefix('[')
    }

    fn multianewarray_default_value(component_type: &str) -> JvmStackValue {
        match component_type {
            "Z" | "B" | "C" | "S" | "I" => JvmStackValue::Int(0),
            "F" => JvmStackValue::Float(0.0),
            "D" => JvmStackValue::Double(0.0),
            "J" => JvmStackValue::Long(0),
            _ => JvmStackValue::Null,
        }
    }

    fn multianewarray_element_type(component_type: &str) -> String {
        match component_type {
            "Z" => "primitive_4".to_string(),
            "C" => "primitive_5".to_string(),
            "F" => "primitive_6".to_string(),
            "D" => "primitive_7".to_string(),
            "B" => "primitive_8".to_string(),
            "S" => "primitive_9".to_string(),
            "I" => "primitive_10".to_string(),
            "J" => "primitive_11".to_string(),
            _ if component_type.starts_with('[') => component_type.to_string(),
            _ if component_type.starts_with('L') && component_type.ends_with(';') => {
                component_type[1..component_type.len() - 1].to_string()
            }
            _ => component_type.to_string(),
        }
    }

    fn array_type_rank(array_type: &str) -> usize {
        array_type.chars().take_while(|ch| *ch == '[').count()
    }

    fn allocate_multianewarray(
        state: &mut JvmState,
        array_type: &str,
        counts: &[i32],
    ) -> Result<u32, String> {
        let Some(component_type) = JVM::array_component_type(array_type) else {
            return Err(format!(
                "multianewarray: {} is not an array type",
                array_type
            ));
        };

        if counts.is_empty() {
            return Err("multianewarray: missing dimensions".into());
        }

        let count = counts[0];
        if count < 0 {
            return Err("java.lang.NegativeArraySizeException".into());
        }

        let element_type = JVM::multianewarray_element_type(component_type);
        let data = if counts.len() == 1 {
            vec![JVM::multianewarray_default_value(component_type); count as usize]
        } else {
            if !component_type.starts_with('[') {
                return Err(format!(
                    "multianewarray: {} does not have enough dimensions for {:?}",
                    array_type, counts
                ));
            }

            let mut data = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let sub_ref = JVM::allocate_multianewarray(state, component_type, &counts[1..])?;
                data.push(JvmStackValue::ObjectRef(sub_ref));
            }
            data
        };

        state.heap.push(HeapObject::Array { element_type, data });
        Ok((state.heap.len() - 1) as u32)
    }

    fn can_cast_array_type(
        state: &JvmState,
        actual_array_type: &str,
        target_array_type: &str,
    ) -> bool {
        if actual_array_type == target_array_type {
            return true;
        }

        let Some(actual_component) = JVM::array_component_type(actual_array_type) else {
            return false;
        };
        let Some(target_component) = JVM::array_component_type(target_array_type) else {
            return false;
        };

        let actual_is_primitive =
            !actual_component.starts_with('[') && !actual_component.starts_with('L');
        let target_is_primitive =
            !target_component.starts_with('[') && !target_component.starts_with('L');

        match (actual_is_primitive, target_is_primitive) {
            (true, true) => actual_component == target_component,
            (false, false) => {
                let actual_component_name = actual_component
                    .strip_prefix('L')
                    .and_then(|name| name.strip_suffix(';'))
                    .unwrap_or(actual_component);
                let target_component_name = target_component
                    .strip_prefix('L')
                    .and_then(|name| name.strip_suffix(';'))
                    .unwrap_or(target_component);

                JVM::can_cast_reference_type(state, actual_component_name, target_component_name)
            }
            _ => false,
        }
    }

    fn can_cast_reference_type(
        state: &JvmState,
        actual_type_name: &str,
        target_type_name: &str,
    ) -> bool {
        if actual_type_name == target_type_name {
            return true;
        }

        if actual_type_name.starts_with('[') {
            return match target_type_name {
                "java/lang/Object" | "java/lang/Cloneable" | "java/io/Serializable" => true,
                _ if target_type_name.starts_with('[') => {
                    JVM::can_cast_array_type(state, actual_type_name, target_type_name)
                }
                _ => false,
            };
        }

        if target_type_name.starts_with('[') {
            return false;
        }

        let actual_is_interface = JVM::is_interface_in_state(state, actual_type_name);
        let target_is_interface = JVM::is_interface_in_state(state, target_type_name);

        match (actual_is_interface, target_is_interface) {
            (false, false) => {
                JVM::class_extends_in_state(state, actual_type_name, target_type_name)
            }
            (false, true) => {
                JVM::class_implements_interface_in_state(state, actual_type_name, target_type_name)
            }
            (true, false) => target_type_name == "java/lang/Object",
            (true, true) => {
                if actual_type_name == target_type_name {
                    return true;
                }

                if let Some(actual_class) = state.classes.get(actual_type_name) {
                    for interface_idx in &actual_class.interfaces {
                        let Some(ConstantInfo::Class(interface_class)) =
                            actual_class.const_pool.get(*interface_idx as usize - 1)
                        else {
                            continue;
                        };

                        let interface_name =
                            JVM::resolve_utf8(interface_class.name_index, &actual_class.const_pool);
                        if interface_name == target_type_name
                            || JVM::class_implements_interface_in_state(
                                state,
                                &interface_name,
                                target_type_name,
                            )
                        {
                            return true;
                        }
                    }
                }

                false
            }
        }
    }

    pub fn get_class_name(class: &classfile_parser::ClassFile) -> Result<String, String> {
        let pool = &class.const_pool;

        let this_class_idx = class.this_class as usize;

        let ConstantInfo::Class(class_info) = &pool[this_class_idx - 1] else {
            return Err("this_class index did not point to a Class constant".into());
        };

        let ConstantInfo::Utf8(utf8_info) = &pool[class_info.name_index as usize - 1] else {
            return Err("Class name_index did not point to a Utf8 constant".into());
        };

        Ok(utf8_info.utf8_string.clone())
    }

    fn resolve_field_name(field: &FieldRefConstant, pool: &[ConstantInfo]) -> String {
        let ConstantInfo::NameAndType(nt_info) = &pool[field.name_and_type_index as usize - 1]
        else {
            panic!()
        };
        let ConstantInfo::Utf8(name_utf8) = &pool[nt_info.name_index as usize - 1] else {
            panic!()
        };

        let ConstantInfo::Utf8(desc_utf8) = &pool[nt_info.descriptor_index as usize - 1] else {
            panic!()
        };

        let key = format!("{}:{}", name_utf8.utf8_string, desc_utf8.utf8_string);

        return key;
    }

    pub fn allocate(&self, class_name: String) -> u32 {
        let fields = {
            let mut fields = HashMap::new();
            if class_name == "java/util/Vector" {
                fields.insert("container".to_string(), JvmStackValue::Vector(Vec::new()));
            } else {
                let mut current_class = Some(class_name.clone());
                let state = self.state.lock();
                while let Some(name) = current_class {
                    if let Some(class_data) = state.classes.get(&name) {
                        for field_info in &class_data.fields {
                            let descriptor = JVM::resolve_utf8(
                                field_info.descriptor_index,
                                &class_data.const_pool,
                            );
                            let default_val =
                                if descriptor.starts_with('L') || descriptor.starts_with('[') {
                                    JvmStackValue::Null
                                } else {
                                    JvmStackValue::Int(0)
                                };
                            let f_name =
                                JVM::resolve_utf8(field_info.name_index, &class_data.const_pool);
                            let key = format!("{}:{}", f_name, descriptor);
                            fields.insert(key, default_val);
                        }
                        current_class = JVM::get_super_class_name(class_data);
                    } else {
                        current_class = None;
                    }
                }
            }
            fields
        };

        let mut state = self.state.lock();
        JVM::allocate_internal(&mut state, class_name, fields)
    }

    pub fn allocate_internal(
        state: &mut JvmState,
        class_name: String,
        fields: HashMap<String, JvmStackValue>,
    ) -> u32 {
        state
            .heap
            .push(HeapObject::Instance(JvmObject { class_name, fields }));
        (state.heap.len() - 1) as u32
    }

    pub fn resolve_utf8(index: u16, pool: &[ConstantInfo]) -> String {
        match &pool[index as usize - 1] {
            ConstantInfo::Utf8(utf8_data) => utf8_data.utf8_string.clone(),
            _ => panic!("Expected Utf8 at index {}, but found something else", index),
        }
    }

    pub fn get_super_class_name(class: &classfile_parser::ClassFile) -> Option<String> {
        // if super_class index is 0, it means this is java/lang/Object
        if class.super_class == 0 {
            return None;
        }

        let pool = &class.const_pool;

        match &pool[class.super_class as usize - 1] {
            ConstantInfo::Class(class_info) => Some(JVM::resolve_utf8(class_info.name_index, pool)),
            _ => panic!("super_class index did not point to a Class info entry"),
        }
    }

    pub fn handle_str_buffer_fns(
        object_ref: &mut HeapObject,
        method: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        // 0th contains the objectref in args

        let heap_obj = if let HeapObject::Instance(obj) = object_ref {
            obj
        } else {
            println!(
                "Expected instance for StringBuffer, but got something else: {:?} | method = {} | args = {:?}",
                object_ref, method, args
            );

            return Err("Expected instance for StringBuffer object".into());
        };

        match method {
            "append" => {
                let buffer: &mut String = {
                    if let JvmStackValue::String(s) = heap_obj.fields.get_mut("buffer").unwrap() {
                        s
                    } else {
                        return Err("StringBuffer instance missing 'buffer' field".into());
                    }
                };

                let val = args[1].clone();
                let append_str = match val {
                    JvmStackValue::String(s) => s,
                    JvmStackValue::Int(i) => i.to_string(),
                    JvmStackValue::Float(f) => f.to_string(),
                    JvmStackValue::Long(l) => l.to_string(),
                    JvmStackValue::Double(d) => d.to_string(),
                    _ => format!("{:?}", val),
                };
                buffer.push_str(&append_str);
                Ok(Some(args[0].clone()))
            }
            "toString" => {
                let buffer: &mut String = {
                    if let JvmStackValue::String(s) = heap_obj.fields.get_mut("buffer").unwrap() {
                        s
                    } else {
                        return Err("StringBuffer instance missing 'buffer' field".into());
                    }
                };

                Ok(Some(JvmStackValue::String(buffer.clone())))
            }
            "<init>" => {
                // Initialize the 'buffer' field to an empty string
                heap_obj
                    .fields
                    .insert("buffer".to_string(), JvmStackValue::String(String::new()));
                Ok(None)
            }
            _ => {
                println!(
                    "[-] Unknown StringBuffer method: {} | args = {:?}",
                    method, args
                );
                panic!();
                Ok(None)
            }
        }
    }

    fn handle_string_static_fns(
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("valueOf", "(I)Ljava/lang/String;") => match args.first() {
                Some(JvmStackValue::Int(value)) => {
                    Ok(Some(JvmStackValue::String(value.to_string())))
                }
                value => Err(format!("String.valueOf(I): invalid arg {:?}", value)),
            },
            ("valueOf", "(J)Ljava/lang/String;") => match args.first() {
                Some(JvmStackValue::Long(value)) => {
                    Ok(Some(JvmStackValue::String(value.to_string())))
                }
                value => Err(format!("String.valueOf(J): invalid arg {:?}", value)),
            },
            ("valueOf", "(Z)Ljava/lang/String;") => match args.first() {
                Some(JvmStackValue::Int(value)) => Ok(Some(JvmStackValue::String(
                    if *value == 0 { "false" } else { "true" }.to_string(),
                ))),
                value => Err(format!("String.valueOf(Z): invalid arg {:?}", value)),
            },
            ("valueOf", "(Ljava/lang/Object;)Ljava/lang/String;") => match args.first() {
                Some(JvmStackValue::String(value)) => {
                    Ok(Some(JvmStackValue::String(value.clone())))
                }
                Some(JvmStackValue::Null) => Ok(Some(JvmStackValue::String("null".to_string()))),
                Some(value) => Ok(Some(JvmStackValue::String(format!("{:?}", value)))),
                None => Err("String.valueOf(Object): missing arg".into()),
            },
            _ => Err(format!(
                "Unsupported String static method: {}{}",
                method, descriptor
            )),
        }
    }

    fn handle_string_fns(
        objectref: JvmStackValue,
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        let string = match objectref {
            JvmStackValue::String(value) => value,
            JvmStackValue::Null => return Err("String: NullPointerException".into()),
            value => return Err(format!("String: expected string object, found {:?}", value)),
        };

        match (method, descriptor) {
            ("concat", "(Ljava/lang/String;)Ljava/lang/String;") => {
                let Some(JvmStackValue::String(suffix)) = args.first() else {
                    return Err(format!("String.concat: invalid arg {:?}", args.first()));
                };

                Ok(Some(JvmStackValue::String(format!("{}{}", string, suffix))))
            }
            ("length", "()I") => Ok(Some(JvmStackValue::Int(string.chars().count() as i32))),
            ("charAt", "(I)C") => {
                let Some(JvmStackValue::Int(index)) = args.first() else {
                    return Err(format!("String.charAt: invalid arg {:?}", args.first()));
                };

                let ch = string
                    .chars()
                    .nth(*index as usize)
                    .ok_or_else(|| format!("StringIndexOutOfBoundsException: {}", index))?;
                Ok(Some(JvmStackValue::Int(ch as i32)))
            }
            ("indexOf", "(I)I") => {
                let Some(JvmStackValue::Int(ch)) = args.first() else {
                    return Err(format!("String.indexOf(I): invalid arg {:?}", args.first()));
                };

                Ok(Some(JvmStackValue::Int(
                    string
                        .find(char::from_u32(*ch as u32).unwrap_or('\0'))
                        .map_or(-1, |idx| string[..idx].chars().count() as i32),
                )))
            }
            ("indexOf", "(II)I") => {
                let (Some(JvmStackValue::Int(ch)), Some(JvmStackValue::Int(from_index))) =
                    (args.first(), args.get(1))
                else {
                    return Err(format!("String.indexOf(II): invalid args {:?}", args));
                };

                let start_byte = string
                    .char_indices()
                    .nth((*from_index).max(0) as usize)
                    .map_or(string.len(), |(idx, _)| idx);
                let needle = char::from_u32(*ch as u32).unwrap_or('\0');

                Ok(Some(JvmStackValue::Int(
                    string[start_byte..]
                        .find(needle)
                        .map_or(-1, |idx| string[..start_byte + idx].chars().count() as i32),
                )))
            }
            ("lastIndexOf", "(II)I") => {
                let (Some(JvmStackValue::Int(ch)), Some(JvmStackValue::Int(from_index))) =
                    (args.first(), args.get(1))
                else {
                    return Err(format!("String.lastIndexOf(II): invalid args {:?}", args));
                };

                let needle = char::from_u32(*ch as u32).unwrap_or('\0');
                let max_chars = ((*from_index).max(0) as usize + 1).min(string.chars().count());
                let prefix: String = string.chars().take(max_chars).collect();

                Ok(Some(JvmStackValue::Int(
                    prefix
                        .rfind(needle)
                        .map_or(-1, |idx| prefix[..idx].chars().count() as i32),
                )))
            }
            ("substring", "(I)Ljava/lang/String;") => {
                let Some(JvmStackValue::Int(begin)) = args.first() else {
                    return Err(format!(
                        "String.substring(I): invalid arg {:?}",
                        args.first()
                    ));
                };

                Ok(Some(JvmStackValue::String(
                    string.chars().skip(*begin as usize).collect(),
                )))
            }
            ("substring", "(II)Ljava/lang/String;") => {
                let (Some(JvmStackValue::Int(begin)), Some(JvmStackValue::Int(end))) =
                    (args.first(), args.get(1))
                else {
                    return Err(format!("String.substring(II): invalid args {:?}", args));
                };

                Ok(Some(JvmStackValue::String(
                    string
                        .chars()
                        .skip(*begin as usize)
                        .take((end - begin).max(0) as usize)
                        .collect(),
                )))
            }
            ("trim", "()Ljava/lang/String;") => {
                Ok(Some(JvmStackValue::String(string.trim().to_string())))
            }
            ("endsWith", "(Ljava/lang/String;)Z") => {
                let Some(JvmStackValue::String(suffix)) = args.first() else {
                    return Err(format!("String.endsWith: invalid arg {:?}", args.first()));
                };

                Ok(Some(JvmStackValue::Int(if string.ends_with(suffix) {
                    1
                } else {
                    0
                })))
            }
            _ => Err(format!(
                "Unsupported String instance method: {}{}",
                method, descriptor
            )),
        }
    }

    fn execute_static_method(
        class_name: &str,
        method_name: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
        stack: &mut Vec<JvmStackValue>, // We need this to push the return value!
    ) -> Result<(), String> {
        if class_name.starts_with("javax/microedition") {
            if class_name == "javax/microedition/lcdui/Image" {
                let return_value = image::handle_static_method(method_name, descriptor, args, jvm)?;

                if let Some(val) = return_value {
                    stack.push(val);
                }

                return Ok(());
            }
            if class_name == "javax/microedition/lcdui/Display" {
                let return_value =
                    display::handle_static_method(method_name, descriptor, args, jvm)?;

                if let Some(val) = return_value {
                    stack.push(val);
                }

                return Ok(());
            }
            panic!(
                "[-] ExeStaticMethod - Skipping native static method call to {}.{}{} | args = {:?}",
                class_name, method_name, descriptor, args
            );
        } else if class_name == "java/lang/String" {
            let return_value = JVM::handle_string_static_fns(method_name, descriptor, args)?;

            if let Some(val) = return_value {
                stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Thread" && method_name == "sleep" {
            if let Some(JvmStackValue::Long(ms)) = args.first() {
                std::thread::sleep(std::time::Duration::from_millis(*ms as u64));
            }
            return Ok(());
        } else if class_name == "java/lang/System" && method_name == "currentTimeMillis" {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            stack.push(JvmStackValue::Long(millis));
            return Ok(());
        } else if class_name == "java/lang/Math" {
            let res = JVM::handle_math_fns(method_name, descriptor, args);
            if let Err(e) = res {
                return Err(format!("Error handling Math method: {}", e).into());
            }
            if let Some(val) = res.unwrap() {
                stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Runtime" {
            let res = JVM::handle_runtime_fns(method_name, descriptor, args);

            if let Err(e) = &res {
                return Err(format!("Error handling Runtime method: {}", e).into());
            }

            if let Some(val) = res.unwrap() {
                stack.push(val);
            }

            return Ok(());
        }

        let class_data = {
            let state = jvm.state.lock();
            state
                .classes
                .get(class_name)
                .ok_or_else(|| format!("ClassDef not found in VM: {}", class_name))?
                .clone()
        };

        let mut current_class_data = class_data.clone();
        let mut method_opt =
            JVM::find_method_in_class(&current_class_data, method_name, descriptor).cloned();

        while method_opt.is_none() {
            let super_name = JVM::get_super_class_name(&current_class_data);
            if let Some(s_name) = super_name {
                let state = jvm.state.lock();
                if let Some(s_data) = state.classes.get(&s_name) {
                    current_class_data = s_data.clone();
                    method_opt =
                        JVM::find_method_in_class(&current_class_data, method_name, descriptor)
                            .cloned();
                    continue;
                }
            }
            break;
        }

        let method = method_opt.ok_or_else(|| {
            format!(
                "Method not found: {}.{}{}",
                class_name, method_name, descriptor
            )
        })?;

        let code_attr = JVM::get_code_attribute(&method, &current_class_data.const_pool)
            .ok_or_else(|| {
                "Static method has no Code attribute (is it abstract or native?)".to_string()
            })?;

        let mut locals = vec![JvmStackValue::Null; code_attr.max_locals as usize];

        let mut local_idx = 0;
        for arg in args {
            if local_idx < locals.len() {
                locals[local_idx] = arg.clone();
            }

            match arg {
                JvmStackValue::Long(_) | JvmStackValue::Double(_) => {
                    local_idx += 2;
                }
                _ => {
                    local_idx += 1;
                }
            }
        }

        let return_value = JVM::run_frame(
            &code_attr.code,
            &current_class_data.const_pool,
            &mut locals,
            jvm,
        )?;

        if let Some(val) = return_value {
            stack.push(val);
        }

        Ok(())
    }

    pub fn paint(&self) -> Result<(), String> {
        static IS_PAINTING: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if IS_PAINTING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        let res = game_canvas::paint(&self);
        IS_PAINTING.store(false, std::sync::atomic::Ordering::SeqCst);
        res
    }

    fn handle_math_fns(
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("abs", "(I)I") => {
                if let Some(JvmStackValue::Int(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Int(v.abs())))
                } else {
                    Err("Math.abs(I)I: missing arg".into())
                }
            }
            ("abs", "(J)J") => {
                if let Some(JvmStackValue::Long(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Long(v.abs())))
                } else {
                    Err("Math.abs(J)J: missing arg".into())
                }
            }
            ("abs", "(F)F") => {
                if let Some(JvmStackValue::Float(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Float(v.abs())))
                } else {
                    Err("Math.abs(F)F: missing arg".into())
                }
            }
            ("abs", "(D)D") => {
                if let Some(JvmStackValue::Double(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Double(v.abs())))
                } else {
                    Err("Math.abs(D)D: missing arg".into())
                }
            }
            ("min", "(II)I") => {
                if let (Some(JvmStackValue::Int(v1)), Some(JvmStackValue::Int(v2))) =
                    (args.get(0), args.get(1))
                {
                    Ok(Some(JvmStackValue::Int((*v1).min(*v2))))
                } else {
                    Err("Math.min(II)I: missing arg".into())
                }
            }
            ("min", "(JJ)J") => {
                if let (Some(JvmStackValue::Long(v1)), Some(JvmStackValue::Long(v2))) =
                    (args.get(0), args.get(1))
                {
                    Ok(Some(JvmStackValue::Long((*v1).min(*v2))))
                } else {
                    Err("Math.min(JJ)J: missing arg".into())
                }
            }
            ("max", "(II)I") => {
                if let (Some(JvmStackValue::Int(v1)), Some(JvmStackValue::Int(v2))) =
                    (args.get(0), args.get(1))
                {
                    Ok(Some(JvmStackValue::Int((*v1).max(*v2))))
                } else {
                    Err("Math.max(II)I: missing arg".into())
                }
            }
            ("max", "(JJ)J") => {
                if let (Some(JvmStackValue::Long(v1)), Some(JvmStackValue::Long(v2))) =
                    (args.get(0), args.get(1))
                {
                    Ok(Some(JvmStackValue::Long((*v1).max(*v2))))
                } else {
                    Err("Math.max(JJ)J: missing arg".into())
                }
            }
            ("sqrt", "(D)D") => {
                if let Some(JvmStackValue::Double(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Double(v.sqrt())))
                } else {
                    Err("Math.sqrt(D)D: missing arg".into())
                }
            }
            ("sin", "(D)D") => {
                if let Some(JvmStackValue::Double(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Double(v.sin())))
                } else {
                    Err("Math.sin(D)D: missing arg".into())
                }
            }
            ("cos", "(D)D") => {
                if let Some(JvmStackValue::Double(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Double(v.cos())))
                } else {
                    Err("Math.cos(D)D: missing arg".into())
                }
            }
            ("tan", "(D)D") => {
                if let Some(JvmStackValue::Double(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Double(v.tan())))
                } else {
                    Err("Math.tan(D)D: missing arg".into())
                }
            }
            ("ceil", "(D)D") => {
                if let Some(JvmStackValue::Double(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Double(v.ceil())))
                } else {
                    Err("Math.ceil(D)D: missing arg".into())
                }
            }
            ("floor", "(D)D") => {
                if let Some(JvmStackValue::Double(v)) = args.get(0) {
                    Ok(Some(JvmStackValue::Double(v.floor())))
                } else {
                    Err("Math.floor(D)D: missing arg".into())
                }
            }
            _ => Err(format!("Unsupported Math method: {}{}", method, descriptor)),
        }
    }

    fn handle_runtime_fns(
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("getRuntime", "()Ljava/lang/Runtime;") => {
                // We can return any dummy objectref here, since we handle all Runtime methods natively
                Ok(Some(JvmStackValue::ObjectRef(0)))
            }
            ("freeMemory", "()J") => {
                // Return a dummy value, since we don't actually track memory usage
                Ok(Some(JvmStackValue::Long(1024 * 1024 * 100))) // 100 MB free
            }
            ("totalMemory", "()J") => {
                // Return a dummy value, since we don't actually track memory usage
                Ok(Some(JvmStackValue::Long(1024 * 1024 * 200))) // 200 MB total
            }
            ("maxMemory", "()J") => {
                // Return a dummy value, since we don't actually track memory usage
                Ok(Some(JvmStackValue::Long(1024 * 1024 * 500))) // 500 MB max
            }
            _ => Err(format!(
                "Unsupported Runtime method: {}{}",
                method, descriptor
            )),
        }
    }

    fn handle_class_fns(
        method_name: &str,
        descriptor: &str,
        _args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method_name, descriptor) {
            ("getResourceAsStream", "(Ljava/lang/String;)Ljava/io/InputStream;") => {
                let name = match _args.get(0) {
                    Some(JvmStackValue::String(s)) => s,
                    _ => return Err("Class.getResourceAsStream: expected String argument".into()),
                };

                let resource_path = if name.starts_with('/') {
                    name[1..].to_string()
                } else {
                    name.clone()
                };

                let mut state = jvm.state.lock();
                let _data = if let Some(_data) = state.resources.get(&resource_path) {
                } else {
                    return Err("Resource not found".into()); // Resource not found, return null
                };

                let mut fields = HashMap::new();

                fields.insert(
                    "jvm_res".to_string(),
                    JvmStackValue::String(resource_path.clone()),
                );

                let stream_ref = JVM::allocate_internal(
                    &mut state,
                    "java/io/ByteArrayInputStream".to_string(),
                    fields,
                );

                Ok(Some(JvmStackValue::ObjectRef(stream_ref)))
            }
            _ => Err(format!(
                "Unsupported Class method: {}{}",
                method_name, descriptor
            )),
        }
    }

    fn handle_byte_array_input_stream_fns(
        method_name: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        let mut state = jvm.state.lock();

        let object_ref =
            match args.get(0) {
                Some(JvmStackValue::ObjectRef(r)) => state
                    .heap
                    .get(*r as usize)
                    .ok_or_else(|| "Invalid object reference".to_string())?,
                _ => return Err(
                    "Expected object reference as first argument to ByteArrayInputStream method"
                        .into(),
                ),
            };

        let resource_path = if let HeapObject::Instance(obj) = object_ref {
            if let Some(JvmStackValue::String(path)) = obj.fields.get("jvm_res") {
                path.clone()
            } else {
                return Err("ByteArrayInputStream instance missing 'jvm_res' field".into());
            }
        } else {
            return Err("Expected instance for ByteArrayInputStream object".into());
        };

        let data = state
            .resources
            .get(&resource_path)
            .cloned()
            .ok_or_else(|| "Resource not found for ByteArrayInputStream".to_string())?;

        match (method_name, descriptor) {
            ("available", "()I") => Ok(Some(JvmStackValue::Int(data.len() as i32))),
            ("read", "([B)I") => {
                let buffer_ref = match args.get(1) {
                    Some(JvmStackValue::ObjectRef(r)) => *r as usize,
                    Some(value) => {
                        return Err(format!(
                            "Expected byte array reference as second argument to read(), found {:?}",
                            value
                        ));
                    }
                    None => return Err("read([B)I: missing byte array argument".into()),
                };

                let copied = match state.heap.get_mut(buffer_ref) {
                    Some(HeapObject::Array { data: buffer, .. }) => {
                        let copy_len = data.len().min(buffer.len());

                        for (slot, byte) in buffer.iter_mut().zip(data.iter()).take(copy_len) {
                            *slot = JvmStackValue::Int(*byte as i32);
                        }

                        copy_len
                    }
                    Some(_) => return Err("read([B)I: expected array buffer".into()),
                    None => return Err("read([B)I: invalid byte array reference".into()),
                };

                Ok(Some(JvmStackValue::Int(copied as i32)))
            }
            _ => Err(format!(
                "Unsupported ByteArrayInputStream method: {}{}",
                method_name, descriptor
            )),
        }
    }
}
