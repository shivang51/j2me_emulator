use std::collections::HashMap;

use classfile_parser::constant_info::{ConstantInfo, FieldRefConstant, MethodRefConstant};

use crate::services::jar_extractor::JarFileData;

#[derive(Debug, Clone)]
pub enum JvmStackValue {
    Int(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    ObjectRef(u32),
    String(String), // Or a pointer to a Heap-allocated string
    Null,
}

#[derive(Debug, Clone)]
pub struct JvmObject {
    pub class_name: String,
    pub fields: HashMap<String, JvmStackValue>,
}

#[derive(Debug)]
pub struct JVM {
    pub static_fields: HashMap<String, JvmStackValue>,
    pub heap: Vec<JvmObject>,
    pub classes: HashMap<String, classfile_parser::ClassFile>,
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
        let mut jvm = JVM {
            static_fields: HashMap::new(),
            heap: Vec::new(),
            classes: HashMap::new(),
        };

        jvm.static_fields.insert(
            "java/lang/System.out:Ljava/io/PrintStream;".to_string(),
            JvmStackValue::ObjectRef(999), // Dummy reference for our native PrintStream
        );

        return jvm;
    }

    pub fn run_jar(&mut self, data: JarFileData) -> Result<Option<JvmStackValue>, String> {
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

        let main_class_name = data.manifest.main_class.replace('.', "/");
        println!("Running main class: {}", main_class_name);

        let main_class = self
            .classes
            .get(&main_class_name)
            .ok_or_else(|| format!("Main class not found: {}", main_class_name))?;

        return self.execute_class(main_class.clone(), Some("startApp".into()));
    }

    pub fn add_class(&mut self, class: classfile_parser::ClassFile) -> Result<(), String> {
        let res = JVM::get_class_name(&class);

        if let Err(e) = res {
            return Err(format!("Failed to get class name: {}", e));
        }

        let class_name = res.unwrap();

        println!("[JVM] Added class: {}", class_name);

        self.classes.insert(class_name, class);

        Ok(())
    }

    // Pass a class with a main method or entry point
    // By default: main method is with name `main`,
    // but we can pass our own
    pub fn execute_class(
        &mut self,
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
                                // Extract bytecode and run interpreter
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
                                    locals.push(JvmStackValue::ObjectRef(class_ref));
                                }
                                let res = JVM::run_frame(&code_attr.code, pool, &mut locals, self);

                                return res;
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
        jvm: &mut JVM,
    ) -> Result<Option<JvmStackValue>, String> {
        let mut pc = 0;
        let mut stack: Vec<JvmStackValue> = Vec::new();

        while pc < bytecode.len() {
            // https://docs.oracle.com/javase/specs/jvms/se8/html/jvms-6.html#jvms-6.5.ldc
            let opcode = bytecode[pc];

            println!(
                "PC: {}, Opcode: {:02X}, Stack: {:?}, Locals: {:?}",
                pc, opcode, stack, locals
            );

            match opcode {
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

                    let val = jvm
                        .static_fields
                        .get(&key)
                        .ok_or_else(|| format!("Static field not found: {}", key));

                    if let Err(e) = &val {
                        println!("Error: {}", e);

                        // print  all static fields available in the JVM for debugging
                        println!("Available static fields:");
                        for (k, v) in &jvm.static_fields {
                            println!("{}: {:?}", k, v);
                            let k_matches = k == &key;
                            println!("match: {:?}", k_matches);
                        }
                        println!("-----------------------------");
                        return Err(e.clone());
                    }

                    stack.push(val.unwrap().clone());

                    pc += 3;
                }
                0x12 => {
                    // ldc

                    let cp_index = bytecode[pc + 1] as usize;
                    let entry = &cp.get(cp_index).expect("Invalid CP index for LDC");

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

                    let obj = jvm
                        .heap
                        .get(heap_idx)
                        .ok_or_else(|| format!("Invalid heap access at index {}", heap_idx))?;

                    let field_value = obj.fields.get(&field_name).ok_or_else(|| {
                        format!(
                            "Field '{}' not found in object of class '{}'",
                            field_name, obj.class_name
                        )
                    })?;

                    stack.push(field_value.clone());
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

                    let obj = jvm
                        .heap
                        .get_mut(heap_idx)
                        .ok_or_else(|| format!("Invalid heap access at index {}", heap_idx))?;

                    obj.fields.insert(field_name, value);

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
                        return Err("java.lang.NullPointerException".into());
                    }

                    if let JvmStackValue::ObjectRef(999) = objectref {
                        JVM::handle_native_printstream(&method_name, &args);
                    } else {
                        let actual_class_name = if let JvmStackValue::ObjectRef(id) = objectref {
                            &jvm.heap[id as usize].class_name.clone()
                        } else {
                            &class_name // Fallback
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
                        return Err("java.lang.NullPointerException".into());
                    }

                    if class_name == "java/lang/Object" && method_name == "<init>" {
                        println!("Skipping native java/lang/Object constructor");
                    } else {
                        // Execute the targeted method.
                        // In a full VM, this creates a new Frame.
                        println!(
                            "invokespecial executing: {}.{}{}",
                            class_name, method_name, descriptor
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

                    // push on the Heap
                    // Note: We don't call <init> here! That is a separate instruction.
                    let objectref = jvm.allocate(class_name);

                    stack.push(JvmStackValue::ObjectRef(objectref));

                    pc += 3;
                }
                0xB1 => {
                    // return
                    println!("Execution finished normally.");
                    return Ok(None);
                }
                0xAC | 0xAF => {
                    // ireturn
                    let val = stack.pop().ok_or("return: Stack underflow")?;

                    println!("Execution finished with return value: {:?}", val);
                    return Ok(Some(val));
                }
                _ => {
                    println!("Unknown Opcode: {:02X}", opcode);
                    pc += 1;
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
        jvm: &mut JVM,
        caller_stack: &mut Vec<JvmStackValue>, // We need this to push the return value!
    ) -> Result<(), String> {
        let class_data = jvm
            .classes
            .get(class_name)
            .ok_or_else(|| format!("ClassDef not found in Universe: {}", class_name))?;

        let method =
            JVM::find_method_in_class(class_data, method_name, descriptor).ok_or_else(|| {
                format!(
                    "Method not found: {}.{}{}",
                    class_name, method_name, descriptor
                )
            })?;

        let code_attr =
            JVM::get_code_attribute(&method, &class_data.const_pool).ok_or_else(|| {
                "Method has no Code attribute (is it abstract or native?)".to_string()
            })?;

        let mut locals = vec![JvmStackValue::Null; code_attr.max_locals as usize];

        locals[0] = objectref;

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

        let return_value = JVM::run_frame(
            &code_attr.code,
            &class_data.const_pool.clone(),
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
        name_utf8.utf8_string.clone()
    }

    pub fn allocate(&mut self, class_name: String) -> u32 {
        let mut fields = HashMap::new();

        // Walk up the inheritance tree to find all fields this object should have
        let mut current_class = Some(class_name.clone());
        while let Some(name) = current_class {
            if let Some(class_data) = self.classes.get(&name) {
                for field_info in &class_data.fields {
                    let f_name = JVM::resolve_utf8(field_info.name_index, &class_data.const_pool);
                    fields.insert(f_name, JvmStackValue::Int(0));
                }
                current_class = JVM::get_super_class_name(class_data);
            } else {
                current_class = None;
            }
        }

        let obj = JvmObject {
            class_name,
            fields: fields,
        };
        self.heap.push(obj);
        (self.heap.len() - 1) as u32 // The objectref
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
}
