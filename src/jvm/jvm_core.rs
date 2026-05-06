use std::collections::HashMap;
use std::panic;
use std::sync::{Arc, Mutex};

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
            println!($($arg)*);
        }
    };
}

#[derive(Debug, Clone)]
pub enum JvmStackValue {
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
            let mut state = self.state.lock().unwrap();
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
            let state = self.state.lock().unwrap();
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
        println!("[JVM] Added class: {}", class_name);
        self.state.lock().unwrap().classes.insert(class_name, class);
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

                                let mut locals: Vec<JvmStackValue> = vec![];
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

                                    locals.push(objectref);
                                }

                                return JVM::run_frame(&code_attr.code, pool, &mut locals, self);
                            }
                        }
                    }
                }
                println!("Method name: {}", name_info.utf8_string);
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

        while pc < bytecode.len() {
            let opcode = bytecode[pc];

            // debug-out added this line for easy finding this line ;)
            jvm_debug!(
                "PC: {}, Opcode: {:02X}, Stack: {:?}, Locals: {:?}",
                pc,
                opcode,
                stack,
                locals
            );

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
                0x1a => {
                    // iload_0
                    let local_val = locals[0].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x1b => {
                    // iload_1
                    let local_val = locals[1].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x1c => {
                    // iload_2
                    let local_val = locals[2].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x1d => {
                    // iload_3
                    let local_val = locals[3].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x1e => {
                    // lload_0
                    let local_val = locals[0].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x1f => {
                    // lload_1
                    let local_val = locals[1].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x20 => {
                    // lload_2
                    let local_val = locals[2].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x21 => {
                    // lload_3
                    let local_val = locals[3].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x27 => {
                    // dload_1
                    let local_val = locals[1].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x28 => {
                    // dload_2
                    let local_val = locals[2].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x29 => {
                    // dload_3
                    let local_val = locals[3].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x2a => {
                    // aload_0
                    let local_val = locals[0].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x2b => {
                    // aload_1
                    let local_val = locals[1].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x2c => {
                    // aload_2
                    let local_val = locals[2].clone();
                    stack.push(local_val);
                    pc += 1;
                }
                0x2d => {
                    // aload_3
                    let local_val = locals[3].clone();
                    stack.push(local_val);
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
                        let state = jvm.state.lock().unwrap();
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
                        let state = jvm.state.lock().unwrap();
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
                0x3c => {
                    // istore_1
                    let val = stack.pop().ok_or("istore_1: Stack underflow")?;
                    if locals.len() < 2 {
                        locals.resize(2, JvmStackValue::Null);
                    }
                    locals[1] = val;
                    pc += 1;
                }
                0x4b => {
                    // astore_9
                    let val = stack.pop().ok_or("astore_1: Stack underflow")?;
                    if locals.len() < 1 {
                        locals.resize(1, JvmStackValue::Null);
                    }
                    locals[0] = val;
                    pc += 1;
                }
                0x4c => {
                    // astore_1
                    let val = stack.pop().ok_or("astore_0: Stack underflow")?;
                    if locals.len() < 2 {
                        locals.resize(2, JvmStackValue::Null);
                    }
                    locals[1] = val;
                    pc += 1;
                }
                0x4d => {
                    // astore_2
                    let val = stack.pop().ok_or("astore_2: Stack underflow")?;
                    if locals.len() < 3 {
                        locals.resize(3, JvmStackValue::Null);
                    }
                    locals[2] = val;
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

                    match jvm.state.lock().unwrap().heap.get_mut(heap_idx) {
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

                    match jvm.state.lock().unwrap().heap.get_mut(heap_idx) {
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

                    let val = {
                        let state = jvm.state.lock().unwrap();
                        state
                            .static_fields
                            .get(&key)
                            .cloned()
                            .ok_or_else(|| format!("Static field not found: {}", key))
                    };

                    if let Err(e) = &val {
                        println!("Error: {}", e);
                        let state = jvm.state.lock().unwrap();
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
                        let state = jvm.state.lock().unwrap();
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
                        let mut state = jvm.state.lock().unwrap();
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
                                let mut state = jvm.state.lock().unwrap();
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
                            let state = jvm.state.lock().unwrap();
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
                                "invokestatic: expected MethodRef at index {}",
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
                    if class_name == "java/lang/System" && method_name == "currentTimeMillis" {
                        panic!(
                            "System.currentTimeMillis is not supported in this JVM implementation"
                        );
                    } else {
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
                        let mut state = jvm.state.lock().unwrap();
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
                        let mut state = jvm.state.lock().unwrap();
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
                        let mut state = jvm.state.lock().unwrap();
                        state.heap.push(array_obj);
                        (state.heap.len() - 1) as u32
                    };

                    stack.push(JvmStackValue::ObjectRef(array_ref));

                    pc += 3;
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
                        let state = jvm.state.lock().unwrap();
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
                0x3D => {
                    // istore_2
                    let val = stack.pop().ok_or("istore_2: Stack underflow")?;
                    if locals.len() < 3 {
                        locals.resize(3, JvmStackValue::Null);
                    }
                    locals[2] = val;
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
                0x4E => {
                    // astore_3
                    let val = stack.pop().ok_or("astore_3: Stack underflow")?;
                    if locals.len() < 4 {
                        locals.resize(4, JvmStackValue::Null);
                    }
                    locals[3] = val;
                    pc += 1;
                }
                0x15 => {
                    // iload
                    let index = bytecode[pc + 1] as usize;
                    if index >= locals.len() {
                        return Err(format!("iload: Invalid local index {}", index).into());
                    }
                    stack.push(locals[index].clone());
                    pc += 2;
                }
                0xAC | 0xAF => {
                    // ireturn
                    let val = stack.pop().ok_or("return: Stack underflow")?;

                    jvm_debug!("Execution finished with return value: {:?}", val);
                    return Ok(Some(val));
                }
                0x37 => {
                    // lstore_1
                    let val = stack.pop().ok_or("lstore_1: stack underflow")?;
                    if locals.len() < 2 {
                        locals.resize(2, JvmStackValue::Null);
                    }
                    locals[1] = val;
                    pc += 1;
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
                let val = args[1].clone(); // args[0] is 'this'
                vector.push(val);
                Ok(None)
            }
            "size" => Ok(Some(JvmStackValue::Int(vector.len() as i32))),
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
                Ok(None)
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
            if class_name == game_canvas::CLASS_NAME {
                let return_value = {
                    let mut state = jvm.state.lock().unwrap();
                    let obj_ref = if let JvmStackValue::ObjectRef(id) = objectref {
                        state.heap.get_mut(id as usize).ok_or_else(|| {
                            format!(
                                "GameCanvas method call with invalid object reference: {}",
                                id
                            )
                        })?
                    } else {
                        return Err("GameCanvas method call with non-reference object".into());
                    };
                    let instance = if let HeapObject::Instance(inst) = obj_ref {
                        inst
                    } else {
                        return Err("GameCanvas method call on non-instance object".into());
                    };
                    game_canvas::handle_virtual_method(instance, method_name, descriptor, args)
                };

                if let Err(e) = &return_value {
                    return Err(format!("Error handling GameCanvas method: {}", e).into());
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == graphics::CLASS_NAME {
                let return_value =
                    graphics::handle_virtual_method(&objectref, method_name, descriptor, args);

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
            return Ok(());
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
                let mut state = jvm.state.lock().unwrap();
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
                let mut state = jvm.state.lock().unwrap();
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
        }

        let Some((_resolved_class_name, const_pool, code_attr)) =
            JVM::find_method_code_in_hierarchy(jvm, class_name, method_name, descriptor)?
        else {
            if JVM::class_extends(jvm, class_name, game_canvas::CLASS_NAME) {
                let return_value = {
                    let mut state = jvm.state.lock().unwrap();
                    let obj_ref = if let JvmStackValue::ObjectRef(id) = objectref {
                        state.heap.get_mut(id as usize).ok_or_else(|| {
                            format!(
                                "GameCanvas method call with invalid object reference: {}",
                                id
                            )
                        })?
                    } else {
                        return Err("GameCanvas method call with non-reference object".into());
                    };
                    let instance = if let HeapObject::Instance(inst) = obj_ref {
                        inst
                    } else {
                        return Err("GameCanvas method call on non-instance object".into());
                    };
                    game_canvas::handle_virtual_method(instance, method_name, descriptor, args)?
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
                    jvm.thread_handles.lock().unwrap().insert(obj_id, handle);

                    return Ok(());
                } else if is_thread_join {
                    let obj_id = match &objectref {
                        JvmStackValue::ObjectRef(id) => *id,
                        _ => return Err("Thread.join: not an object ref".into()),
                    };
                    if let Some(handle) = jvm.thread_handles.lock().unwrap().remove(&obj_id) {
                        handle
                            .join()
                            .map_err(|_| "Thread.join: thread panicked".to_string())?;
                    }
                    return Ok(());
                } else if is_thread_is_alive {
                    let obj_id = match &objectref {
                        JvmStackValue::ObjectRef(id) => *id,
                        _ => return Err("Thread.isAlive: not an object ref".into()),
                    };
                    let is_alive = {
                        let handles = jvm.thread_handles.lock().unwrap();
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

        locals[0] = objectref.clone();

        let mut local_idx = 1;
        for arg in args {
            locals[local_idx] = arg.clone();

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
        let state = jvm.state.lock().unwrap();

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
        let state = jvm.state.lock().unwrap();

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
        if class_name == "java/util/Vector" {
            let mut obj = JvmObject {
                class_name: class_name.clone(),
                fields: HashMap::new(),
            };

            obj.fields
                .insert("container".to_string(), JvmStackValue::Vector(Vec::new()));

            let mut state = self.state.lock().unwrap();
            state.heap.push(HeapObject::Instance(obj));

            return (state.heap.len() - 1) as u32;
        }

        let mut fields = HashMap::new();

        // Walk up the inheritance tree to find all fields this object should have
        let mut current_class = Some(class_name.clone());
        let state = self.state.lock().unwrap();
        while let Some(name) = current_class {
            if let Some(class_data) = state.classes.get(&name) {
                for field_info in &class_data.fields {
                    let descriptor =
                        JVM::resolve_utf8(field_info.descriptor_index, &class_data.const_pool);
                    let default_val = if descriptor.starts_with('L') || descriptor.starts_with('[')
                    {
                        JvmStackValue::Null
                    } else {
                        JvmStackValue::Int(0)
                    };
                    let f_name = JVM::resolve_utf8(field_info.name_index, &class_data.const_pool);
                    let key = format!("{}:{}", f_name, descriptor);
                    fields.insert(key, default_val);
                }
                current_class = JVM::get_super_class_name(class_data);
            } else {
                current_class = None;
            }
        }
        drop(state); // Drop the lock before we mutate the heap

        let obj = HeapObject::Instance(JvmObject {
            class_name,
            fields: fields,
        });

        let mut state = self.state.lock().unwrap();
        state.heap.push(obj);
        (state.heap.len() - 1) as u32 // The objectref
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
            let state = jvm.state.lock().unwrap();
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
                let state = jvm.state.lock().unwrap();
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

        for (i, arg) in args.iter().enumerate() {
            locals[i] = arg.clone();
        }

        let return_value = JVM::run_frame(
            &code_attr.code,
            &class_data.const_pool.clone(),
            &mut locals,
            jvm,
        )?;

        if let Some(val) = return_value {
            stack.push(val);
        }

        Ok(())
    }

    pub fn paint(&self) -> Result<(), String> {
        let disp = display::get_display(self);

        let displayable_res = display::get_displayable_obj(disp, self)?;
        let displayable_ref =
            displayable_res.ok_or_else(|| "No displayable object set".to_string())?;

        let class_name = if let JvmStackValue::ObjectRef(id) = displayable_ref {
            let state = self.state.lock().unwrap();
            if let HeapObject::Instance(inst) = state
                .heap
                .get(id as usize)
                .ok_or("Invalid heap reference")?
            {
                inst.class_name.clone()
            } else {
                return Err("Displayable is not an instance".into());
            }
        } else {
            return Err("Displayable is not an object ref".into());
        };

        let graphics_handle = self.allocate(graphics::CLASS_NAME.to_string());

        return JVM::execute_method(
            displayable_ref,
            &class_name,
            "paint",
            "(Ljavax/microedition/lcdui/Graphics;)V",
            &[JvmStackValue::ObjectRef(graphics_handle)],
            self,
            &mut Vec::new(),
        );
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
}
