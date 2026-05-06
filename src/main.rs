mod app;
mod jvm;
mod services;

use app::App;
use jvm::JVM;

use services::jar_extractor::JarExtractor;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    const JAR_PATH: &str = "test_files/tower_defense_(base_128x128_188767.jar";

    let mut jar_extractor = JarExtractor::for_path(JAR_PATH.to_string());

    let data = jar_extractor.run();

    match data {
        Ok(result) => println!("Jar extraction successful: {}", result.data),
        Err(e) => println!("Jar extraction failed: {}", e.message),
    }

    let mut jvm = JVM::new();
    let res = jvm.run_jar(jar_extractor.data.as_ref().unwrap().clone());

    match res {
        Ok(_) => println!("JVM execution successful"),
        Err(e) => println!("JVM execution failed: {}", e),
    }

    let event_loop = EventLoop::new().unwrap();

    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    app.jvm = Some(Box::leak(Box::new(jvm)));
    let res = event_loop.run_app(&mut app);

    if let Err(e) = res {
        println!("Event loop failed: {}", e);
    }
}
