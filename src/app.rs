use std::sync::LazyLock;

use egui_winit::winit;
use egui_winit::winit::application::ApplicationHandler;
use egui_winit::winit::event::WindowEvent;
use egui_winit::winit::event_loop::ActiveEventLoop;
use egui_winit::winit::keyboard::{KeyCode, PhysicalKey};
use egui_winit::winit::window::{Window, WindowId};
use parking_lot::Mutex;
use pixels::{Pixels, SurfaceTexture};

use crate::jvm::JVM;
use crate::jvm::javax::lcdui::game::game_canvas::{DEFAULT_HEIGHT, DEFAULT_WIDTH};

pub struct DrawState {
    pub pixels: Option<Pixels<'static>>,
    pub width: u32,
    pub height: u32,
}

pub struct InputState {
    pub space_pressed: bool,
    pub up_pressed: bool,
    pub down_pressed: bool,
    pub left_pressed: bool,
    pub right_pressed: bool,
    pub a_pressed: bool,
    pub b_pressed: bool,
    pub c_pressed: bool,
    pub d_pressed: bool,
}

pub static INPUT_STATE: LazyLock<Mutex<InputState>> = LazyLock::new(|| {
    Mutex::new(InputState {
        space_pressed: false,
        up_pressed: false,
        down_pressed: false,
        left_pressed: false,
        right_pressed: false,
        a_pressed: false,
        b_pressed: false,
        c_pressed: false,
        d_pressed: false,
    })
});

pub static DRAW_STATE: LazyLock<Mutex<DrawState>> = LazyLock::new(|| {
    Mutex::new(DrawState {
        pixels: None,
        width: 0,
        height: 0,
    })
});

#[derive(Default)]
pub struct App {
    window: Option<&'static Window>,
    pub jvm: Option<JVM>,
}

impl App {
    fn draw(&mut self) {
        // {
        //     let mut draw_state = DRAW_STATE.lock();
        //     if let Some(pixels) = &mut draw_state.pixels {
        //         pixels.clear_color(Color::BLACK);
        //     }
        // }
        //
        if let Some(jvm) = &mut self.jvm {
            let res = jvm.paint();
            if let Err(e) = res {
                eprintln!("[App] jvm.paint() failed: {}", e);
                panic!("JVM paint failed");
            }
        }

        let mut draw_state = DRAW_STATE.lock();
        if let Some(pixels) = &mut draw_state.pixels {
            pixels.render().unwrap();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes().with_title("No file loaded - J2ME"))
            .unwrap();
        let window_ref: &'static Window = Box::leak(Box::new(window));
        let size = window_ref.inner_size();
        let surface = SurfaceTexture::new(size.width, size.height, window_ref);
        if let Some(jar) = &self.jvm.as_ref().unwrap().loaded_jar {
            window_ref.set_title(&format!("{} - J2ME", jar.manifest.name));
        }

        println!(
            "[+] Window {} resumed with size {}x{}",
            window_ref.title(),
            size.width,
            size.height
        );

        self.window = Some(window_ref);

        let internal_width = DEFAULT_WIDTH as u32;
        let internal_height = DEFAULT_HEIGHT as u32;

        if size.width > 0 && size.height > 0 {
            let mut draw_state = DRAW_STATE.lock();
            draw_state.pixels =
                Some(Pixels::new(internal_width, internal_height, surface).unwrap());
            draw_state.width = internal_width;
            draw_state.height = internal_height;
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                std::process::exit(0);
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
                    println!("Window resized to {}x{}", new_size.width, new_size.height);
                    let mut draw_state = DRAW_STATE.lock();
                    if let Some(pixels) = &mut draw_state.pixels {
                        pixels
                            .resize_surface(new_size.width, new_size.height)
                            .unwrap();
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let keycode = if let PhysicalKey::Code(code) = event.physical_key {
                    code
                } else {
                    return;
                };

                let is_pressed = event.state == winit::event::ElementState::Pressed;

                match keycode {
                    KeyCode::Space => {
                        INPUT_STATE.lock().space_pressed = is_pressed;
                    }
                    KeyCode::ArrowUp => {
                        INPUT_STATE.lock().up_pressed = is_pressed;
                    }
                    KeyCode::ArrowDown => {
                        INPUT_STATE.lock().down_pressed = is_pressed;
                    }
                    KeyCode::ArrowLeft => {
                        INPUT_STATE.lock().left_pressed = is_pressed;
                    }
                    KeyCode::ArrowRight => {
                        INPUT_STATE.lock().right_pressed = is_pressed;
                    }
                    KeyCode::KeyA => {
                        INPUT_STATE.lock().a_pressed = is_pressed;
                    }
                    KeyCode::KeyS => {
                        INPUT_STATE.lock().b_pressed = is_pressed;
                    }
                    KeyCode::KeyD => {
                        INPUT_STATE.lock().c_pressed = is_pressed;
                    }
                    KeyCode::KeyF => {
                        INPUT_STATE.lock().d_pressed = is_pressed;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
