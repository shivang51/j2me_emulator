use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::panic;
use std::sync::{Arc, Condvar, Mutex as StdMutex};
use std::time::{Duration, Instant};

use classfile_parser::constant_info::{ConstantInfo, FieldRefConstant, MethodRefConstant};

use crate::{
    jvm::javax::{
        lcdui::{display, game::game_canvas, graphics, image},
        media::player,
        midlet, rms,
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

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, Copy)]
struct CalendarParts {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
    day_of_year: u32,
    day_of_week: u32,
}

pub type SharedJvmState = Arc<Mutex<JvmState>>;

pub struct JVM {
    pub loaded_jar: Option<JarFileData>,
    pub state: SharedJvmState,
    pub thread_handles: Arc<Mutex<HashMap<u32, std::thread::JoinHandle<()>>>>,
    pause_control: Arc<PauseControl>,
}

#[derive(Debug)]
struct PauseControl {
    state: StdMutex<PauseState>,
    wake: Condvar,
}

#[derive(Debug)]
struct PauseState {
    paused: bool,
    stopped: bool,
    pause_started: Option<Instant>,
    total_paused: Duration,
}

impl Clone for JVM {
    fn clone(&self) -> Self {
        JVM {
            loaded_jar: self.loaded_jar.clone(),
            state: Arc::clone(&self.state),
            thread_handles: Arc::clone(&self.thread_handles),
            pause_control: Arc::clone(&self.pause_control),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExceptionHandler {
    pub start_pc: u16,
    pub end_pc: u16,
    pub handler_pc: u16,
    pub catch_type: u16,
}

#[derive(Debug)]
pub struct Code {
    #[allow(dead_code)]
    pub max_stack: u16,
    pub max_locals: u16,
    pub code: Vec<u8>,
    pub exception_table: Vec<ExceptionHandler>,
}

impl JVM {
    pub fn is_reference_value(val: &JvmStackValue) -> bool {
        matches!(
            val,
            JvmStackValue::ObjectRef(_)
                | JvmStackValue::String(_)
                | JvmStackValue::Vector(_)
                | JvmStackValue::Null
        )
    }

    pub fn reference_values_equal(val1: &JvmStackValue, val2: &JvmStackValue) -> bool {
        val1 == val2
    }

    fn method_label(class_name: &str, method_name: &str, descriptor: &str) -> String {
        format!("{}.{}{}", class_name, method_name, descriptor)
    }

    fn append_method_context(
        error: String,
        class_name: &str,
        method_name: &str,
        descriptor: &str,
    ) -> String {
        format!(
            "{} | in {}",
            error,
            JVM::method_label(class_name, method_name, descriptor)
        )
    }

    fn append_static_method_context(
        error: String,
        class_name: &str,
        method_name: &str,
        descriptor: &str,
    ) -> String {
        format!(
            "{} | in static {}",
            error,
            JVM::method_label(class_name, method_name, descriptor)
        )
    }

    fn append_invoke_context(
        error: String,
        invoke_kind: &str,
        class_name: &str,
        method_name: &str,
        descriptor: &str,
        pc: usize,
    ) -> String {
        format!(
            "{} | while {} {} at pc {}",
            error,
            invoke_kind,
            JVM::method_label(class_name, method_name, descriptor),
            pc
        )
    }

    fn exception_handlers_from_code_attribute(
        code_attr: &classfile_parser::attribute_info::CodeAttribute,
    ) -> Vec<ExceptionHandler> {
        code_attr
            .exception_table
            .iter()
            .map(|entry| ExceptionHandler {
                start_pc: entry.start_pc,
                end_pc: entry.end_pc,
                handler_pc: entry.handler_pc,
                catch_type: entry.catch_type,
            })
            .collect()
    }

    fn normalize_class_name(class_name: &str) -> String {
        class_name.replace('.', "/")
    }

    fn java_exception_class_from_error(error: &str) -> Option<String> {
        const KNOWN_EXCEPTIONS: &[(&str, &str)] = &[
            ("NullPointerException", "java/lang/NullPointerException"),
            (
                "ArrayIndexOutOfBoundsException",
                "java/lang/ArrayIndexOutOfBoundsException",
            ),
            (
                "StringIndexOutOfBoundsException",
                "java/lang/StringIndexOutOfBoundsException",
            ),
            (
                "IndexOutOfBoundsException",
                "java/lang/IndexOutOfBoundsException",
            ),
            ("ArithmeticException", "java/lang/ArithmeticException"),
            (
                "NegativeArraySizeException",
                "java/lang/NegativeArraySizeException",
            ),
            ("ClassCastException", "java/lang/ClassCastException"),
            (
                "IllegalArgumentException",
                "java/lang/IllegalArgumentException",
            ),
            ("IllegalStateException", "java/lang/IllegalStateException"),
            ("NumberFormatException", "java/lang/NumberFormatException"),
            (
                "UnsupportedOperationException",
                "java/lang/UnsupportedOperationException",
            ),
            ("SecurityException", "java/lang/SecurityException"),
            ("RuntimeException", "java/lang/RuntimeException"),
            ("IOException", "java/io/IOException"),
            ("InterruptedException", "java/lang/InterruptedException"),
            ("OutOfMemoryError", "java/lang/OutOfMemoryError"),
            ("NoClassDefFoundError", "java/lang/NoClassDefFoundError"),
            (
                "RecordStoreNotFoundException",
                "javax/microedition/rms/RecordStoreNotFoundException",
            ),
            (
                "RecordStoreNotOpenException",
                "javax/microedition/rms/RecordStoreNotOpenException",
            ),
            (
                "RecordStoreFullException",
                "javax/microedition/rms/RecordStoreFullException",
            ),
            (
                "InvalidRecordIDException",
                "javax/microedition/rms/InvalidRecordIDException",
            ),
            (
                "RecordStoreException",
                "javax/microedition/rms/RecordStoreException",
            ),
            ("Exception", "java/lang/Exception"),
            ("Throwable", "java/lang/Throwable"),
        ];

        for (needle, class_name) in KNOWN_EXCEPTIONS {
            if error.contains(needle) {
                return Some((*class_name).to_string());
            }
        }

        error
            .split(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '|' | ',' | ';'))
            .map(|token| token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '[' | ']')))
            .map(JVM::normalize_class_name)
            .find(|token| {
                (token.starts_with("java/") || token.starts_with("javax/microedition/rms/"))
                    && (token.ends_with("Exception")
                        || token.ends_with("Error")
                        || token == "java/lang/Throwable")
            })
    }

    fn builtin_exception_extends(exception_class: &str, catch_class: &str) -> bool {
        if exception_class == catch_class {
            return true;
        }

        match exception_class {
            "java/lang/NullPointerException"
            | "java/lang/ArrayIndexOutOfBoundsException"
            | "java/lang/StringIndexOutOfBoundsException"
            | "java/lang/IndexOutOfBoundsException"
            | "java/lang/ArithmeticException"
            | "java/lang/NegativeArraySizeException"
            | "java/lang/ClassCastException"
            | "java/lang/IllegalArgumentException"
            | "java/lang/IllegalStateException"
            | "java/lang/NumberFormatException"
            | "java/lang/UnsupportedOperationException"
            | "java/lang/SecurityException" => matches!(
                catch_class,
                "java/lang/RuntimeException" | "java/lang/Exception" | "java/lang/Throwable"
            ),
            "java/lang/RuntimeException" => {
                matches!(catch_class, "java/lang/Exception" | "java/lang/Throwable")
            }
            "java/io/IOException" | "java/lang/InterruptedException" => {
                matches!(catch_class, "java/lang/Exception" | "java/lang/Throwable")
            }
            "javax/microedition/rms/RecordStoreNotFoundException"
            | "javax/microedition/rms/RecordStoreNotOpenException"
            | "javax/microedition/rms/RecordStoreFullException"
            | "javax/microedition/rms/InvalidRecordIDException" => matches!(
                catch_class,
                "javax/microedition/rms/RecordStoreException"
                    | "java/lang/Exception"
                    | "java/lang/Throwable"
            ),
            "javax/microedition/rms/RecordStoreException" => {
                matches!(catch_class, "java/lang/Exception" | "java/lang/Throwable")
            }
            "java/lang/Exception" => catch_class == "java/lang/Throwable",
            "java/lang/OutOfMemoryError" | "java/lang/NoClassDefFoundError" | "java/lang/Error" => {
                matches!(catch_class, "java/lang/Error" | "java/lang/Throwable")
            }
            _ => false,
        }
    }

    fn exception_matches_catch_class(jvm: &JVM, exception_class: &str, catch_class: &str) -> bool {
        if exception_class == catch_class {
            return true;
        }

        if JVM::class_extends(jvm, exception_class, catch_class) {
            return true;
        }

        JVM::builtin_exception_extends(exception_class, catch_class)
    }

    fn resolve_exception_catch_type(catch_type: u16, cp: &[ConstantInfo]) -> Option<String> {
        if catch_type == 0 {
            return Some("java/lang/Throwable".to_string());
        }

        let ConstantInfo::Class(class_info) = cp.get(catch_type as usize - 1)? else {
            return None;
        };

        Some(JVM::resolve_utf8(class_info.name_index, cp))
    }

    fn find_exception_handler(
        throw_pc: usize,
        exception_class: &str,
        exception_table: &[ExceptionHandler],
        cp: &[ConstantInfo],
        jvm: &JVM,
    ) -> Option<usize> {
        for handler in exception_table {
            if throw_pc < handler.start_pc as usize || throw_pc >= handler.end_pc as usize {
                continue;
            }

            if handler.catch_type == 0 {
                return Some(handler.handler_pc as usize);
            }

            let Some(catch_class) = JVM::resolve_exception_catch_type(handler.catch_type, cp)
            else {
                continue;
            };

            if JVM::exception_matches_catch_class(jvm, exception_class, &catch_class) {
                return Some(handler.handler_pc as usize);
            }
        }

        None
    }

    fn allocate_exception_object(jvm: &JVM, exception_class: &str, message: &str) -> JvmStackValue {
        let mut fields = HashMap::new();
        fields.insert(
            "detailMessage:Ljava/lang/String;".to_string(),
            JvmStackValue::String(message.to_string()),
        );
        fields.insert(
            "message:Ljava/lang/String;".to_string(),
            JvmStackValue::String(message.to_string()),
        );

        let mut state = jvm.state.lock();
        let object_ref = JVM::allocate_internal(&mut state, exception_class.to_string(), fields);
        JvmStackValue::ObjectRef(object_ref)
    }

    fn handle_exception_in_current_frame(
        error: String,
        throw_pc: usize,
        exception_table: &[ExceptionHandler],
        cp: &[ConstantInfo],
        jvm: &JVM,
        stack: &mut Vec<JvmStackValue>,
    ) -> Result<usize, String> {
        let Some(exception_class) = JVM::java_exception_class_from_error(&error) else {
            return Err(error);
        };

        let Some(handler_pc) =
            JVM::find_exception_handler(throw_pc, &exception_class, exception_table, cp, jvm)
        else {
            return Err(error);
        };

        jvm_debug!(
            "Caught {} thrown at pc {}, jumping to handler {}",
            exception_class,
            throw_pc,
            handler_pc
        );
        stack.clear();
        stack.push(JVM::allocate_exception_object(
            jvm,
            &exception_class,
            &error,
        ));

        Ok(handler_pc)
    }

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
        state.static_fields.insert(
            "java/lang/System.err:Ljava/io/PrintStream;".to_string(),
            JvmStackValue::ObjectRef(999),
        );
        state.static_fields.insert(
            "java/lang/System.in:Ljava/io/InputStream;".to_string(),
            JvmStackValue::Null,
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
            pause_control: Arc::new(PauseControl {
                state: StdMutex::new(PauseState {
                    paused: false,
                    stopped: false,
                    pause_started: None,
                    total_paused: Duration::ZERO,
                }),
                wake: Condvar::new(),
            }),
        }
    }

    pub fn pause(&self) {
        let mut pause_state = self.pause_control.state.lock().unwrap();
        if pause_state.paused || pause_state.stopped {
            return;
        }

        pause_state.paused = true;
        pause_state.pause_started = Some(Instant::now());
        drop(pause_state);
        player::pause_all();
    }

    pub fn resume(&self) {
        let mut pause_state = self.pause_control.state.lock().unwrap();
        if !pause_state.paused || pause_state.stopped {
            return;
        }

        if let Some(started) = pause_state.pause_started.take() {
            pause_state.total_paused += started.elapsed();
        }
        pause_state.paused = false;
        drop(pause_state);

        player::resume_all();
        self.pause_control.wake.notify_all();
    }

    pub fn set_paused(&self, paused: bool) {
        if paused {
            self.pause();
        } else {
            self.resume();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.pause_control.state.lock().unwrap().paused
    }

    pub fn shutdown(&self) {
        let mut pause_state = self.pause_control.state.lock().unwrap();
        if pause_state.stopped {
            return;
        }

        pause_state.stopped = true;
        pause_state.paused = false;
        if let Some(started) = pause_state.pause_started.take() {
            pause_state.total_paused += started.elapsed();
        }
        drop(pause_state);

        player::stop_all();
        self.thread_handles.lock().clear();
        self.pause_control.wake.notify_all();
    }

    fn is_shutdown(&self) -> bool {
        self.pause_control.state.lock().unwrap().stopped
    }

    fn wait_if_paused(&self) -> bool {
        let mut pause_state = self.pause_control.state.lock().unwrap();
        while pause_state.paused && !pause_state.stopped {
            pause_state = self.pause_control.wake.wait(pause_state).unwrap();
        }

        !pause_state.stopped
    }

    fn sleep_emulated(&self, duration: Duration) {
        let mut remaining = duration;

        while remaining > Duration::ZERO {
            if !self.wait_if_paused() {
                return;
            }

            let chunk = remaining.min(Duration::from_millis(10));
            let before = Instant::now();
            std::thread::sleep(chunk);
            remaining = remaining.saturating_sub(before.elapsed());
        }
    }

    fn current_time_millis(&self) -> i64 {
        let wall_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let paused_duration = {
            let pause_state = self.pause_control.state.lock().unwrap();
            let current_pause = pause_state
                .pause_started
                .map(|started| started.elapsed())
                .unwrap_or(Duration::ZERO);
            pause_state.total_paused + current_pause
        };

        wall_millis.saturating_sub(paused_duration.as_millis()) as i64
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
            let mut state = self.state.lock();
            if state.initialized_classes.contains(class_name) {
                return Ok(());
            }

            if let Some(class_data) = state.classes.get(class_name) {
                class_data.clone()
            } else if JVM::is_builtin_static_class(class_name) {
                state.initialized_classes.insert(class_name.to_string());
                return Ok(());
            } else {
                return Err(format!("Class not found: {}", class_name));
            }
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

    fn is_builtin_static_class(class_name: &str) -> bool {
        matches!(
            class_name,
            "java/lang/System" | "java/util/Calendar" | "java/util/TimeZone"
        )
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

                                let class_name = JVM::get_class_name(&class)?;
                                let descriptor = match &pool[method.descriptor_index as usize - 1] {
                                    ConstantInfo::Utf8(desc_info) => desc_info.utf8_string.clone(),
                                    _ => String::new(),
                                };
                                let mut locals =
                                    vec![JvmStackValue::Null; code_attr.max_locals as usize];
                                if main_name != "main" {
                                    println!(
                                        "Executing entry point method '{}' instead of 'main'",
                                        main_name
                                    );
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

                                let exception_table =
                                    JVM::exception_handlers_from_code_attribute(&code_attr);

                                return JVM::run_frame(
                                    &code_attr.code,
                                    pool,
                                    &exception_table,
                                    &mut locals,
                                    self,
                                )
                                .map_err(|e| {
                                    JVM::append_method_context(
                                        e,
                                        &class_name,
                                        &main_name,
                                        &descriptor,
                                    )
                                });
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
        exception_table: &[ExceptionHandler],
        locals: &mut Vec<JvmStackValue>,
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        let mut pc = 0;
        let mut stack: Vec<JvmStackValue> = Vec::new();
        let mut op_count = 0;

        while pc < bytecode.len() {
            if !jvm.wait_if_paused() {
                return Ok(None);
            }

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
                0x2f => {
                    // laload
                    let index = match stack.pop().ok_or("laload: Stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("laload: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("laload: Stack underflow (arrayref)")?;
                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("laload: arrayref is not a reference".into()),
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
                                    JvmStackValue::Long(value) => {
                                        stack.push(JvmStackValue::Long(*value))
                                    }
                                    value => {
                                        return Err(format!(
                                            "laload: expected Long, found {:?}",
                                            value
                                        ));
                                    }
                                }
                            }
                            _ => return Err("laload: object is not an array".into()),
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
                0x35 => {
                    // saload
                    let index = match stack.pop().ok_or("saload: Stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("saload: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("saload: Stack underflow (arrayref)")?;
                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("saload: arrayref is not a reference".into()),
                    };

                    {
                        let state = jvm.state.lock();
                        match state.heap.get(heap_idx) {
                            Some(HeapObject::Array { element_type, data }) => {
                                if element_type != "primitive_9" {
                                    return Err(format!(
                                        "saload: expected short array, found array of type {}",
                                        element_type
                                    ));
                                }

                                if index < 0 || index as usize >= data.len() {
                                    return Err(format!(
                                        "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds",
                                        index
                                    ));
                                }

                                match &data[index as usize] {
                                    JvmStackValue::Int(value) => {
                                        stack.push(JvmStackValue::Int((*value as i16) as i32))
                                    }
                                    value => {
                                        return Err(format!(
                                            "saload: expected Int, found {:?}",
                                            value
                                        ));
                                    }
                                }
                            }
                            _ => return Err("saload: object is not an array".into()),
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
                            if element_type != "primitive_4" && element_type != "primitive_8" {
                                return Err(format!(
                                    "bastore: expected byte or boolean array, found array of type {}",
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

                            let stored = if element_type == "primitive_4" {
                                value & 1
                            } else {
                                (value as i8) as i32
                            };

                            data[index as usize] = JvmStackValue::Int(stored);
                        }
                        _ => return Err("bastore: object is not an array".into()),
                    }

                    pc += 1;
                }
                0x55 => {
                    // castore
                    let value = match stack.pop().ok_or("castore: stack underflow (value)")? {
                        JvmStackValue::Int(value) => value,
                        value => return Err(format!("castore: value is not an int: {:?}", value)),
                    };

                    let index = match stack.pop().ok_or("castore: stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("castore: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("castore: stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("castore: arrayref is not a reference".into()),
                    };

                    match jvm.state.lock().heap.get_mut(heap_idx) {
                        Some(HeapObject::Array { element_type, data }) => {
                            if element_type != "primitive_5" {
                                return Err(format!(
                                    "castore: expected char array, found array of type {}",
                                    element_type
                                ));
                            }

                            if index < 0 || index as usize >= data.len() {
                                return Err(format!(
                                    "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds for length {}",
                                    index,
                                    data.len()
                                ));
                            }

                            data[index as usize] = JvmStackValue::Int(value & 0xFFFF);
                        }
                        _ => return Err("castore: object is not an array".into()),
                    }

                    pc += 1;
                }
                0x56 => {
                    // sastore
                    let value = match stack.pop().ok_or("sastore: stack underflow (value)")? {
                        JvmStackValue::Int(value) => value,
                        value => return Err(format!("sastore: value is not an int: {:?}", value)),
                    };

                    let index = match stack.pop().ok_or("sastore: stack underflow (index)")? {
                        JvmStackValue::Int(i) => i,
                        _ => return Err("sastore: index is not an int".into()),
                    };

                    let arrayref = stack.pop().ok_or("sastore: stack underflow (arrayref)")?;

                    let heap_idx = match arrayref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("sastore: arrayref is not a reference".into()),
                    };

                    match jvm.state.lock().heap.get_mut(heap_idx) {
                        Some(HeapObject::Array { element_type, data }) => {
                            if element_type != "primitive_9" {
                                return Err(format!(
                                    "sastore: expected short array, found array of type {}",
                                    element_type
                                ));
                            }

                            if index < 0 || index as usize >= data.len() {
                                return Err(format!(
                                    "java.lang.ArrayIndexOutOfBoundsException: Index {} out of bounds for length {}",
                                    index,
                                    data.len()
                                ));
                            }

                            data[index as usize] = JvmStackValue::Int((value as i16) as i32);
                        }
                        _ => return Err("sastore: object is not an array".into()),
                    }

                    pc += 1;
                }
                0x57 => {
                    // pop
                    stack.pop().ok_or("pop: Stack underflow")?;
                    pc += 1;
                }
                0x58 => {
                    // pop2
                    let value1 = stack.pop().ok_or("pop2: Stack underflow")?;
                    if !JVM::is_category_2_value(&value1) {
                        let value2 = stack.pop().ok_or("pop2: Stack underflow")?;
                        if JVM::is_category_2_value(&value2) {
                            return Err(
                                "pop2: invalid category 2 value under category 1 value".into()
                            );
                        }
                    }
                    pc += 1;
                }
                0x59 => {
                    // dup - Duplicate the top value on the stack

                    let top_value = stack.last().cloned().ok_or("dup: Stack underflow")?;

                    stack.push(top_value);

                    pc += 1;
                }
                0x5c => {
                    // dup2
                    let value1 = stack.last().cloned().ok_or("dup2: Stack underflow")?;

                    if JVM::is_category_2_value(&value1) {
                        stack.push(value1);
                    } else {
                        let value2 = stack
                            .get(stack.len().checked_sub(2).ok_or("dup2: Stack underflow")?)
                            .cloned()
                            .ok_or("dup2: Stack underflow")?;

                        if JVM::is_category_2_value(&value2) {
                            return Err(
                                "dup2: invalid category 2 value under category 1 value".into()
                            );
                        }

                        stack.push(value2);
                        stack.push(value1);
                    }

                    pc += 1;
                }
                0x5d => {
                    // dup2_x1
                    let value1 = stack.pop().ok_or("dup2_x1: stack underflow (value1)")?;

                    if JVM::is_category_2_value(&value1) {
                        let value2 = stack.pop().ok_or("dup2_x1: stack underflow (value2)")?;

                        if JVM::is_category_2_value(&value2) {
                            return Err(
                                "dup2_x1: invalid category 2 value under category 2 value".into()
                            );
                        }

                        stack.push(value1.clone());
                        stack.push(value2);
                        stack.push(value1);
                    } else {
                        let value2 = stack.pop().ok_or("dup2_x1: stack underflow (value2)")?;
                        if JVM::is_category_2_value(&value2) {
                            return Err(
                                "dup2_x1: invalid category 2 value under category 1 value".into()
                            );
                        }

                        let value3 = stack.pop().ok_or("dup2_x1: stack underflow (value3)")?;
                        if JVM::is_category_2_value(&value3) {
                            return Err(
                                "dup2_x1: invalid category 2 value at insertion point".into()
                            );
                        }

                        stack.push(value2.clone());
                        stack.push(value1.clone());
                        stack.push(value3);
                        stack.push(value2);
                        stack.push(value1);
                    }

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
                0x81 => {
                    // lor
                    let val2 = stack.pop().ok_or("lor: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("lor: stack underflow (val1)")?;

                    if let (JvmStackValue::Long(v1), JvmStackValue::Long(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Long(v1 | v2));
                    } else {
                        return Err(format!(
                            "lor: expected two Longs, found {:?} and {:?}",
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
                0x91 => {
                    // i2b
                    let val = stack.pop().ok_or("i2b: Stack underflow")?;
                    if let JvmStackValue::Int(i) = val {
                        stack.push(JvmStackValue::Int((i as i8) as i32));
                    } else {
                        return Err("i2b: expected Int".into());
                    }
                    pc += 1;
                }
                0x93 => {
                    // i2s
                    let val = stack.pop().ok_or("i2s: Stack underflow")?;
                    if let JvmStackValue::Int(i) = val {
                        stack.push(JvmStackValue::Int((i as i16) as i32));
                    } else {
                        return Err("i2s: expected Int".into());
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
                0xA5 | 0xA6 => {
                    // if_acmp<cond>
                    let offset =
                        (((bytecode[pc + 1] as i16) << 8) | (bytecode[pc + 2] as i16)) as i32;

                    let val2 = stack
                        .pop()
                        .ok_or("if_acmp<cond>: stack underflow for val2")?;
                    let val1 = stack
                        .pop()
                        .ok_or("if_acmp<cond>: stack underflow for val1")?;

                    if !JVM::is_reference_value(&val1) || !JVM::is_reference_value(&val2) {
                        return Err(format!(
                            "if_acmp<cond>: expected references, found {:?} and {:?}",
                            val1, val2
                        ));
                    }

                    let equal = JVM::reference_values_equal(&val1, &val2);
                    let condition_met = match opcode {
                        0xA5 => equal,
                        0xA6 => !equal,
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

                    let field_key = JVM::get_field_key(field_ref, cp);
                    let legacy_field_name = JVM::resolve_field_name(field_ref, cp);

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
                            let resolved_field_key = JVM::resolve_instance_field_key(
                                &state,
                                &field_key,
                                &legacy_field_name,
                            );
                            obj.fields
                                .get(&resolved_field_key)
                                .or_else(|| obj.fields.get(&field_key))
                                .or_else(|| obj.fields.get(&legacy_field_name))
                                .ok_or_else(|| {
                                    format!(
                                        "Field '{}' not found in object of class '{}'",
                                        field_key, obj.class_name
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

                    let field_key = JVM::get_field_key(field_ref, cp);
                    let legacy_field_name = JVM::resolve_field_name(field_ref, cp);

                    let value = stack.pop().ok_or("putfield: stack underflow (value)")?;

                    let objectref = stack.pop().ok_or("putfield: stack underflow (objectref)")?;

                    let heap_idx = match objectref {
                        JvmStackValue::ObjectRef(id) => id as usize,
                        JvmStackValue::Null => return Err("java.lang.NullPointerException".into()),
                        _ => return Err("putfield: objectref is not a reference".into()),
                    };

                    let resolved_field_key = {
                        let state = jvm.state.lock();
                        JVM::resolve_instance_field_key(&state, &field_key, &legacy_field_name)
                    };

                    {
                        let mut state = jvm.state.lock();
                        let obj = state
                            .heap
                            .get_mut(heap_idx)
                            .ok_or_else(|| format!("Invalid heap access at index {}", heap_idx))?;

                        if let HeapObject::Instance(obj) = obj {
                            if obj.fields.contains_key(&resolved_field_key)
                                || !obj.fields.contains_key(&legacy_field_name)
                            {
                                obj.fields.insert(resolved_field_key, value);
                            } else if obj.fields.contains_key(&field_key)
                                || !obj.fields.contains_key(&legacy_field_name)
                            {
                                obj.fields.insert(field_key, value);
                            } else {
                                obj.fields.insert(legacy_field_name, value);
                            }
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
                        let error = JVM::append_invoke_context(
                            "java.lang.NullPointerException".to_string(),
                            "invokevirtual",
                            &class_name,
                            &method_name,
                            &descriptor,
                            pc,
                        );
                        pc = JVM::handle_exception_in_current_frame(
                            error,
                            pc,
                            exception_table,
                            cp,
                            jvm,
                            &mut stack,
                        )?;
                        continue;
                    }

                    if matches!(&objectref, JvmStackValue::String(_))
                        || class_name == "java/lang/String"
                    {
                        let res = JVM::execute_method(
                            objectref,
                            "java/lang/String",
                            &method_name,
                            &descriptor,
                            &args,
                            jvm,
                            &mut stack,
                        );

                        if let Err(e) = res {
                            let error = JVM::append_invoke_context(
                                e,
                                "invokevirtual",
                                "java/lang/String",
                                &method_name,
                                &descriptor,
                                pc,
                            );
                            pc = JVM::handle_exception_in_current_frame(
                                error,
                                pc,
                                exception_table,
                                cp,
                                jvm,
                                &mut stack,
                            )?;
                            continue;
                        }
                    } else if let JvmStackValue::ObjectRef(999) = objectref {
                        let return_value = match JVM::handle_native_printstream(
                            &objectref,
                            &method_name,
                            &descriptor,
                            &args,
                            jvm,
                        ) {
                            Ok(return_value) => return_value,
                            Err(e) => {
                                let error = JVM::append_invoke_context(
                                    e,
                                    "invokevirtual",
                                    "java/io/PrintStream",
                                    &method_name,
                                    &descriptor,
                                    pc,
                                );
                                pc = JVM::handle_exception_in_current_frame(
                                    error,
                                    pc,
                                    exception_table,
                                    cp,
                                    jvm,
                                    &mut stack,
                                )?;
                                continue;
                            }
                        };
                        if let Some(val) = return_value {
                            stack.push(val);
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

                            let return_value = match res {
                                Ok(return_value) => return_value,
                                Err(e) => {
                                    let error = JVM::append_invoke_context(
                                        format!("Error handling StringBuffer method: {}", e),
                                        "invokevirtual",
                                        "java/lang/StringBuffer",
                                        &method_name,
                                        &descriptor,
                                        pc,
                                    );
                                    pc = JVM::handle_exception_in_current_frame(
                                        error,
                                        pc,
                                        exception_table,
                                        cp,
                                        jvm,
                                        &mut stack,
                                    )?;
                                    continue;
                                }
                            };

                            if let Some(return_val) = return_value {
                                stack.push(return_val);
                            }
                        }
                    } else {
                        let actual_class_name = {
                            let state = jvm.state.lock();
                            let heap_idx = match objectref {
                                JvmStackValue::ObjectRef(id) => id as usize,
                                _ => {
                                    return Err(format!(
                                        "invokevirtual: objectref is not a reference for {}.{}{}; got {:?}; args {:?}",
                                        class_name, method_name, descriptor, objectref, args
                                    ));
                                }
                            };

                            match state.heap.get(heap_idx) {
                                Some(HeapObject::Instance(obj)) => obj.class_name.clone(),
                                Some(HeapObject::Array { .. }) => class_name.clone(),
                                None => {
                                    return Err(format!(
                                        "invokevirtual: invalid objectref {} for {}.{}{}; args {:?}",
                                        heap_idx, class_name, method_name, descriptor, args
                                    ));
                                }
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
                            let error = JVM::append_invoke_context(
                                e,
                                "invokevirtual",
                                &actual_class_name,
                                &method_name,
                                &descriptor,
                                pc,
                            );
                            pc = JVM::handle_exception_in_current_frame(
                                error,
                                pc,
                                exception_table,
                                cp,
                                jvm,
                                &mut stack,
                            )?;
                            continue;
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
                        let error = JVM::append_invoke_context(
                            "java.lang.NullPointerException".to_string(),
                            "invokespecial",
                            &class_name,
                            &method_name,
                            &descriptor,
                            pc,
                        );
                        pc = JVM::handle_exception_in_current_frame(
                            error,
                            pc,
                            exception_table,
                            cp,
                            jvm,
                            &mut stack,
                        )?;
                        continue;
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
                            let error = JVM::append_invoke_context(
                                e,
                                "invokespecial",
                                &class_name,
                                &method_name,
                                &descriptor,
                                pc,
                            );
                            pc = JVM::handle_exception_in_current_frame(
                                error,
                                pc,
                                exception_table,
                                cp,
                                jvm,
                                &mut stack,
                            )?;
                            continue;
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
                        let error = JVM::append_invoke_context(
                            e,
                            "invokestatic",
                            &class_name,
                            &method_name,
                            &descriptor,
                            pc,
                        );
                        pc = JVM::handle_exception_in_current_frame(
                            error,
                            pc,
                            exception_table,
                            cp,
                            jvm,
                            &mut stack,
                        )?;
                        continue;
                    }

                    pc += 3;
                }
                0xB9 => {
                    // invokeinterface
                    let cp_index =
                        u16::from_be_bytes([bytecode[pc + 1], bytecode[pc + 2]]) as usize;

                    // The count byte (not strictly needed as we can derive from descriptor)
                    let _count = bytecode[pc + 3];

                    // The fourth byte must be zero (reserved for future use)
                    let _reserved = bytecode[pc + 4];

                    // Resolve interface method info
                    let (class_name, method_name, descriptor) = match &cp[cp_index - 1] {
                        ConstantInfo::InterfaceMethodRef(iface_ref) => {
                            JVM::resolve_interface_method_identity(iface_ref, cp)
                        }
                        ConstantInfo::MethodRef(m_ref) => {
                            // Fallback for implementations that use MethodRef for interface methods
                            JVM::resolve_method_identity(m_ref, cp)
                        }
                        _ => {
                            return Err(format!(
                                "invokeinterface: expected InterfaceMethodRef at index {}",
                                cp_index
                            )
                            .into());
                        }
                    };

                    // get count of args
                    let arg_count = JVM::count_arguments(&descriptor);

                    // get args from stack
                    let mut args = Vec::new();
                    for _ in 0..arg_count {
                        args.push(stack.pop().ok_or("Stack underflow: missing arguments")?);
                    }

                    args.reverse(); // Maintain original order: [arg1, arg2, ...]

                    let objectref = stack.pop().ok_or("Stack underflow: missing objectref")?;

                    // if objectref is null, throw NullPointerException
                    if let JvmStackValue::Null = objectref {
                        let error = JVM::append_invoke_context(
                            "java.lang.NullPointerException".to_string(),
                            "invokeinterface",
                            &class_name,
                            &method_name,
                            &descriptor,
                            pc,
                        );
                        pc = JVM::handle_exception_in_current_frame(
                            error,
                            pc,
                            exception_table,
                            cp,
                            jvm,
                            &mut stack,
                        )?;
                        continue;
                    }

                    // For interface method lookup:
                    // 1. First check if the object's actual class has the method
                    // 2. Then search superclasses
                    // 3. Then search superinterfaces

                    let actual_class_name = {
                        let state = jvm.state.lock();
                        if let JvmStackValue::ObjectRef(id) = &objectref {
                            if let Some(HeapObject::Instance(obj)) = state.heap.get(*id as usize) {
                                obj.class_name.clone()
                            } else {
                                class_name.clone()
                            }
                        } else {
                            return Err("invokeinterface: objectref is not a reference".into());
                        }
                    };

                    // Execute the method on the actual class
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
                        let error = JVM::append_invoke_context(
                            e,
                            "invokeinterface",
                            &actual_class_name,
                            &method_name,
                            &descriptor,
                            pc,
                        );
                        pc = JVM::handle_exception_in_current_frame(
                            error,
                            pc,
                            exception_table,
                            cp,
                            jvm,
                            &mut stack,
                        )?;
                        continue;
                    }

                    pc += 5; // invokeinterface is 5 bytes total
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
                0x79 => {
                    // lshl
                    let val2 = stack.pop().ok_or("lshl: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("lshl: stack underflow (val1)")?;

                    if let (JvmStackValue::Long(v1), JvmStackValue::Int(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        let s = (v2 & 0x3f) as u32;
                        stack.push(JvmStackValue::Long(v1.wrapping_shl(s)));
                    } else {
                        return Err(format!(
                            "lshl: expected Long and Int, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x7A => {
                    // ishr
                    let val2 = stack.pop().ok_or("ishr: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("ishr: stack underflow (val1)")?;

                    if let (JvmStackValue::Int(v1), JvmStackValue::Int(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        let s = (v2 & 0x1f) as u32;
                        stack.push(JvmStackValue::Int(v1 >> s));
                    } else {
                        return Err(format!(
                            "ishr: expected two Ints, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x7C => {
                    // iushr
                    let val2 = stack.pop().ok_or("iushr: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("iushr: stack underflow (val1)")?;

                    if let (JvmStackValue::Int(v1), JvmStackValue::Int(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        let s = (v2 & 0x1f) as u32;
                        stack.push(JvmStackValue::Int(((v1 as u32) >> s) as i32));
                    } else {
                        return Err(format!(
                            "iushr: expected two Ints, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
                }
                0x7D => {
                    // lushr
                    let val2 = stack.pop().ok_or("lushr: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("lushr: stack underflow (val1)")?;

                    if let (JvmStackValue::Long(v1), JvmStackValue::Int(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        let s = (v2 & 0x3f) as u32;
                        stack.push(JvmStackValue::Long(((v1 as u64) >> s) as i64));
                    } else {
                        return Err(format!(
                            "lushr: expected Long and Int, found {:?} and {:?}",
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
                0x7F => {
                    // land
                    let val2 = stack.pop().ok_or("land: stack underflow (val2)")?;
                    let val1 = stack.pop().ok_or("land: stack underflow (val1)")?;

                    if let (JvmStackValue::Long(v1), JvmStackValue::Long(v2)) =
                        (val1.clone(), val2.clone())
                    {
                        stack.push(JvmStackValue::Long(v1 & v2));
                    } else {
                        return Err(format!(
                            "land: expected two Longs, found {:?} and {:?}",
                            val1, val2
                        )
                        .into());
                    }
                    pc += 1;
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
                0xAD => {
                    // lreturn
                    let val = stack.pop().ok_or("lreturn: Stack underflow")?;
                    if let JvmStackValue::Long(_) = val {
                        jvm_debug!("Execution finished with long return value: {:?}", val);
                        return Ok(Some(val));
                    }

                    return Err(format!("lreturn: expected Long, found {:?}", val));
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

    fn resolve_instance_field_key(
        state: &JvmState,
        field_key: &str,
        legacy_field_name: &str,
    ) -> String {
        let Some((referenced_class_name, _)) = field_key.rsplit_once('.') else {
            return field_key.to_string();
        };

        let mut current_class = Some(referenced_class_name.to_string());
        while let Some(class_name) = current_class {
            let Some(class_data) = state.classes.get(&class_name) else {
                break;
            };

            for field_info in &class_data.fields {
                if field_info
                    .access_flags
                    .contains(classfile_parser::field_info::FieldAccessFlags::STATIC)
                {
                    continue;
                }

                let field_name = JVM::resolve_utf8(field_info.name_index, &class_data.const_pool);
                let descriptor =
                    JVM::resolve_utf8(field_info.descriptor_index, &class_data.const_pool);
                if format!("{}:{}", field_name, descriptor) == legacy_field_name {
                    return format!("{}.{}:{}", class_name, field_name, descriptor);
                }
            }

            current_class = JVM::get_super_class_name(class_data);
        }

        field_key.to_string()
    }

    fn count_arguments(descriptor: &str) -> usize {
        let mut count = 0;
        let mut chars = descriptor.chars().peekable();

        if chars.next() != Some('(') {
            return 0;
        }

        while let Some(c) = chars.next() {
            match c {
                ')' => break,
                '[' => {
                    count += 1;

                    while matches!(chars.peek(), Some('[')) {
                        chars.next();
                    }

                    if chars.next() == Some('L') {
                        while let Some(ch) = chars.next() {
                            if ch == ';' {
                                break;
                            }
                        }
                    }
                }
                'L' => {
                    count += 1;
                    while let Some(ch) = chars.next() {
                        if ch == ';' {
                            break;
                        }
                    }
                }
                'B' | 'C' | 'D' | 'F' | 'I' | 'J' | 'S' | 'Z' => count += 1,
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

    fn resolve_interface_method_identity(
        m: &classfile_parser::constant_info::InterfaceMethodRefConstant,
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

    fn handle_hashtable_fns(
        object_ref: &mut HeapObject,
        method: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        let heap_obj = if let HeapObject::Instance(obj) = object_ref {
            obj
        } else {
            return Err("Expected instance for Hashtable object".into());
        };

        match method {
            "<init>" => {
                heap_obj
                    .fields
                    .insert("keys".to_string(), JvmStackValue::Vector(Vec::new()));
                heap_obj
                    .fields
                    .insert("values".to_string(), JvmStackValue::Vector(Vec::new()));
                Ok(None)
            }
            "put" => {
                let key = args[0].clone();
                let value = args[1].clone();
                let mut old_value = JvmStackValue::Null;

                let mut keys = match heap_obj.fields.get("keys").unwrap() {
                    JvmStackValue::Vector(v) => v.clone(),
                    _ => unreachable!(),
                };
                let mut values = match heap_obj.fields.get("values").unwrap() {
                    JvmStackValue::Vector(v) => v.clone(),
                    _ => unreachable!(),
                };

                if let Some(pos) = keys.iter().position(|k| k == &key) {
                    old_value = values[pos].clone();
                    values[pos] = value;
                } else {
                    keys.push(key);
                    values.push(value);
                }

                heap_obj
                    .fields
                    .insert("keys".to_string(), JvmStackValue::Vector(keys));
                heap_obj
                    .fields
                    .insert("values".to_string(), JvmStackValue::Vector(values));

                Ok(Some(old_value))
            }
            "get" => {
                let key = args[0].clone();
                let keys = match heap_obj.fields.get("keys").unwrap() {
                    JvmStackValue::Vector(v) => v,
                    _ => unreachable!(),
                };
                let values = match heap_obj.fields.get("values").unwrap() {
                    JvmStackValue::Vector(v) => v,
                    _ => unreachable!(),
                };

                if let Some(pos) = keys.iter().position(|k| k == &key) {
                    Ok(Some(values[pos].clone()))
                } else {
                    Ok(Some(JvmStackValue::Null))
                }
            }
            "size" => {
                let keys = match heap_obj.fields.get("keys").unwrap() {
                    JvmStackValue::Vector(v) => v,
                    _ => unreachable!(),
                };
                Ok(Some(JvmStackValue::Int(keys.len() as i32)))
            }
            "remove" => {
                let key = args[0].clone();
                let mut keys = match heap_obj.fields.get("keys").unwrap() {
                    JvmStackValue::Vector(v) => v.clone(),
                    _ => unreachable!(),
                };
                let mut values = match heap_obj.fields.get("values").unwrap() {
                    JvmStackValue::Vector(v) => v.clone(),
                    _ => unreachable!(),
                };

                let old_value = if let Some(pos) = keys.iter().position(|k| k == &key) {
                    keys.remove(pos);
                    let val = values.remove(pos);
                    val
                } else {
                    JvmStackValue::Null
                };

                heap_obj
                    .fields
                    .insert("keys".to_string(), JvmStackValue::Vector(keys));
                heap_obj
                    .fields
                    .insert("values".to_string(), JvmStackValue::Vector(values));

                Ok(Some(old_value))
            }
            "clear" => {
                heap_obj
                    .fields
                    .insert("keys".to_string(), JvmStackValue::Vector(Vec::new()));
                heap_obj
                    .fields
                    .insert("values".to_string(), JvmStackValue::Vector(Vec::new()));
                Ok(None)
            }
            "isEmpty" => {
                let keys = match heap_obj.fields.get("keys").unwrap() {
                    JvmStackValue::Vector(v) => v,
                    _ => unreachable!(),
                };
                Ok(Some(JvmStackValue::Int(if keys.is_empty() {
                    1
                } else {
                    0
                })))
            }
            "contains" | "containsValue" => {
                let value = args[0].clone();
                let values = match heap_obj.fields.get("values").unwrap() {
                    JvmStackValue::Vector(v) => v,
                    _ => unreachable!(),
                };
                let res = values.contains(&value);
                Ok(Some(JvmStackValue::Int(if res { 1 } else { 0 })))
            }
            "containsKey" => {
                let key = args[0].clone();
                let keys = match heap_obj.fields.get("keys").unwrap() {
                    JvmStackValue::Vector(v) => v,
                    _ => unreachable!(),
                };
                let res = keys.contains(&key);
                Ok(Some(JvmStackValue::Int(if res { 1 } else { 0 })))
            }
            _ => {
                println!(
                    "[-] Unknown Hashtable method: {} | args = {:?}",
                    method, args
                );
                panic!();
            }
        }
    }

    fn handle_random_fns(
        object_ref: &mut HeapObject,
        method: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        let heap_obj = if let HeapObject::Instance(obj) = object_ref {
            obj
        } else {
            return Err("Expected instance for Random object".into());
        };

        match method {
            "<init>" => {
                let seed = if let Some(JvmStackValue::Long(s)) = args.get(0) {
                    (*s ^ 0x5DEECE66Di64) & ((1i64 << 48) - 1)
                } else if args.is_empty() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos() as i64;
                    (now ^ 0x5DEECE66Di64) & ((1i64 << 48) - 1)
                } else {
                    return Err("Random.<init> invalid args".into());
                };
                heap_obj
                    .fields
                    .insert("seed".to_string(), JvmStackValue::Long(seed));
                Ok(None)
            }
            "setSeed" => {
                let seed = match args.get(0) {
                    Some(JvmStackValue::Long(s)) => *s,
                    _ => return Err("Random.setSeed expects Long argument".into()),
                };
                let new_seed = (seed ^ 0x5DEECE66Di64) & ((1i64 << 48) - 1);
                heap_obj
                    .fields
                    .insert("seed".to_string(), JvmStackValue::Long(new_seed));
                Ok(None)
            }
            "nextInt" => {
                let mut current_seed = match heap_obj.fields.get("seed") {
                    Some(JvmStackValue::Long(s)) => *s,
                    _ => 0,
                };

                if let Some(JvmStackValue::Int(n)) = args.get(0) {
                    if *n <= 0 {
                        return Err("Random.nextInt(n) must be positive".into());
                    }

                    let mut next_seed = (current_seed
                        .wrapping_mul(0x5DEECE66Di64)
                        .wrapping_add(0xBi64))
                        & ((1i64 << 48) - 1);
                    heap_obj
                        .fields
                        .insert("seed".to_string(), JvmStackValue::Long(next_seed));

                    let n = *n as i64;
                    if (n & -n) == n {
                        let bits = (next_seed >> 17) as i32;
                        let res = ((n * (bits as i64)) >> 31) as i32;
                        return Ok(Some(JvmStackValue::Int(res)));
                    }

                    let mut bits;
                    let mut val;
                    loop {
                        bits = (next_seed >> 17) as i32;
                        val = bits % (n as i32);
                        if bits - val + (n as i32) - 1 >= 0 {
                            break;
                        }
                        next_seed = (next_seed.wrapping_mul(0x5DEECE66Di64).wrapping_add(0xBi64))
                            & ((1i64 << 48) - 1);
                        heap_obj
                            .fields
                            .insert("seed".to_string(), JvmStackValue::Long(next_seed));
                    }
                    Ok(Some(JvmStackValue::Int(val)))
                } else {
                    current_seed = (current_seed
                        .wrapping_mul(0x5DEECE66Di64)
                        .wrapping_add(0xBi64))
                        & ((1i64 << 48) - 1);
                    heap_obj
                        .fields
                        .insert("seed".to_string(), JvmStackValue::Long(current_seed));
                    let res = (current_seed >> 16) as i32;
                    Ok(Some(JvmStackValue::Int(res)))
                }
            }
            "nextLong" => {
                let mut current_seed = match heap_obj.fields.get("seed") {
                    Some(JvmStackValue::Long(s)) => *s,
                    _ => 0,
                };
                current_seed = (current_seed
                    .wrapping_mul(0x5DEECE66Di64)
                    .wrapping_add(0xBi64))
                    & ((1i64 << 48) - 1);
                let high = (current_seed >> 16) as i32 as i64;
                current_seed = (current_seed
                    .wrapping_mul(0x5DEECE66Di64)
                    .wrapping_add(0xBi64))
                    & ((1i64 << 48) - 1);
                let low = (current_seed >> 16) as i32 as i64;
                heap_obj
                    .fields
                    .insert("seed".to_string(), JvmStackValue::Long(current_seed));

                let res = (high << 32) + (low & 0xFFFFFFFFu32 as i64);
                Ok(Some(JvmStackValue::Long(res)))
            }
            _ => Err(format!("Random method {} not implemented", method)),
        }
    }

    fn handle_integer_fns(
        object_ref: &mut HeapObject,
        method: &str,
        args: &[JvmStackValue],
    ) -> Result<Option<JvmStackValue>, String> {
        let heap_obj = if let HeapObject::Instance(obj) = object_ref {
            obj
        } else {
            return Err("Expected instance for Integer object".into());
        };

        match method {
            "<init>" => {
                let value = match args.get(0) {
                    Some(JvmStackValue::Int(v)) => *v,
                    _ => return Err("Integer.<init> expects Int argument".into()),
                };
                heap_obj
                    .fields
                    .insert("value".to_string(), JvmStackValue::Int(value));
                Ok(None)
            }
            "intValue" => {
                let val = heap_obj.fields.get("value").unwrap().clone();
                Ok(Some(val))
            }
            "toString" => {
                let val = match heap_obj.fields.get("value").unwrap() {
                    JvmStackValue::Int(v) => v,
                    _ => unreachable!(),
                };
                Ok(Some(JvmStackValue::String(val.to_string())))
            }
            "equals" => Err(format!("Integer method {} not implemented", method)),
            _ => Err(format!("Integer method {} not implemented", method)),
        }
    }

    fn handle_integer_static_fns(
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match method {
            "parseInt" => {
                let s = match args.get(0) {
                    Some(JvmStackValue::String(s)) => s,
                    _ => return Err("Integer.parseInt expects String argument".into()),
                };
                let radix = if args.len() > 1 {
                    match args.get(1) {
                        Some(JvmStackValue::Int(r)) => *r as u32,
                        _ => 10,
                    }
                } else {
                    10
                };
                let parsed = i32::from_str_radix(s.trim(), radix).unwrap_or(0);
                Ok(Some(JvmStackValue::Int(parsed)))
            }
            "toString" => {
                let val = match args.get(0) {
                    Some(JvmStackValue::Int(v)) => *v,
                    _ => return Err("Integer.toString expects Int argument".into()),
                };
                let radix = if args.len() > 1 {
                    match args.get(1) {
                        Some(JvmStackValue::Int(r)) => *r as u32,
                        _ => 10,
                    }
                } else {
                    10
                };
                let s = if radix == 16 {
                    format!("{:x}", val)
                } else {
                    val.to_string()
                };
                Ok(Some(JvmStackValue::String(s)))
            }
            "valueOf" => {
                if let Some(JvmStackValue::String(s)) = args.get(0) {
                    let radix = if args.len() > 1 {
                        match args.get(1) {
                            Some(JvmStackValue::Int(r)) => *r as u32,
                            _ => 10,
                        }
                    } else {
                        10
                    };
                    let parsed = i32::from_str_radix(s.trim(), radix).unwrap_or(0);

                    let id = jvm.allocate("java/lang/Integer".to_string());
                    {
                        let mut state = jvm.state.lock();
                        let heap_obj = state.heap.get_mut(id as usize).unwrap();
                        if let HeapObject::Instance(obj) = heap_obj {
                            obj.fields
                                .insert("value".to_string(), JvmStackValue::Int(parsed));
                        }
                    }
                    Ok(Some(JvmStackValue::ObjectRef(id)))
                } else if let Some(JvmStackValue::Int(v)) = args.get(0) {
                    let id = jvm.allocate("java/lang/Integer".to_string());
                    {
                        let mut state = jvm.state.lock();
                        let heap_obj = state.heap.get_mut(id as usize).unwrap();
                        if let HeapObject::Instance(obj) = heap_obj {
                            obj.fields
                                .insert("value".to_string(), JvmStackValue::Int(*v));
                        }
                    }
                    Ok(Some(JvmStackValue::ObjectRef(id)))
                } else {
                    Err("Integer.valueOf invalid args".into())
                }
            }
            _ => Err(format!("Integer static method {} not implemented", method)),
        }
    }

    fn handle_native_printstream(
        objectref: &JvmStackValue,
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("println", _) => {
                if let Some(val) = args.first() {
                    println!("{}", JVM::printstream_value_to_string(val, descriptor, jvm));
                } else {
                    println!();
                }
                Ok(None)
            }
            ("print", _) => {
                if let Some(val) = args.first() {
                    print!("{}", JVM::printstream_value_to_string(val, descriptor, jvm));
                }
                Ok(None)
            }
            ("append", descriptor) if descriptor.starts_with("(C)") => {
                if let Some(val) = args.first() {
                    print!("{}", JVM::printstream_char_to_string(val));
                }
                Ok(Some(objectref.clone()))
            }
            ("append", descriptor)
                if descriptor.starts_with("(Ljava/lang/String;")
                    || descriptor.starts_with("(Ljava/lang/Object;") =>
            {
                let value = args
                    .first()
                    .map(|val| JVM::char_sequence_to_string(val, jvm))
                    .unwrap_or_else(|| "null".to_string());
                print!("{}", value);
                Ok(Some(objectref.clone()))
            }
            ("append", descriptor) if descriptor.starts_with("(Ljava/lang/CharSequence;)") => {
                let value = args
                    .first()
                    .map(|val| JVM::char_sequence_to_string(val, jvm))
                    .unwrap_or_else(|| "null".to_string());
                print!("{}", value);
                Ok(Some(objectref.clone()))
            }
            ("append", descriptor) if descriptor.starts_with("(Ljava/lang/CharSequence;II)") => {
                let value = args
                    .first()
                    .map(|val| JVM::char_sequence_to_string(val, jvm))
                    .unwrap_or_else(|| "null".to_string());
                let start = match args.get(1) {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => {
                        return Err(format!(
                            "PrintStream.append(CharSequence,int,int): invalid start {:?}",
                            value
                        ));
                    }
                };
                let end = match args.get(2) {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => {
                        return Err(format!(
                            "PrintStream.append(CharSequence,int,int): invalid end {:?}",
                            value
                        ));
                    }
                };
                print!("{}", JVM::substring_by_char_range(&value, start, end)?);
                Ok(Some(objectref.clone()))
            }
            ("toString", "()Ljava/lang/String;") => Ok(Some(JvmStackValue::String(
                "java.io.PrintStream".to_string(),
            ))),
            _ => {
                println!(
                    "Native PrintStream called unknown method: {}{}",
                    method, descriptor
                );
                Ok(None)
            }
        }
    }

    fn printstream_value_to_string(value: &JvmStackValue, descriptor: &str, jvm: &JVM) -> String {
        if descriptor.starts_with("(C)") {
            return JVM::printstream_char_to_string(value);
        }

        JVM::char_sequence_to_string(value, jvm)
    }

    fn printstream_char_to_string(value: &JvmStackValue) -> String {
        match value {
            JvmStackValue::Int(value) => char::from_u32(*value as u32)
                .unwrap_or(char::REPLACEMENT_CHARACTER)
                .to_string(),
            other => JVM::char_sequence_to_string_without_heap(other),
        }
    }

    fn char_sequence_to_string(value: &JvmStackValue, jvm: &JVM) -> String {
        match value {
            JvmStackValue::ObjectRef(id) => {
                let state = jvm.state.lock();
                match state.heap.get(*id as usize) {
                    Some(HeapObject::Instance(obj)) => {
                        if let Some(JvmStackValue::String(buffer)) = obj.fields.get("buffer") {
                            buffer.clone()
                        } else {
                            format!("{}@{:x}", obj.class_name.replace('/', "."), id)
                        }
                    }
                    _ => format!("ObjectRef({})", id),
                }
            }
            other => JVM::char_sequence_to_string_without_heap(other),
        }
    }

    fn char_sequence_to_string_without_heap(value: &JvmStackValue) -> String {
        match value {
            JvmStackValue::String(value) => value.clone(),
            JvmStackValue::Int(value) => value.to_string(),
            JvmStackValue::Float(value) => value.to_string(),
            JvmStackValue::Long(value) => value.to_string(),
            JvmStackValue::Double(value) => value.to_string(),
            JvmStackValue::Null => "null".to_string(),
            other => format!("{:?}", other),
        }
    }

    fn java_string_hash(value: &str) -> i32 {
        value.chars().fold(0i32, |hash, ch| {
            hash.wrapping_mul(31).wrapping_add(ch as i32)
        })
    }

    fn allocate_class_object(jvm: &JVM, class_name: String) -> JvmStackValue {
        let class_obj = JvmObject {
            class_name: "java/lang/Class".to_string(),
            fields: {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), JvmStackValue::String(class_name));
                fields
            },
        };

        let mut state = jvm.state.lock();
        state.heap.push(HeapObject::Instance(class_obj));
        JvmStackValue::ObjectRef((state.heap.len() - 1) as u32)
    }

    fn handle_object_fns(
        objectref: &JvmStackValue,
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("<init>", "()V") => Ok(None),
            ("getClass", "()Ljava/lang/Class;") => {
                let class_name = match objectref {
                    JvmStackValue::ObjectRef(id) => {
                        let state = jvm.state.lock();
                        match state.heap.get(*id as usize) {
                            Some(HeapObject::Instance(obj)) => obj.class_name.clone(),
                            Some(HeapObject::Array { element_type, .. }) => {
                                format!("[{}", element_type)
                            }
                            None => {
                                return Err(format!(
                                    "Object.getClass: invalid heap reference {}",
                                    id
                                ));
                            }
                        }
                    }
                    JvmStackValue::String(_) => "java/lang/String".to_string(),
                    JvmStackValue::Null => {
                        return Err("Object.getClass: NullPointerException".into());
                    }
                    value => {
                        return Err(format!(
                            "Object.getClass: expected reference, found {:?}",
                            value
                        ));
                    }
                };

                Ok(Some(JVM::allocate_class_object(jvm, class_name)))
            }
            ("toString", "()Ljava/lang/String;") => Ok(Some(JvmStackValue::String(
                JVM::char_sequence_to_string(objectref, jvm),
            ))),
            ("equals", "(Ljava/lang/Object;)Z") => {
                let is_equal = args
                    .first()
                    .map(|arg| JVM::reference_values_equal(objectref, arg))
                    .unwrap_or(false);
                Ok(Some(JvmStackValue::Int(if is_equal { 1 } else { 0 })))
            }
            ("hashCode", "()I") => {
                let hash = match objectref {
                    JvmStackValue::ObjectRef(id) => *id as i32,
                    JvmStackValue::String(value) => JVM::java_string_hash(value),
                    JvmStackValue::Null => {
                        return Err("Object.hashCode: NullPointerException".into());
                    }
                    value => {
                        return Err(format!(
                            "Object.hashCode: expected reference, found {:?}",
                            value
                        ));
                    }
                };
                Ok(Some(JvmStackValue::Int(hash)))
            }
            _ => Err(format!(
                "Unsupported Object method: {}{}",
                method, descriptor
            )),
        }
    }

    fn substring_by_char_range(value: &str, start: i32, end: i32) -> Result<String, String> {
        if start < 0 || end < start {
            return Err(format!(
                "java.lang.IndexOutOfBoundsException: start {}, end {}",
                start, end
            ));
        }

        let start = start as usize;
        let end = end as usize;
        let chars: Vec<char> = value.chars().collect();
        if end > chars.len() {
            return Err(format!(
                "java.lang.IndexOutOfBoundsException: end {} out of bounds for length {}",
                end,
                chars.len()
            ));
        }

        Ok(chars[start..end].iter().collect())
    }

    fn is_category_2_value(value: &JvmStackValue) -> bool {
        matches!(value, JvmStackValue::Long(_) | JvmStackValue::Double(_))
    }

    fn handle_thread_init(
        objectref: &JvmStackValue,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<(), String> {
        let target = if descriptor.starts_with("(Ljava/lang/Runnable;") {
            args.first().cloned().unwrap_or(JvmStackValue::Null)
        } else {
            JvmStackValue::Null
        };

        if !matches!(target, JvmStackValue::ObjectRef(_) | JvmStackValue::Null) {
            return Err(format!(
                "Thread.<init>: expected Runnable reference, found {:?}",
                target
            ));
        }

        let heap_idx = match objectref {
            JvmStackValue::ObjectRef(id) => *id as usize,
            _ => return Err("Thread.<init>: expected object reference".into()),
        };

        let mut state = jvm.state.lock();
        let heap_obj = state
            .heap
            .get_mut(heap_idx)
            .ok_or_else(|| format!("Thread.<init>: invalid heap reference {}", heap_idx))?;

        let HeapObject::Instance(obj) = heap_obj else {
            return Err("Thread.<init>: object reference is not an instance".into());
        };

        obj.fields.insert(
            "java/lang/Thread.target:Ljava/lang/Runnable;".to_string(),
            target,
        );

        Ok(())
    }

    fn get_thread_target(
        objectref: &JvmStackValue,
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        let heap_idx = match objectref {
            JvmStackValue::ObjectRef(id) => *id as usize,
            _ => return Err("Thread.run: expected object reference".into()),
        };

        let state = jvm.state.lock();
        let heap_obj = state
            .heap
            .get(heap_idx)
            .ok_or_else(|| format!("Thread.run: invalid heap reference {}", heap_idx))?;

        let HeapObject::Instance(obj) = heap_obj else {
            return Err("Thread.run: object reference is not an instance".into());
        };

        match obj
            .fields
            .get("java/lang/Thread.target:Ljava/lang/Runnable;")
        {
            Some(JvmStackValue::ObjectRef(id)) => Ok(Some(JvmStackValue::ObjectRef(*id))),
            Some(JvmStackValue::Null) | None => Ok(None),
            Some(value) => Err(format!("Thread.run: invalid Runnable target {:?}", value)),
        }
    }

    fn execute_default_thread_run(objectref: &JvmStackValue, jvm: &JVM) -> Result<(), String> {
        let Some(target) = JVM::get_thread_target(objectref, jvm)? else {
            return Ok(());
        };

        let target_class_name = {
            let state = jvm.state.lock();
            let target_idx = match target {
                JvmStackValue::ObjectRef(id) => id as usize,
                _ => return Ok(()),
            };

            match state.heap.get(target_idx) {
                Some(HeapObject::Instance(obj)) => obj.class_name.clone(),
                Some(_) => return Err("Thread.run: Runnable target is not an instance".into()),
                None => {
                    return Err(format!(
                        "Thread.run: invalid Runnable target reference {}",
                        target_idx
                    ));
                }
            }
        };

        let mut caller_stack = Vec::new();
        JVM::execute_method(
            target,
            &target_class_name,
            "run",
            "()V",
            &[],
            jvm,
            &mut caller_stack,
        )
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
        JVM::execute_method_inner(
            objectref,
            class_name,
            method_name,
            descriptor,
            args,
            jvm,
            caller_stack,
        )
        .map_err(|e| JVM::append_method_context(e, class_name, method_name, descriptor))
    }

    fn execute_method_inner(
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
                let return_value = midlet::handle_virtual_method(
                    method_name,
                    descriptor,
                    args,
                    jvm.loaded_jar.as_ref(),
                );

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
            if class_name == player::CLASS_NAME {
                let return_value =
                    player::handle_virtual_method(&objectref, method_name, descriptor, args, jvm);

                if let Err(e) = &return_value {
                    return Err(format!("Error handling Player method: {}", e).into());
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == player::VOLUME_CONTROL_CLASS_NAME {
                let return_value = player::handle_volume_control_method(
                    &objectref,
                    method_name,
                    descriptor,
                    args,
                    jvm,
                );

                if let Err(e) = &return_value {
                    return Err(format!("Error handling VolumeControl method: {}", e).into());
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == rms::RECORD_STORE_CLASS_NAME {
                let return_value =
                    rms::handle_record_store_method(&objectref, method_name, descriptor, args, jvm);

                if let Err(e) = &return_value {
                    return Err(format!("Error handling RecordStore method: {}", e));
                }

                if let Some(val) = return_value.unwrap() {
                    caller_stack.push(val);
                }

                return Ok(());
            }
            if class_name == rms::RECORD_ENUMERATION_CLASS_NAME {
                let return_value = rms::handle_record_enumeration_method_with_args(
                    &objectref,
                    method_name,
                    descriptor,
                    args,
                    jvm,
                );

                if let Err(e) = &return_value {
                    return Err(format!("Error handling RecordEnumeration method: {}", e));
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
            let return_value =
                JVM::handle_native_printstream(&objectref, method_name, descriptor, args, jvm)?;
            if let Some(val) = return_value {
                caller_stack.push(val);
            }
            return Ok(());
        } else if class_name == "java/lang/String" {
            let return_value =
                JVM::handle_string_fns(objectref, method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Object" {
            let return_value =
                JVM::handle_object_fns(&objectref, method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Thread" && method_name == "<init>" {
            JVM::handle_thread_init(&objectref, descriptor, args, jvm)?;
            return Ok(());
        } else if class_name == "java/util/Calendar" {
            let return_value =
                JVM::handle_calendar_fns(&objectref, method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/util/Date" {
            let return_value =
                JVM::handle_date_fns(&objectref, method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/util/TimeZone" {
            let return_value =
                JVM::handle_timezone_fns(&objectref, method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                caller_stack.push(val);
            }

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
        } else if class_name == "java/util/Hashtable" {
            let this_id = match objectref {
                JvmStackValue::ObjectRef(id) => id,
                _ => return Err("Hashtable: NullPointerException".into()),
            };

            let res = {
                let mut state = jvm.state.lock();
                let object_ref = state
                    .heap
                    .get_mut(this_id as usize)
                    .ok_or_else(|| format!("Invalid heap reference: {}", this_id))?;
                JVM::handle_hashtable_fns(object_ref, method_name, args)
            };

            if let Err(e) = res {
                return Err(format!("Error handling Hashtable method: {}", e).into());
            }

            let return_value = res.unwrap();
            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/util/Random" {
            let this_id = match objectref {
                JvmStackValue::ObjectRef(id) => id,
                _ => return Err("Random: NullPointerException".into()),
            };

            let res = {
                let mut state = jvm.state.lock();
                let object_ref = state
                    .heap
                    .get_mut(this_id as usize)
                    .ok_or_else(|| format!("Invalid heap reference: {}", this_id))?;
                JVM::handle_random_fns(object_ref, method_name, args)
            };

            if let Err(e) = res {
                return Err(format!("Error handling Random method: {}", e).into());
            }

            let return_value = res.unwrap();
            if let Some(val) = return_value {
                caller_stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Integer" {
            let this_id = match objectref {
                JvmStackValue::ObjectRef(id) => id,
                _ => return Err("Integer: NullPointerException".into()),
            };

            let res = {
                let mut state = jvm.state.lock();
                let object_ref = state
                    .heap
                    .get_mut(this_id as usize)
                    .ok_or_else(|| format!("Invalid heap reference: {}", this_id))?;
                JVM::handle_integer_fns(object_ref, method_name, args)
            };

            if let Err(e) = res {
                return Err(format!("Error handling Integer method: {}", e).into());
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
            if matches!(
                (method_name, descriptor),
                ("<init>", "()V")
                    | ("toString", "()Ljava/lang/String;")
                    | ("equals", "(Ljava/lang/Object;)Z")
                    | ("hashCode", "()I")
            ) {
                let return_value =
                    JVM::handle_object_fns(&objectref, method_name, descriptor, args, jvm)?;

                if let Some(val) = return_value {
                    caller_stack.push(val);
                }

                return Ok(());
            }

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

            if JVM::class_extends(jvm, class_name, midlet::CLASS_NAME) {
                let return_value = midlet::handle_virtual_method(
                    method_name,
                    descriptor,
                    args,
                    jvm.loaded_jar.as_ref(),
                )?;

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
            let is_thread_run = method_name == "run" && descriptor == "()V";

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
                    if jvm.is_shutdown() {
                        caller_stack.push(JvmStackValue::Int(0));
                        return Ok(());
                    }

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
                } else if is_thread_run {
                    JVM::execute_default_thread_run(&objectref, jvm)?;
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

        let return_value = JVM::run_frame(
            &code_attr.code,
            &const_pool,
            &code_attr.exception_table,
            &mut locals,
            jvm,
        )?;

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
                    let exception_table = JVM::exception_handlers_from_code_attribute(&code_attr);
                    return Some(Code {
                        max_stack: code_attr.max_stack,
                        max_locals: code_attr.max_locals,
                        code: code_attr.code,
                        exception_table,
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
            } else if class_name == "java/util/Hashtable" {
                fields.insert("keys".to_string(), JvmStackValue::Vector(Vec::new()));
                fields.insert("values".to_string(), JvmStackValue::Vector(Vec::new()));
            } else if class_name == "java/util/Random" {
                fields.insert("seed".to_string(), JvmStackValue::Long(0));
            } else if class_name == "java/lang/Integer" {
                fields.insert("value".to_string(), JvmStackValue::Int(0));
            } else if class_name == "java/util/Calendar" {
                fields.insert(
                    "time".to_string(),
                    JvmStackValue::Long(self.current_time_millis()),
                );
                fields.insert(
                    "timezone".to_string(),
                    JvmStackValue::String("GMT".to_string()),
                );
            } else if class_name == "java/util/Date" {
                fields.insert(
                    "time".to_string(),
                    JvmStackValue::Long(self.current_time_millis()),
                );
            } else if class_name == "java/util/TimeZone" {
                fields.insert("id".to_string(), JvmStackValue::String("GMT".to_string()));
                fields.insert("rawOffset".to_string(), JvmStackValue::Int(0));
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
                            let key = format!("{}.{}:{}", name, f_name, descriptor);
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
                JVM::ensure_string_buffer_field(heap_obj);

                let val = args.get(1).cloned().unwrap_or(JvmStackValue::Null);
                let append_str = match val {
                    JvmStackValue::String(s) => s,
                    JvmStackValue::Int(i) => i.to_string(),
                    JvmStackValue::Float(f) => f.to_string(),
                    JvmStackValue::Long(l) => l.to_string(),
                    JvmStackValue::Double(d) => d.to_string(),
                    JvmStackValue::Null => "null".to_string(),
                    _ => format!("{:?}", val),
                };

                let buffer = match heap_obj.fields.get_mut("buffer") {
                    Some(JvmStackValue::String(s)) => s,
                    _ => return Err("StringBuffer instance has invalid 'buffer' field".into()),
                };
                buffer.push_str(&append_str);
                Ok(Some(args[0].clone()))
            }
            "toString" => {
                JVM::ensure_string_buffer_field(heap_obj);
                let buffer = match heap_obj.fields.get("buffer") {
                    Some(JvmStackValue::String(s)) => s,
                    _ => return Err("StringBuffer instance has invalid 'buffer' field".into()),
                };

                Ok(Some(JvmStackValue::String(buffer.clone())))
            }
            "<init>" => {
                let initial_value = match args.get(1) {
                    Some(JvmStackValue::String(s)) => s.clone(),
                    Some(JvmStackValue::Null) | None => String::new(),
                    Some(value) => format!("{:?}", value),
                };
                heap_obj
                    .fields
                    .insert("buffer".to_string(), JvmStackValue::String(initial_value));
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

    fn ensure_string_buffer_field(heap_obj: &mut JvmObject) {
        if !matches!(
            heap_obj.fields.get("buffer"),
            Some(JvmStackValue::String(_))
        ) {
            heap_obj
                .fields
                .insert("buffer".to_string(), JvmStackValue::String(String::new()));
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
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        if method == "<init>" {
            let parsed_string = match descriptor {
                "()V" => "".to_string(),
                "(Ljava/lang/String;)V" => {
                    let other = args.first().ok_or("String.<init>(String): missing arg")?;
                    JVM::char_sequence_to_string(other, jvm)
                }
                "([B)V" => {
                    let bytes_ref = args.first().ok_or("String.<init>(byte[]): missing arg")?;
                    let bytes = JVM::extract_byte_array(bytes_ref, jvm)?;
                    String::from_utf8_lossy(&bytes).into_owned()
                }
                "([BII)V" => {
                    let bytes_ref = args.first().ok_or("String.<init>(byte[], int, int): missing arg 1")?;
                    let offset = match args.get(1) {
                        Some(JvmStackValue::Int(o)) => *o as usize,
                        _ => return Err("String.<init>(byte[], int, int): invalid/missing offset".into()),
                    };
                    let length = match args.get(2) {
                        Some(JvmStackValue::Int(l)) => *l as usize,
                        _ => return Err("String.<init>(byte[], int, int): invalid/missing length".into()),
                    };
                    let bytes = JVM::extract_byte_array(bytes_ref, jvm)?;
                    if offset + length > bytes.len() {
                        return Err(format!("StringIndexOutOfBoundsException: offset {}, length {}, bytes len {}", offset, length, bytes.len()));
                    }
                    String::from_utf8_lossy(&bytes[offset..offset+length]).into_owned()
                }
                "([BIILjava/lang/String;)V" => {
                    let bytes_ref = args.first().ok_or("String.<init>(byte[], int, int, String): missing arg 1")?;
                    let offset = match args.get(1) {
                        Some(JvmStackValue::Int(o)) => *o as usize,
                        _ => return Err("String.<init>(byte[], int, int, String): invalid/missing offset".into()),
                    };
                    let length = match args.get(2) {
                        Some(JvmStackValue::Int(l)) => *l as usize,
                        _ => return Err("String.<init>(byte[], int, int, String): invalid/missing length".into()),
                    };
                    let charset_ref = args.get(3).ok_or("String.<init>(byte[], int, int, String): missing charset")?;
                    let charset = JVM::char_sequence_to_string(charset_ref, jvm).to_ascii_uppercase();

                    let bytes = JVM::extract_byte_array(bytes_ref, jvm)?;
                    if offset + length > bytes.len() {
                        return Err(format!("StringIndexOutOfBoundsException: offset {}, length {}, bytes len {}", offset, length, bytes.len()));
                    }
                    let sub_bytes = &bytes[offset..offset+length];

                    JVM::decode_bytes_with_charset(sub_bytes, &charset)?
                }
                "([C)V" => {
                    let chars_ref = args.first().ok_or("String.<init>(char[]): missing arg")?;
                    let chars = JVM::extract_char_array(chars_ref, jvm)?;
                    chars.into_iter().collect::<String>()
                }
                "([CII)V" => {
                    let chars_ref = args.first().ok_or("String.<init>(char[], int, int): missing arg 1")?;
                    let offset = match args.get(1) {
                        Some(JvmStackValue::Int(o)) => *o as usize,
                        _ => return Err("String.<init>(char[], int, int): invalid/missing offset".into()),
                    };
                    let length = match args.get(2) {
                        Some(JvmStackValue::Int(l)) => *l as usize,
                        _ => return Err("String.<init>(char[], int, int): invalid/missing length".into()),
                    };
                    let chars = JVM::extract_char_array(chars_ref, jvm)?;
                    if offset + length > chars.len() {
                        return Err(format!("StringIndexOutOfBoundsException: offset {}, length {}, chars len {}", offset, length, chars.len()));
                    }
                    chars[offset..offset+length].iter().collect::<String>()
                }
                "([Ljava/lang/String;)V" => {
                    "".to_string()
                }
                _ => return Err(format!("Unsupported String constructor: {}", descriptor)),
            };

            if let JvmStackValue::ObjectRef(id) = objectref {
                let mut state = jvm.state.lock();
                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(id as usize) {
                    obj.fields.insert("buffer".to_string(), JvmStackValue::String(parsed_string));
                    return Ok(None);
                } else {
                    return Err(format!("String.<init>: object ref {} is not a heap instance", id));
                }
            } else {
                return Err("String.<init>: expected ObjectRef for 'this'".into());
            }
        }

        let string = match &objectref {
            JvmStackValue::String(value) => value.clone(),
            JvmStackValue::ObjectRef(id) => {
                let state = jvm.state.lock();
                match state.heap.get(*id as usize) {
                    Some(HeapObject::Instance(obj)) => {
                        if let Some(JvmStackValue::String(buffer)) = obj.fields.get("buffer") {
                            buffer.clone()
                        } else {
                            "".to_string()
                        }
                    }
                    _ => return Err(format!("String: expected string object instance, found {:?}", objectref)),
                }
            }
            JvmStackValue::Null => return Err("String: NullPointerException".into()),
            value => return Err(format!("String: expected string object, found {:?}", value)),
        };

        match (method, descriptor) {
            ("getClass", "()Ljava/lang/Class;") => Ok(Some(JVM::allocate_class_object(
                jvm,
                "java/lang/String".to_string(),
            ))),
            ("toString", "()Ljava/lang/String;") => Ok(Some(JvmStackValue::String(string))),
            ("equals", "(Ljava/lang/Object;)Z") => {
                let is_equal =
                    matches!(args.first(), Some(JvmStackValue::String(other)) if other == &string);
                Ok(Some(JvmStackValue::Int(if is_equal { 1 } else { 0 })))
            }
            ("hashCode", "()I") => Ok(Some(JvmStackValue::Int(JVM::java_string_hash(&string)))),
            ("getBytes", "()[B") => Ok(Some(JVM::allocate_byte_array(jvm, string.as_bytes()))),
            ("getBytes", "(Ljava/lang/String;)[B") => {
                let charset = match args.first() {
                    Some(JvmStackValue::String(charset)) => charset.to_ascii_uppercase(),
                    Some(JvmStackValue::Null) | None => "UTF-8".to_string(),
                    value => return Err(format!("String.getBytes: invalid charset {:?}", value)),
                };

                let bytes = match charset.as_str() {
                    "UTF-8" | "UTF8" => string.as_bytes().to_vec(),
                    "US-ASCII" | "ASCII" => string
                        .chars()
                        .map(|ch| if ch as u32 <= 0x7f { ch as u8 } else { b'?' })
                        .collect(),
                    "ISO-8859-1" | "ISO8859-1" | "ISO_8859_1" | "ISO8859_1" => string
                        .chars()
                        .map(|ch| if ch as u32 <= 0xff { ch as u8 } else { b'?' })
                        .collect(),
                    "UTF-16BE" | "UNICODEBIGUNMARKED" => string
                        .encode_utf16()
                        .flat_map(|unit| unit.to_be_bytes())
                        .collect(),
                    "UTF-16LE" | "UNICODELITTLEUNMARKED" => string
                        .encode_utf16()
                        .flat_map(|unit| unit.to_le_bytes())
                        .collect(),
                    _ => string.as_bytes().to_vec(),
                };

                Ok(Some(JVM::allocate_byte_array(jvm, &bytes)))
            }
            ("compareTo", "(Ljava/lang/String;)I") | ("compareTo", "(Ljava/lang/Object;)I") => {
                let Some(JvmStackValue::String(other)) = args.first() else {
                    return Err(format!("String.compareTo: invalid arg {:?}", args.first()));
                };

                let left: Vec<u16> = string.encode_utf16().collect();
                let right: Vec<u16> = other.encode_utf16().collect();

                for (a, b) in left.iter().zip(&right) {
                    if a != b {
                        return Ok(Some(JvmStackValue::Int(*a as i32 - *b as i32)));
                    }
                }

                Ok(Some(JvmStackValue::Int(
                    left.len() as i32 - right.len() as i32,
                )))
            }
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
            ("subSequence", "(II)Ljava/lang/CharSequence;") => {
                let (Some(JvmStackValue::Int(begin)), Some(JvmStackValue::Int(end))) =
                    (args.first(), args.get(1))
                else {
                    return Err(format!("String.subSequence(II): invalid args {:?}", args));
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
            ("toUpperCase", "()Ljava/lang/String;") => {
                Ok(Some(JvmStackValue::String(string.to_uppercase())))
            }
            ("toLowerCase", "()Ljava/lang/String;") => {
                Ok(Some(JvmStackValue::String(string.to_lowercase())))
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
            ("startsWith", "(Ljava/lang/String;)Z") => {
                let Some(JvmStackValue::String(prefix)) = args.first() else {
                    return Err(format!("String.startsWith: invalid arg {:?}", args.first()));
                };

                Ok(Some(JvmStackValue::Int(if string.starts_with(prefix) {
                    1
                } else {
                    0
                })))
            }
            ("contains", "(Ljava/lang/CharSequence;)Z") => {
                let Some(JvmStackValue::String(seq)) = args.first() else {
                    return Err(format!("String.contains: invalid arg {:?}", args.first()));
                };

                Ok(Some(JvmStackValue::Int(if string.contains(seq) {
                    1
                } else {
                    0
                })))
            }
            ("isEmpty", "()Z") => Ok(Some(JvmStackValue::Int(if string.is_empty() {
                1
            } else {
                0
            }))),
            ("replace", "(CC)Ljava/lang/String;") => {
                let (Some(JvmStackValue::Int(old_char)), Some(JvmStackValue::Int(new_char))) =
                    (args.first(), args.get(1))
                else {
                    return Err(format!("String.replace: invalid args {:?}", args));
                };

                let old_char = char::from_u32(*old_char as u32).unwrap_or('\0');
                let new_char = char::from_u32(*new_char as u32).unwrap_or('\0');

                Ok(Some(JvmStackValue::String(
                    string.replace(old_char, String::from(new_char).as_str()),
                )))
            }
            ("indexOf", "(Ljava/lang/String;)I") => {
                let Some(JvmStackValue::String(substr)) = args.first() else {
                    return Err(format!(
                        "String.indexOf(String): invalid arg {:?}",
                        args.first()
                    ));
                };

                Ok(Some(JvmStackValue::Int(
                    string
                        .find(substr)
                        .map_or(-1, |idx| string[..idx].chars().count() as i32),
                )))
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
        JVM::execute_static_method_inner(class_name, method_name, descriptor, args, jvm, stack)
            .map_err(|e| JVM::append_static_method_context(e, class_name, method_name, descriptor))
    }

    fn execute_static_method_inner(
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
            if class_name == player::MANAGER_CLASS_NAME {
                let return_value =
                    player::handle_manager_static_method(method_name, descriptor, args, jvm)?;

                if let Some(val) = return_value {
                    stack.push(val);
                }

                return Ok(());
            }
            if class_name == rms::RECORD_STORE_CLASS_NAME {
                let return_value = rms::handle_static_method(method_name, descriptor, args, jvm)?;

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
        } else if class_name == "java/lang/Integer" {
            let return_value = JVM::handle_integer_static_fns(method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/lang/Thread" && method_name == "sleep" {
            if let Some(JvmStackValue::Long(ms)) = args.first() {
                jvm.sleep_emulated(Duration::from_millis(*ms as u64));
            }
            return Ok(());
        } else if class_name == "java/lang/System" {
            let return_value = JVM::handle_system_static_fns(method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/util/Calendar" {
            let return_value = JVM::handle_calendar_static_fns(method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                stack.push(val);
            }

            return Ok(());
        } else if class_name == "java/util/TimeZone" {
            let return_value = JVM::handle_timezone_static_fns(method_name, descriptor, args, jvm)?;

            if let Some(val) = return_value {
                stack.push(val);
            }

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
            &code_attr.exception_table,
            &mut locals,
            jvm,
        )?;

        if let Some(val) = return_value {
            stack.push(val);
        }

        Ok(())
    }

    pub fn paint(&self) -> Result<(), String> {
        if self.is_paused() || self.is_shutdown() {
            return Ok(());
        }

        static IS_PAINTING: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if IS_PAINTING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        let res = game_canvas::paint(&self);
        IS_PAINTING.store(false, std::sync::atomic::Ordering::SeqCst);
        res
    }

    fn allocate_calendar_object(jvm: &JVM, time: i64, timezone: String) -> JvmStackValue {
        let mut fields = HashMap::new();
        fields.insert("time".to_string(), JvmStackValue::Long(time));
        fields.insert("timezone".to_string(), JvmStackValue::String(timezone));

        let mut state = jvm.state.lock();
        JvmStackValue::ObjectRef(JVM::allocate_internal(
            &mut state,
            "java/util/Calendar".to_string(),
            fields,
        ))
    }

    fn allocate_date_object(jvm: &JVM, time: i64) -> JvmStackValue {
        let mut fields = HashMap::new();
        fields.insert("time".to_string(), JvmStackValue::Long(time));

        let mut state = jvm.state.lock();
        JvmStackValue::ObjectRef(JVM::allocate_internal(
            &mut state,
            "java/util/Date".to_string(),
            fields,
        ))
    }

    fn allocate_timezone_object(jvm: &JVM, id: String, raw_offset: i32) -> JvmStackValue {
        let mut fields = HashMap::new();
        fields.insert("id".to_string(), JvmStackValue::String(id));
        fields.insert("rawOffset".to_string(), JvmStackValue::Int(raw_offset));

        let mut state = jvm.state.lock();
        JvmStackValue::ObjectRef(JVM::allocate_internal(
            &mut state,
            "java/util/TimeZone".to_string(),
            fields,
        ))
    }

    fn object_ref_id(value: &JvmStackValue, context: &str) -> Result<usize, String> {
        match value {
            JvmStackValue::ObjectRef(id) => Ok(*id as usize),
            JvmStackValue::Null => Err("java.lang.NullPointerException".into()),
            value => Err(format!(
                "{}: expected object reference, found {:?}",
                context, value
            )),
        }
    }

    fn object_long_field(
        jvm: &JVM,
        object_ref: &JvmStackValue,
        field: &str,
    ) -> Result<i64, String> {
        let id = JVM::object_ref_id(object_ref, field)?;
        let state = jvm.state.lock();
        match state.heap.get(id) {
            Some(HeapObject::Instance(obj)) => match obj.fields.get(field) {
                Some(JvmStackValue::Long(value)) => Ok(*value),
                Some(value) => Err(format!("{}: expected long field, found {:?}", field, value)),
                None => Ok(0),
            },
            Some(_) => Err(format!("{}: object reference is not an instance", field)),
            None => Err(format!("{}: invalid object reference {}", field, id)),
        }
    }

    fn set_object_long_field(
        jvm: &JVM,
        object_ref: &JvmStackValue,
        field: &str,
        value: i64,
    ) -> Result<(), String> {
        let id = JVM::object_ref_id(object_ref, field)?;
        let mut state = jvm.state.lock();
        match state.heap.get_mut(id) {
            Some(HeapObject::Instance(obj)) => {
                obj.fields
                    .insert(field.to_string(), JvmStackValue::Long(value));
                Ok(())
            }
            Some(_) => Err(format!("{}: object reference is not an instance", field)),
            None => Err(format!("{}: invalid object reference {}", field, id)),
        }
    }

    fn timezone_id_from_value(value: Option<&JvmStackValue>, jvm: &JVM) -> Result<String, String> {
        match value {
            Some(JvmStackValue::ObjectRef(id)) => {
                let state = jvm.state.lock();
                match state.heap.get(*id as usize) {
                    Some(HeapObject::Instance(obj)) => match obj.fields.get("id") {
                        Some(JvmStackValue::String(value)) => Ok(value.clone()),
                        _ => Ok("GMT".to_string()),
                    },
                    _ => Ok("GMT".to_string()),
                }
            }
            Some(JvmStackValue::String(value)) => Ok(value.clone()),
            Some(JvmStackValue::Null) | None => Ok("GMT".to_string()),
            Some(value) => Err(format!("TimeZone: invalid value {:?}", value)),
        }
    }

    fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
    }

    fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if JVM::is_leap_year(year) => 29,
            2 => 28,
            _ => 30,
        }
    }

    fn day_of_year(year: i32, month: u32, day: u32) -> u32 {
        let mut total = day;
        for m in 1..month {
            total += JVM::days_in_month(year, m);
        }
        total
    }

    fn civil_from_days(days: i64) -> (i32, u32, u32) {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        let year = y + if month <= 2 { 1 } else { 0 };

        (year as i32, month as u32, day as u32)
    }

    fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
        let mut y = year as i64;
        let m = month as i64;
        let d = day as i64;
        y -= if m <= 2 { 1 } else { 0 };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = m + if m > 2 { -3 } else { 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    fn calendar_parts_from_millis(time: i64) -> CalendarParts {
        let days = time.div_euclid(86_400_000);
        let millis_of_day = time.rem_euclid(86_400_000);
        let (year, month, day) = JVM::civil_from_days(days);
        let hour = (millis_of_day / 3_600_000) as u32;
        let minute = ((millis_of_day / 60_000) % 60) as u32;
        let second = ((millis_of_day / 1_000) % 60) as u32;
        let millis = (millis_of_day % 1_000) as u32;
        let day_of_year = JVM::day_of_year(year, month, day);
        let day_of_week = (days + 4).rem_euclid(7) as u32 + 1;

        CalendarParts {
            year,
            month,
            day,
            hour,
            minute,
            second,
            millis,
            day_of_year,
            day_of_week,
        }
    }

    fn calendar_millis_from_parts(parts: CalendarParts) -> i64 {
        let month = parts.month.clamp(1, 12);
        let day = parts.day.clamp(1, JVM::days_in_month(parts.year, month));
        let days = JVM::days_from_civil(parts.year, month, day);
        days * 86_400_000
            + parts.hour.min(23) as i64 * 3_600_000
            + parts.minute.min(59) as i64 * 60_000
            + parts.second.min(59) as i64 * 1_000
            + parts.millis.min(999) as i64
    }

    fn calendar_get_field(time: i64, field: i32) -> i32 {
        let parts = JVM::calendar_parts_from_millis(time);
        match field {
            0 => 1,
            1 => parts.year,
            2 => parts.month as i32 - 1,
            3 => ((parts.day_of_year + 6) / 7) as i32,
            4 => ((parts.day + 6) / 7) as i32,
            5 => parts.day as i32,
            6 => parts.day_of_year as i32,
            7 => parts.day_of_week as i32,
            8 => ((parts.day + 6) / 7) as i32,
            9 => (parts.hour / 12) as i32,
            10 => (parts.hour % 12) as i32,
            11 => parts.hour as i32,
            12 => parts.minute as i32,
            13 => parts.second as i32,
            14 => parts.millis as i32,
            15 | 16 => 0,
            _ => 0,
        }
    }

    fn calendar_set_field(time: i64, field: i32, value: i32) -> i64 {
        let mut parts = JVM::calendar_parts_from_millis(time);
        match field {
            1 => parts.year = value,
            2 => {
                let total_months = parts.year * 12 + value;
                parts.year = total_months.div_euclid(12);
                parts.month = total_months.rem_euclid(12) as u32 + 1;
            }
            5 => parts.day = value.max(1) as u32,
            11 => parts.hour = value.max(0) as u32,
            10 => parts.hour = (parts.hour / 12) * 12 + value.clamp(0, 11) as u32,
            9 => parts.hour = value.clamp(0, 1) as u32 * 12 + parts.hour % 12,
            12 => parts.minute = value.max(0) as u32,
            13 => parts.second = value.max(0) as u32,
            14 => parts.millis = value.max(0) as u32,
            _ => {}
        }

        JVM::calendar_millis_from_parts(parts)
    }

    fn calendar_add_field(time: i64, field: i32, amount: i32) -> i64 {
        match field {
            1 => {
                let mut parts = JVM::calendar_parts_from_millis(time);
                parts.year += amount;
                JVM::calendar_millis_from_parts(parts)
            }
            2 => {
                let parts = JVM::calendar_parts_from_millis(time);
                let month_zero = parts.month as i32 - 1;
                let total_months = parts.year * 12 + month_zero + amount;
                let mut updated = parts;
                updated.year = total_months.div_euclid(12);
                updated.month = total_months.rem_euclid(12) as u32 + 1;
                JVM::calendar_millis_from_parts(updated)
            }
            3 | 4 => time.saturating_add(amount as i64 * 7 * 86_400_000),
            5 | 6 | 7 => time.saturating_add(amount as i64 * 86_400_000),
            10 | 11 => time.saturating_add(amount as i64 * 3_600_000),
            12 => time.saturating_add(amount as i64 * 60_000),
            13 => time.saturating_add(amount as i64 * 1_000),
            14 => time.saturating_add(amount as i64),
            _ => time,
        }
    }

    fn handle_calendar_static_fns(
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("getInstance", "()Ljava/util/Calendar;") => Ok(Some(JVM::allocate_calendar_object(
                jvm,
                jvm.current_time_millis(),
                "GMT".to_string(),
            ))),
            ("getInstance", "(Ljava/util/TimeZone;)Ljava/util/Calendar;") => {
                let timezone = JVM::timezone_id_from_value(args.first(), jvm)?;
                Ok(Some(JVM::allocate_calendar_object(
                    jvm,
                    jvm.current_time_millis(),
                    timezone,
                )))
            }
            _ => Err(format!(
                "Unsupported Calendar static method: {}{}",
                method, descriptor
            )),
        }
    }

    fn handle_calendar_fns(
        objectref: &JvmStackValue,
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("<init>", "()V") => {
                JVM::set_object_long_field(jvm, objectref, "time", jvm.current_time_millis())?;
                Ok(None)
            }
            ("get", "(I)I") => {
                let field = match args.first() {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.get: invalid field {:?}", value)),
                };
                let time = JVM::object_long_field(jvm, objectref, "time")?;
                Ok(Some(JvmStackValue::Int(JVM::calendar_get_field(
                    time, field,
                ))))
            }
            ("set", "(II)V") => {
                let field = match args.first() {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.set(II): invalid field {:?}", value)),
                };
                let value = match args.get(1) {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.set(II): invalid value {:?}", value)),
                };
                let time = JVM::object_long_field(jvm, objectref, "time")?;
                JVM::set_object_long_field(
                    jvm,
                    objectref,
                    "time",
                    JVM::calendar_set_field(time, field, value),
                )?;
                Ok(None)
            }
            ("set", "(III)V") | ("set", "(IIIII)V") | ("set", "(IIIIII)V") => {
                let year = match args.first() {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.set: invalid year {:?}", value)),
                };
                let month = match args.get(1) {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.set: invalid month {:?}", value)),
                };
                let day = match args.get(2) {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.set: invalid day {:?}", value)),
                };
                let hour = match args.get(3) {
                    Some(JvmStackValue::Int(value)) => *value,
                    _ => 0,
                };
                let minute = match args.get(4) {
                    Some(JvmStackValue::Int(value)) => *value,
                    _ => 0,
                };
                let second = match args.get(5) {
                    Some(JvmStackValue::Int(value)) => *value,
                    _ => 0,
                };
                let parts = CalendarParts {
                    year,
                    month: (month + 1).max(1) as u32,
                    day: day.max(1) as u32,
                    hour: hour.max(0) as u32,
                    minute: minute.max(0) as u32,
                    second: second.max(0) as u32,
                    millis: 0,
                    day_of_year: 0,
                    day_of_week: 0,
                };
                JVM::set_object_long_field(
                    jvm,
                    objectref,
                    "time",
                    JVM::calendar_millis_from_parts(parts),
                )?;
                Ok(None)
            }
            ("add", "(II)V") | ("roll", "(II)V") => {
                let field = match args.first() {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.add/roll: invalid field {:?}", value)),
                };
                let amount = match args.get(1) {
                    Some(JvmStackValue::Int(value)) => *value,
                    value => return Err(format!("Calendar.add/roll: invalid amount {:?}", value)),
                };
                let time = JVM::object_long_field(jvm, objectref, "time")?;
                JVM::set_object_long_field(
                    jvm,
                    objectref,
                    "time",
                    JVM::calendar_add_field(time, field, amount),
                )?;
                Ok(None)
            }
            ("getTimeInMillis", "()J") => Ok(Some(JvmStackValue::Long(JVM::object_long_field(
                jvm, objectref, "time",
            )?))),
            ("setTimeInMillis", "(J)V") => {
                let time = match args.first() {
                    Some(JvmStackValue::Long(value)) => *value,
                    value => {
                        return Err(format!("Calendar.setTimeInMillis: invalid arg {:?}", value));
                    }
                };
                JVM::set_object_long_field(jvm, objectref, "time", time)?;
                Ok(None)
            }
            ("getTime", "()Ljava/util/Date;") => Ok(Some(JVM::allocate_date_object(
                jvm,
                JVM::object_long_field(jvm, objectref, "time")?,
            ))),
            ("setTime", "(Ljava/util/Date;)V") => {
                let date = args
                    .first()
                    .ok_or_else(|| "Calendar.setTime: missing Date argument".to_string())?;
                let time = JVM::object_long_field(jvm, date, "time")?;
                JVM::set_object_long_field(jvm, objectref, "time", time)?;
                Ok(None)
            }
            ("getTimeZone", "()Ljava/util/TimeZone;") => {
                let timezone = JVM::timezone_id_from_value(Some(objectref), jvm)
                    .unwrap_or_else(|_| "GMT".to_string());
                Ok(Some(JVM::allocate_timezone_object(jvm, timezone, 0)))
            }
            ("setTimeZone", "(Ljava/util/TimeZone;)V") => {
                let timezone = JVM::timezone_id_from_value(args.first(), jvm)?;
                let id = JVM::object_ref_id(objectref, "Calendar.setTimeZone")?;
                let mut state = jvm.state.lock();
                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(id) {
                    obj.fields
                        .insert("timezone".to_string(), JvmStackValue::String(timezone));
                }
                Ok(None)
            }
            ("clear", "()V") => {
                JVM::set_object_long_field(jvm, objectref, "time", 0)?;
                Ok(None)
            }
            ("clear", "(I)V") => Ok(None),
            ("isSet", "(I)Z") => Ok(Some(JvmStackValue::Int(1))),
            ("before", "(Ljava/lang/Object;)Z") | ("after", "(Ljava/lang/Object;)Z") => {
                let other = args
                    .first()
                    .ok_or_else(|| "Calendar.before/after: missing argument".to_string())?;
                let left = JVM::object_long_field(jvm, objectref, "time")?;
                let right = JVM::object_long_field(jvm, other, "time")?;
                let result = if method == "before" {
                    left < right
                } else {
                    left > right
                };
                Ok(Some(JvmStackValue::Int(if result { 1 } else { 0 })))
            }
            ("equals", "(Ljava/lang/Object;)Z") => {
                let Some(other) = args.first() else {
                    return Ok(Some(JvmStackValue::Int(0)));
                };
                let left = JVM::object_long_field(jvm, objectref, "time")?;
                let right = JVM::object_long_field(jvm, other, "time").unwrap_or(i64::MIN);
                Ok(Some(JvmStackValue::Int(if left == right { 1 } else { 0 })))
            }
            ("toString", "()Ljava/lang/String;") => {
                let time = JVM::object_long_field(jvm, objectref, "time")?;
                Ok(Some(JvmStackValue::String(format!("Calendar({})", time))))
            }
            _ => Err(format!(
                "Unsupported Calendar method: {}{}",
                method, descriptor
            )),
        }
    }

    fn handle_date_fns(
        objectref: &JvmStackValue,
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("<init>", "()V") => {
                JVM::set_object_long_field(jvm, objectref, "time", jvm.current_time_millis())?;
                Ok(None)
            }
            ("<init>", "(J)V") => {
                let time = match args.first() {
                    Some(JvmStackValue::Long(value)) => *value,
                    value => return Err(format!("Date.<init>(J): invalid arg {:?}", value)),
                };
                JVM::set_object_long_field(jvm, objectref, "time", time)?;
                Ok(None)
            }
            ("getTime", "()J") => Ok(Some(JvmStackValue::Long(JVM::object_long_field(
                jvm, objectref, "time",
            )?))),
            ("setTime", "(J)V") => {
                let time = match args.first() {
                    Some(JvmStackValue::Long(value)) => *value,
                    value => return Err(format!("Date.setTime: invalid arg {:?}", value)),
                };
                JVM::set_object_long_field(jvm, objectref, "time", time)?;
                Ok(None)
            }
            ("before", "(Ljava/util/Date;)Z") | ("after", "(Ljava/util/Date;)Z") => {
                let other = args
                    .first()
                    .ok_or_else(|| "Date.before/after: missing argument".to_string())?;
                let left = JVM::object_long_field(jvm, objectref, "time")?;
                let right = JVM::object_long_field(jvm, other, "time")?;
                let result = if method == "before" {
                    left < right
                } else {
                    left > right
                };
                Ok(Some(JvmStackValue::Int(if result { 1 } else { 0 })))
            }
            ("equals", "(Ljava/lang/Object;)Z") => {
                let Some(other) = args.first() else {
                    return Ok(Some(JvmStackValue::Int(0)));
                };
                let left = JVM::object_long_field(jvm, objectref, "time")?;
                let right = JVM::object_long_field(jvm, other, "time").unwrap_or(i64::MIN);
                Ok(Some(JvmStackValue::Int(if left == right { 1 } else { 0 })))
            }
            ("toString", "()Ljava/lang/String;") => {
                let time = JVM::object_long_field(jvm, objectref, "time")?;
                Ok(Some(JvmStackValue::String(format!("Date({})", time))))
            }
            _ => Err(format!("Unsupported Date method: {}{}", method, descriptor)),
        }
    }

    fn handle_timezone_static_fns(
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("getDefault", "()Ljava/util/TimeZone;") => Ok(Some(JVM::allocate_timezone_object(
                jvm,
                "GMT".to_string(),
                0,
            ))),
            ("getTimeZone", "(Ljava/lang/String;)Ljava/util/TimeZone;") => {
                let id = match args.first() {
                    Some(JvmStackValue::String(value)) => value.clone(),
                    Some(JvmStackValue::Null) | None => "GMT".to_string(),
                    value => return Err(format!("TimeZone.getTimeZone: invalid id {:?}", value)),
                };
                Ok(Some(JVM::allocate_timezone_object(jvm, id, 0)))
            }
            _ => Err(format!(
                "Unsupported TimeZone static method: {}{}",
                method, descriptor
            )),
        }
    }

    fn handle_timezone_fns(
        objectref: &JvmStackValue,
        method: &str,
        descriptor: &str,
        _args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        let id = JVM::object_ref_id(objectref, "TimeZone")?;
        match (method, descriptor) {
            ("getID", "()Ljava/lang/String;") => {
                let state = jvm.state.lock();
                let value = match state.heap.get(id) {
                    Some(HeapObject::Instance(obj)) => match obj.fields.get("id") {
                        Some(JvmStackValue::String(value)) => value.clone(),
                        _ => "GMT".to_string(),
                    },
                    _ => "GMT".to_string(),
                };
                Ok(Some(JvmStackValue::String(value)))
            }
            ("getRawOffset", "()I") => {
                let state = jvm.state.lock();
                let value = match state.heap.get(id) {
                    Some(HeapObject::Instance(obj)) => match obj.fields.get("rawOffset") {
                        Some(JvmStackValue::Int(value)) => *value,
                        _ => 0,
                    },
                    _ => 0,
                };
                Ok(Some(JvmStackValue::Int(value)))
            }
            ("useDaylightTime", "()Z") | ("inDaylightTime", "(Ljava/util/Date;)Z") => {
                Ok(Some(JvmStackValue::Int(0)))
            }
            ("toString", "()Ljava/lang/String;") => {
                Ok(Some(JvmStackValue::String("TimeZone(GMT)".to_string())))
            }
            _ => Err(format!(
                "Unsupported TimeZone method: {}{}",
                method, descriptor
            )),
        }
    }

    fn handle_system_static_fns(
        method: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        match (method, descriptor) {
            ("currentTimeMillis", "()J") => {
                Ok(Some(JvmStackValue::Long(jvm.current_time_millis())))
            }
            ("gc", "()V") | ("exit", "(I)V") => Ok(None),
            ("identityHashCode", "(Ljava/lang/Object;)I") => {
                let hash = match args.first() {
                    Some(JvmStackValue::Null) | None => 0,
                    Some(JvmStackValue::ObjectRef(id)) => *id as i32,
                    Some(JvmStackValue::String(value)) => JVM::java_string_hash(value),
                    Some(value) => {
                        return Err(format!(
                            "System.identityHashCode: expected reference, found {:?}",
                            value
                        ));
                    }
                };
                Ok(Some(JvmStackValue::Int(hash)))
            }
            ("getProperty", "(Ljava/lang/String;)Ljava/lang/String;") => {
                let key = match args.first() {
                    Some(JvmStackValue::String(value)) => value.as_str(),
                    Some(JvmStackValue::Null) => {
                        return Err("java.lang.NullPointerException".into());
                    }
                    value => {
                        return Err(format!("System.getProperty: invalid key {:?}", value));
                    }
                };

                let value = match key {
                    "microedition.configuration" => Some("CLDC-1.1"),
                    "microedition.profiles" => Some("MIDP-2.0"),
                    "microedition.platform" => Some("j2me-emulator"),
                    "microedition.encoding" => Some("UTF-8"),
                    "microedition.locale" => Some("en-US"),
                    "file.separator" => Some("/"),
                    "path.separator" => Some(":"),
                    "line.separator" => Some("\n"),
                    _ => None,
                };

                Ok(Some(match value {
                    Some(value) => JvmStackValue::String(value.to_string()),
                    None => JvmStackValue::Null,
                }))
            }
            ("arraycopy", "(Ljava/lang/Object;ILjava/lang/Object;II)V") => {
                JVM::system_arraycopy(args, jvm)?;
                Ok(None)
            }
            _ => Err(format!(
                "Unsupported System method: {}{}",
                method, descriptor
            )),
        }
    }

    fn system_arraycopy(args: &[JvmStackValue], jvm: &JVM) -> Result<(), String> {
        let src_ref = match args.first() {
            Some(JvmStackValue::ObjectRef(id)) => *id as usize,
            Some(JvmStackValue::Null) => return Err("java.lang.NullPointerException".into()),
            value => return Err(format!("System.arraycopy: invalid src {:?}", value)),
        };
        let src_pos = match args.get(1) {
            Some(JvmStackValue::Int(value)) => *value,
            value => return Err(format!("System.arraycopy: invalid srcPos {:?}", value)),
        };
        let dst_ref = match args.get(2) {
            Some(JvmStackValue::ObjectRef(id)) => *id as usize,
            Some(JvmStackValue::Null) => return Err("java.lang.NullPointerException".into()),
            value => return Err(format!("System.arraycopy: invalid dst {:?}", value)),
        };
        let dst_pos = match args.get(3) {
            Some(JvmStackValue::Int(value)) => *value,
            value => return Err(format!("System.arraycopy: invalid dstPos {:?}", value)),
        };
        let length = match args.get(4) {
            Some(JvmStackValue::Int(value)) => *value,
            value => return Err(format!("System.arraycopy: invalid length {:?}", value)),
        };

        if src_pos < 0 || dst_pos < 0 || length < 0 {
            return Err(format!(
                "java.lang.ArrayIndexOutOfBoundsException: srcPos {}, dstPos {}, length {}",
                src_pos, dst_pos, length
            ));
        }

        let src_pos = src_pos as usize;
        let dst_pos = dst_pos as usize;
        let length = length as usize;

        let mut state = jvm.state.lock();

        let copied_values = match state.heap.get(src_ref) {
            Some(HeapObject::Array { data, .. }) => {
                let src_end = src_pos.checked_add(length).ok_or_else(|| {
                    "java.lang.ArrayIndexOutOfBoundsException: source range overflow".to_string()
                })?;

                if src_end > data.len() {
                    return Err(format!(
                        "java.lang.ArrayIndexOutOfBoundsException: source range {}..{} out of bounds for length {}",
                        src_pos,
                        src_end,
                        data.len()
                    ));
                }

                data[src_pos..src_end].to_vec()
            }
            Some(_) => return Err("java.lang.ArrayStoreException: source is not an array".into()),
            None => {
                return Err(format!(
                    "System.arraycopy: invalid source reference {}",
                    src_ref
                ));
            }
        };

        match state.heap.get_mut(dst_ref) {
            Some(HeapObject::Array { data, .. }) => {
                let dst_end = dst_pos.checked_add(length).ok_or_else(|| {
                    "java.lang.ArrayIndexOutOfBoundsException: destination range overflow"
                        .to_string()
                })?;

                if dst_end > data.len() {
                    return Err(format!(
                        "java.lang.ArrayIndexOutOfBoundsException: destination range {}..{} out of bounds for length {}",
                        dst_pos,
                        dst_end,
                        data.len()
                    ));
                }

                for (slot, value) in data[dst_pos..dst_end]
                    .iter_mut()
                    .zip(copied_values.into_iter())
                {
                    *slot = value;
                }
            }
            Some(_) => {
                return Err("java.lang.ArrayStoreException: destination is not an array".into());
            }
            None => {
                return Err(format!(
                    "System.arraycopy: invalid destination reference {}",
                    dst_ref
                ));
            }
        }

        Ok(())
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
                    return Err(format!("Resource not found {}", resource_path).into()); // Resource not found, return null
                };

                let mut fields = HashMap::new();

                fields.insert(
                    "jvm_res".to_string(),
                    JvmStackValue::String(resource_path.clone()),
                );
                fields.insert("jvm_pos".to_string(), JvmStackValue::Int(0));
                fields.insert("jvm_mark".to_string(), JvmStackValue::Int(0));

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

    fn extract_byte_array(val: &JvmStackValue, jvm: &JVM) -> Result<Vec<u8>, String> {
        match val {
            JvmStackValue::ObjectRef(id) => {
                let state = jvm.state.lock();
                match state.heap.get(*id as usize) {
                    Some(HeapObject::Array { data, .. }) => {
                        JVM::byte_array_values_to_bytes(data, "extract_byte_array")
                    }
                    _ => Err("expected byte array object".into()),
                }
            }
            JvmStackValue::Null => Err("NullPointerException: null byte array".into()),
            _ => Err("expected object reference for byte array".into()),
        }
    }

    fn extract_char_array(val: &JvmStackValue, jvm: &JVM) -> Result<Vec<char>, String> {
        match val {
            JvmStackValue::ObjectRef(id) => {
                let state = jvm.state.lock();
                match state.heap.get(*id as usize) {
                    Some(HeapObject::Array { data, .. }) => {
                        let mut chars = Vec::with_capacity(data.len());
                        for item in data {
                            match item {
                                JvmStackValue::Int(c) => {
                                    let ch = char::from_u32(*c as u32)
                                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                                    chars.push(ch);
                                }
                                _ => return Err(format!("expected char in array, found {:?}", item)),
                            }
                        }
                        Ok(chars)
                    }
                    _ => Err("expected char array object".into()),
                }
            }
            JvmStackValue::Null => Err("NullPointerException: null char array".into()),
            _ => Err("expected object reference for char array".into()),
        }
    }

    fn decode_bytes_with_charset(bytes: &[u8], charset: &str) -> Result<String, String> {
        match charset {
            "UTF-8" | "UTF8" => Ok(String::from_utf8_lossy(bytes).into_owned()),
            "US-ASCII" | "ASCII" => {
                let s: String = bytes
                    .iter()
                    .map(|&b| if b <= 0x7f { b as char } else { '?' })
                    .collect();
                Ok(s)
            }
            "ISO-8859-1" | "ISO8859-1" | "ISO_8859_1" | "ISO8859_1" => {
                let s: String = bytes.iter().map(|&b| b as char).collect();
                Ok(s)
            }
            "UTF-16BE" | "UNICODEBIGUNMARKED" => {
                let mut u16_units = Vec::with_capacity(bytes.len() / 2);
                for chunk in bytes.chunks_exact(2) {
                    u16_units.push(u16::from_be_bytes([chunk[0], chunk[1]]));
                }
                Ok(String::from_utf16_lossy(&u16_units))
            }
            "UTF-16LE" | "UNICODELITTLEUNMARKED" => {
                let mut u16_units = Vec::with_capacity(bytes.len() / 2);
                for chunk in bytes.chunks_exact(2) {
                    u16_units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
                }
                Ok(String::from_utf16_lossy(&u16_units))
            }
            _ => {
                // Default to UTF-8
                Ok(String::from_utf8_lossy(bytes).into_owned())
            }
        }
    }

    fn byte_array_values_to_bytes(
        values: &[JvmStackValue],
        context: &str,
    ) -> Result<Vec<u8>, String> {
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

    fn bytes_to_jvm_vector(bytes: &[u8]) -> JvmStackValue {
        JvmStackValue::Vector(
            bytes
                .iter()
                .map(|byte| JvmStackValue::Int(*byte as i32))
                .collect(),
        )
    }

    fn handle_byte_array_input_stream_fns(
        method_name: &str,
        descriptor: &str,
        args: &[JvmStackValue],
        jvm: &JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        let mut state = jvm.state.lock();

        let stream_ref =
            match args.get(0) {
                Some(JvmStackValue::ObjectRef(r)) => *r as usize,
                _ => return Err(
                    "Expected object reference as first argument to ByteArrayInputStream method"
                        .into(),
                ),
            };

        match (method_name, descriptor) {
            ("<init>", "([B)V") => {
                let buffer_ref = match args.get(1) {
                    Some(JvmStackValue::ObjectRef(r)) => *r as usize,
                    Some(JvmStackValue::Null) => {
                        return Err("java.lang.NullPointerException".into());
                    }
                    Some(value) => {
                        return Err(format!(
                            "ByteArrayInputStream.<init>([B): expected byte array, found {:?}",
                            value
                        ));
                    }
                    None => return Err("ByteArrayInputStream.<init>([B): missing buffer".into()),
                };

                let data = match state.heap.get(buffer_ref) {
                    Some(HeapObject::Array { data, .. }) => {
                        JVM::byte_array_values_to_bytes(data, "ByteArrayInputStream.<init>([B)")?
                    }
                    Some(_) => return Err("ByteArrayInputStream.<init>([B): expected array".into()),
                    None => {
                        return Err(format!(
                            "ByteArrayInputStream.<init>([B): invalid buffer reference {}",
                            buffer_ref
                        ));
                    }
                };

                let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) else {
                    return Err("Expected instance for ByteArrayInputStream object".into());
                };

                obj.fields
                    .insert("jvm_data".to_string(), JVM::bytes_to_jvm_vector(&data));
                obj.fields
                    .insert("jvm_pos".to_string(), JvmStackValue::Int(0));
                obj.fields
                    .insert("jvm_mark".to_string(), JvmStackValue::Int(0));

                return Ok(None);
            }
            ("<init>", "([BII)V") => {
                let buffer_ref = match args.get(1) {
                    Some(JvmStackValue::ObjectRef(r)) => *r as usize,
                    Some(JvmStackValue::Null) => {
                        return Err("java.lang.NullPointerException".into());
                    }
                    Some(value) => {
                        return Err(format!(
                            "ByteArrayInputStream.<init>([BII): expected byte array, found {:?}",
                            value
                        ));
                    }
                    None => return Err("ByteArrayInputStream.<init>([BII): missing buffer".into()),
                };
                let offset = match args.get(2) {
                    Some(JvmStackValue::Int(value)) => *value,
                    Some(value) => {
                        return Err(format!(
                            "ByteArrayInputStream.<init>([BII): invalid offset {:?}",
                            value
                        ));
                    }
                    None => return Err("ByteArrayInputStream.<init>([BII): missing offset".into()),
                };
                let len = match args.get(3) {
                    Some(JvmStackValue::Int(value)) => *value,
                    Some(value) => {
                        return Err(format!(
                            "ByteArrayInputStream.<init>([BII): invalid length {:?}",
                            value
                        ));
                    }
                    None => return Err("ByteArrayInputStream.<init>([BII): missing length".into()),
                };

                if offset < 0 || len < 0 {
                    return Err(format!(
                        "java.lang.IndexOutOfBoundsException: offset {}, length {}",
                        offset, len
                    ));
                }

                let source = match state.heap.get(buffer_ref) {
                    Some(HeapObject::Array { data, .. }) => {
                        JVM::byte_array_values_to_bytes(data, "ByteArrayInputStream.<init>([BII)")?
                    }
                    Some(_) => {
                        return Err("ByteArrayInputStream.<init>([BII): expected array".into());
                    }
                    None => {
                        return Err(format!(
                            "ByteArrayInputStream.<init>([BII): invalid buffer reference {}",
                            buffer_ref
                        ));
                    }
                };

                let offset = offset as usize;
                if offset > source.len() {
                    return Err(format!(
                        "java.lang.IndexOutOfBoundsException: offset {}, buffer length {}",
                        offset,
                        source.len()
                    ));
                }

                let len = (len as usize).min(source.len().saturating_sub(offset));
                let data = source[offset..offset + len].to_vec();

                let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) else {
                    return Err("Expected instance for ByteArrayInputStream object".into());
                };

                obj.fields
                    .insert("jvm_data".to_string(), JVM::bytes_to_jvm_vector(&data));
                obj.fields
                    .insert("jvm_pos".to_string(), JvmStackValue::Int(0));
                obj.fields
                    .insert("jvm_mark".to_string(), JvmStackValue::Int(0));

                return Ok(None);
            }
            _ => {}
        }

        let (data, pos) = if let Some(HeapObject::Instance(obj)) = state.heap.get(stream_ref) {
            let pos = match obj.fields.get("jvm_pos") {
                Some(JvmStackValue::Int(pos)) if *pos >= 0 => *pos as usize,
                Some(JvmStackValue::Int(_)) | None => 0,
                Some(value) => {
                    return Err(format!(
                        "ByteArrayInputStream invalid 'jvm_pos' field: {:?}",
                        value
                    ));
                }
            };

            let data = match obj.fields.get("jvm_data") {
                Some(JvmStackValue::Vector(values)) => {
                    JVM::byte_array_values_to_bytes(values, "ByteArrayInputStream.jvm_data")?
                }
                Some(value) => {
                    return Err(format!(
                        "ByteArrayInputStream invalid 'jvm_data' field: {:?}",
                        value
                    ));
                }
                None => {
                    let resource_path =
                        if let Some(JvmStackValue::String(path)) = obj.fields.get("jvm_res") {
                            path.clone()
                        } else {
                            return Err("ByteArrayInputStream instance missing backing data".into());
                        };

                    state
                        .resources
                        .get(&resource_path)
                        .cloned()
                        .ok_or_else(|| "Resource not found for ByteArrayInputStream".to_string())?
                }
            };

            (data, pos)
        } else {
            return Err("Expected instance for ByteArrayInputStream object".into());
        };

        match (method_name, descriptor) {
            ("available", "()I") => {
                let available = data.len().saturating_sub(pos);
                Ok(Some(JvmStackValue::Int(available as i32)))
            }
            ("markSupported", "()Z") => Ok(Some(JvmStackValue::Int(1))),
            ("mark", "(I)V") => {
                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) {
                    obj.fields
                        .insert("jvm_mark".to_string(), JvmStackValue::Int(pos as i32));
                }

                Ok(None)
            }
            ("reset", "()V") => {
                let mark = match state.heap.get(stream_ref) {
                    Some(HeapObject::Instance(obj)) => match obj.fields.get("jvm_mark") {
                        Some(JvmStackValue::Int(value)) if *value >= 0 => *value as usize,
                        Some(JvmStackValue::Int(_)) | None => 0,
                        Some(value) => {
                            return Err(format!(
                                "ByteArrayInputStream invalid 'jvm_mark' field: {:?}",
                                value
                            ));
                        }
                    },
                    _ => return Err("Expected instance for ByteArrayInputStream object".into()),
                };

                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) {
                    obj.fields
                        .insert("jvm_pos".to_string(), JvmStackValue::Int(mark as i32));
                }

                Ok(None)
            }
            ("close", "()V") => Ok(None),
            ("skip", "(J)J") => {
                let skip_by = match args.get(1) {
                    Some(JvmStackValue::Long(value)) if *value > 0 => *value as usize,
                    Some(JvmStackValue::Long(_)) => 0,
                    Some(value) => {
                        return Err(format!(
                            "ByteArrayInputStream.skip(J): expected long argument, found {:?}",
                            value
                        ));
                    }
                    None => return Err("ByteArrayInputStream.skip(J): missing argument".into()),
                };

                let remaining = data.len().saturating_sub(pos);
                let skipped = skip_by.min(remaining);

                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) {
                    obj.fields.insert(
                        "jvm_pos".to_string(),
                        JvmStackValue::Int((pos + skipped) as i32),
                    );
                }

                Ok(Some(JvmStackValue::Long(skipped as i64)))
            }
            ("read", "()I") => {
                if pos >= data.len() {
                    return Ok(Some(JvmStackValue::Int(-1)));
                }

                let value = data[pos] as i32;
                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) {
                    obj.fields
                        .insert("jvm_pos".to_string(), JvmStackValue::Int((pos + 1) as i32));
                }

                Ok(Some(JvmStackValue::Int(value)))
            }
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

                if pos >= data.len() {
                    return Ok(Some(JvmStackValue::Int(-1)));
                }

                let copied = match state.heap.get_mut(buffer_ref) {
                    Some(HeapObject::Array { data: buffer, .. }) => {
                        let remaining = &data[pos..];
                        let copy_len = remaining.len().min(buffer.len());

                        for (slot, byte) in buffer.iter_mut().zip(remaining.iter()).take(copy_len) {
                            *slot = JvmStackValue::Int(*byte as i32);
                        }

                        copy_len
                    }
                    Some(_) => return Err("read([B)I: expected array buffer".into()),
                    None => return Err("read([B)I: invalid byte array reference".into()),
                };

                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) {
                    obj.fields.insert(
                        "jvm_pos".to_string(),
                        JvmStackValue::Int((pos + copied) as i32),
                    );
                }

                Ok(Some(JvmStackValue::Int(copied as i32)))
            }
            ("read", "([BII)I") => {
                let buffer_ref = match args.get(1) {
                    Some(JvmStackValue::ObjectRef(r)) => *r as usize,
                    Some(value) => {
                        return Err(format!(
                            "Expected byte array reference as second argument to read(), found {:?}",
                            value
                        ));
                    }
                    None => return Err("read([BII)I: missing byte array argument".into()),
                };
                let offset = match args.get(2) {
                    Some(JvmStackValue::Int(value)) => *value,
                    Some(value) => {
                        return Err(format!("read([BII)I: invalid offset {:?}", value));
                    }
                    None => return Err("read([BII)I: missing offset argument".into()),
                };
                let len = match args.get(3) {
                    Some(JvmStackValue::Int(value)) => *value,
                    Some(value) => {
                        return Err(format!("read([BII)I: invalid length {:?}", value));
                    }
                    None => return Err("read([BII)I: missing length argument".into()),
                };

                if offset < 0 || len < 0 {
                    return Err(format!(
                        "java.lang.IndexOutOfBoundsException: offset {}, length {}",
                        offset, len
                    ));
                }

                let offset = offset as usize;
                let len = len as usize;

                if len == 0 {
                    return Ok(Some(JvmStackValue::Int(0)));
                }

                if pos >= data.len() {
                    return Ok(Some(JvmStackValue::Int(-1)));
                }

                let copied = match state.heap.get_mut(buffer_ref) {
                    Some(HeapObject::Array { data: buffer, .. }) => {
                        if offset > buffer.len() || len > buffer.len().saturating_sub(offset) {
                            return Err(format!(
                                "java.lang.IndexOutOfBoundsException: offset {}, length {}, buffer length {}",
                                offset,
                                len,
                                buffer.len()
                            ));
                        }

                        let remaining = &data[pos..];
                        let copy_len = remaining.len().min(len);

                        for (slot, byte) in buffer[offset..offset + copy_len]
                            .iter_mut()
                            .zip(remaining.iter())
                        {
                            *slot = JvmStackValue::Int(*byte as i32);
                        }

                        copy_len
                    }
                    Some(_) => return Err("read([BII)I: expected array buffer".into()),
                    None => return Err("read([BII)I: invalid byte array reference".into()),
                };

                if let Some(HeapObject::Instance(obj)) = state.heap.get_mut(stream_ref) {
                    obj.fields.insert(
                        "jvm_pos".to_string(),
                        JvmStackValue::Int((pos + copied) as i32),
                    );
                }

                Ok(Some(JvmStackValue::Int(copied as i32)))
            }
            _ => Err(format!(
                "Unsupported ByteArrayInputStream method: {}{}",
                method_name, descriptor
            )),
        }
    }
}
