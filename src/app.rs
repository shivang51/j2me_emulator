use std::sync::LazyLock;
use std::time::Duration;

use egui_wgpu::RendererOptions;
use egui_winit::egui::FullOutput;
use egui_winit::winit::application::ApplicationHandler;
use egui_winit::winit::event::WindowEvent;
use egui_winit::winit::event_loop::ActiveEventLoop;
use egui_winit::winit::keyboard::{KeyCode, PhysicalKey};
use egui_winit::winit::window::{Window, WindowId};
use egui_winit::{egui, winit};
use parking_lot::Mutex;
use pixels::{Pixels, SurfaceTexture, wgpu};

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
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    game_texture: Option<egui::TextureHandle>,
    pub jvm: Option<JVM>,
}

impl App {
    fn draw(&mut self) {
        if let Some(jvm) = &mut self.jvm {
            let res = jvm.paint();
            if let Err(e) = res {
                eprintln!("[App] jvm.paint() failed: {}", e);
            }
        }

        let full_output = self.draw_ui();
        let window = self.window.unwrap();
        let egui_renderer = self.egui_renderer.as_mut().unwrap();
        let egui_state = self.egui_state.as_mut().unwrap();

        let paint_jobs = egui_state
            .egui_ctx()
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let mut draw_state = DRAW_STATE.lock();
        if let Some(pixels) = &mut draw_state.pixels {
            let screen_descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [window.inner_size().width, window.inner_size().height],
                pixels_per_point: full_output.pixels_per_point,
            };
            pixels
                .render_with(|encoder, render_target, context| {
                    let _ = context;

                    {
                        let _clear_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("clear_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: render_target,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                                depth_slice: None,
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                    }

                    for (id, delta) in &full_output.textures_delta.set {
                        egui_renderer.update_texture(pixels.device(), pixels.queue(), *id, delta);
                    }

                    egui_renderer.update_buffers(
                        pixels.device(),
                        pixels.queue(),
                        encoder,
                        &paint_jobs,
                        &screen_descriptor,
                    );

                    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui_render_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: render_target,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });

                    unsafe {
                        let rpass_static: &mut wgpu::RenderPass<'static> =
                            std::mem::transmute(&mut rpass);
                        egui_renderer.render(rpass_static, &paint_jobs, &screen_descriptor);
                    }

                    Ok(())
                })
                .unwrap();
        }

        for id in full_output.textures_delta.free {
            egui_renderer.free_texture(&id);
        }

        if let Some(repaint_after) = full_output
            .viewport_output
            .get(&egui::viewport::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
        {
            if repaint_after == Duration::ZERO {
                window.request_redraw();
            }
        }
    }

    fn draw_ui(&mut self) -> FullOutput {
        {
            let ctx = self.egui_state.as_ref().unwrap().egui_ctx();
            let mut draw_state = DRAW_STATE.lock();
            let width = draw_state.width;
            let height = draw_state.height;

            if let Some(pixels) = &mut draw_state.pixels {
                let frame = pixels.frame();
                let size = [width as usize, height as usize];
                let image = egui::ColorImage::from_rgba_unmultiplied(size, frame);

                if let Some(handle) = &mut self.game_texture {
                    handle.set(image, egui::TextureOptions::NEAREST);
                } else {
                    self.game_texture =
                        Some(ctx.load_texture("game_screen", image, egui::TextureOptions::NEAREST));
                }
            }
        }

        let egui_state = self.egui_state.as_mut().unwrap();
        let raw_input = egui_state.take_egui_input(self.window.unwrap());
        egui_state.egui_ctx().run_ui(raw_input, |ctx| {
            egui::Panel::top("menu_bar").show_inside(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Open .jar").clicked() {
                            ui.close();
                        }
                        if ui.button("Exit").clicked() {
                            std::process::exit(0);
                        }
                    });
                    ui.menu_button("Emulation", |ui| {
                        if ui.button("Reset").clicked() { /* Reset JVM */ }
                    });
                });
            });

            egui::Panel::right("Right")
                .resizable(true)
                .show_inside(ctx, |ui| {
                    ui.heading("Keypad");
                    ui.add_space(10.0);

                    let buttons = [
                        ["1", "2", "3"],
                        ["4", "5", "6"],
                        ["7", "8", "9"],
                        ["*", "0", "#"],
                    ];

                    egui::Grid::new("phone_grid")
                        .spacing([10.0, 10.0])
                        .show(ui, |ui| {
                            for row in buttons {
                                for label in row {
                                    if ui
                                        .add(egui::Button::new(label).min_size([45.0, 45.0].into()))
                                        .clicked()
                                    {
                                        println!("Key {} pressed", label);
                                    }
                                }
                                ui.end_row();
                            }
                        });

                    ui.add_space(20.0);
                    ui.separator();
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("A").clicked() {
                            println!("A clicked");
                        }
                        if ui.button("B").clicked() {
                            println!("B clicked");
                        }
                    });
                });

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show_inside(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading(
                            self.jvm
                                .as_ref()
                                .and_then(|j| j.loaded_jar.as_ref())
                                .map_or("No file loaded", |jar| &jar.manifest.name),
                        );
                        ui.add_space(8.0);

                        if let Some(texture) = &self.game_texture {
                            let available_size = ui.available_size();
                            let tex_size = texture.size_vec2();
                            let scale = (available_size.x / tex_size.x)
                                .min(available_size.y / tex_size.y)
                                .max(1.0);

                            ui.add(egui::Image::new(texture).fit_to_exact_size(tex_size * scale));
                        }
                    });
                });

            egui::Window::new("Debug Stats")
                .min_size([240.0, 120.0])
                .resizable(true)
                .show(ctx, |ui| {
                    ui.label(format!("Resolution: {}x{}", DEFAULT_WIDTH, DEFAULT_HEIGHT));
                    if ui.button("Dump Heap").clicked() {
                        println!("Dumping heap...");
                    }
                });
        })
    }

    fn add_egui_fonts(ctx: &egui::Context) {
        let builtin_fonts = egui::FontDefinitions::builtin_font_names();

        println!("[+] Built-in fonts: {:?}", builtin_fonts);

        if builtin_fonts.len() != 0 {
            return;
        }

        let mut fonts = egui::FontDefinitions::default();
        let font_candidates = ["/usr/share/fonts/TTF/Segoe-UI-Variable-Static-Display.ttf"];

        if let Some(font_path) = font_candidates
            .iter()
            .find(|path| std::path::Path::new(path).exists())
        {
            if let Ok(font_bytes) = std::fs::read(font_path) {
                fonts.font_data.insert(
                    "segoe_ui".to_owned(),
                    std::sync::Arc::new(egui::FontData::from_owned(font_bytes)),
                );

                if let Some(proportional) = fonts.families.get_mut(&egui::FontFamily::Proportional)
                {
                    proportional.insert(0, "segoe_ui".to_owned());
                }

                if let Some(monospace) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                    monospace.insert(0, "segoe_ui".to_owned());
                }
            }
        }

        ctx.set_fonts(fonts);
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
        if let Some(jar) = self.jvm.as_ref().and_then(|j| j.loaded_jar.as_ref()) {
            window_ref.set_title(&format!("{} - J2ME", jar.manifest.name));
        }

        println!(
            "[+] Window {} resumed with size {}x{}",
            window_ref.title(),
            size.width,
            size.height
        );

        let egui_ctx = egui::Context::default();
        egui_ctx.set_theme(egui::Theme::Dark);

        App::add_egui_fonts(&egui_ctx);

        let egui_state = egui_winit::State::new(
            egui_ctx,
            egui::viewport::ViewportId::ROOT,
            &window_ref,
            Some(window_ref.scale_factor() as f32),
            None,
            None,
        );

        let internal_width = DEFAULT_WIDTH as u32;
        let internal_height = DEFAULT_HEIGHT as u32;

        if size.width > 0 && size.height > 0 {
            let mut draw_state = DRAW_STATE.lock();
            let pixels = Pixels::new(internal_width, internal_height, surface).unwrap();

            let egui_renderer = egui_wgpu::Renderer::new(
                pixels.device(),
                pixels.render_texture_format(),
                RendererOptions::default(),
            );

            draw_state.pixels = Some(pixels);
            draw_state.width = internal_width;
            draw_state.height = internal_height;
            self.egui_renderer = Some(egui_renderer);
        }

        self.window = Some(window_ref);
        self.egui_state = Some(egui_state);

        window_ref.request_redraw();
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let window = self.window.unwrap();
        let egui_state = self.egui_state.as_mut().unwrap();

        let response = egui_state.on_window_event(window, &event);
        if response.consumed {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                std::process::exit(0);
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                window.request_redraw();
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
                    window.request_redraw();
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
