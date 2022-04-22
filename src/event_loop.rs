use crate::*;

use futures::executor::block_on;
use std::collections::HashMap;

use tao::{
    accelerator::Accelerator,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::ModifiersState,
    menu::{MenuBar as Menu, MenuItem, MenuItemAttributes},
    window::{Window, WindowBuilder},
};

// See https://rust-lang.github.io/api-guidelines/future-proofing.html
pub(crate) mod private {
    pub trait Sealed {}
}

pub type KeyCode = tao::keyboard::KeyCode;
pub type KeyPress = tao::keyboard::Key<'static>;
pub type WEvent<'a, T> = tao::event::Event<'a, T>;
pub type WMouseButton = tao::event::MouseButton;

struct Setup {
    size: PhysicalSize<u32>,
    surface: wgpu::Surface,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

async fn setup(window: &Window) -> Setup {
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::WindowExtWebSys;
        let query_string = web_sys::window().unwrap().location().search().unwrap();
        let level: log::Level = parse_url_query_string(&query_string, "RUST_LOG")
            .map(|x| x.parse().ok())
            .flatten()
            .unwrap_or(log::Level::Error);
        console_log::init_with_level(level).expect("could not initialize logger");
        std::panic::set_hook(Box::new(console_error_panic_hook::hook));
        // On wasm, append the canvas to the document body
        web_sys::window()
            .and_then(|win| win.document())
            .and_then(|doc| doc.body())
            .and_then(|body| {
                body.append_child(&web_sys::Element::from(window.canvas()))
                    .ok()
            })
            .expect("couldn't append canvas to document body");
    }

    // log::info!("Initializing the surface...");

    let backend = wgpu::util::backend_bits_from_env().unwrap_or_else(wgpu::Backends::all);

    let instance = wgpu::Instance::new(backend);
    let (size, surface) = unsafe {
        let size = window.inner_size();
        let surface = instance.create_surface(&window);
        (size, surface)
    };
    let adapter =
        wgpu::util::initialize_adapter_from_env_or_default(&instance, backend, Some(&surface))
            .await
            .expect("No suitable GPU adapters found on the system!");

    #[cfg(not(target_arch = "wasm32"))]
    {
        let adapter_info = adapter.get_info();
        println!("Using {} ({:?})", adapter_info.name, adapter_info.backend);
    }

    let trace_dir = std::env::var("WGPU_TRACE");
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                features: wgpu::Features::default(),
                limits: wgpu::Limits::default(),
            },
            trace_dir.ok().as_ref().map(std::path::Path::new),
        )
        .await
        .expect("Unable to find a suitable GPU adapter!");

    Setup {
        size,
        surface,
        adapter,
        device,
        queue,
    }
}

struct MenuItem2 {
    name: String,
    submenu: Vec<usize>,
    command: CommandInfo,
}

fn make_menu_rec(
    items: &Vec<MenuItem2>,
    i: usize,
    command_map: &mut HashMap<tao::menu::MenuId, String>,
) -> Menu {
    let mut menu = Menu::new();

    if i == 0 {
        let mut app_menu = Menu::new();

        let app_name = match std::env::current_exe() {
            Ok(exe_path) => exe_path.file_name().unwrap().to_str().unwrap().to_string(),
            Err(_) => "rui".to_string(),
        };

        app_menu.add_native_item(MenuItem::About(app_name, Default::default()));
        app_menu.add_native_item(MenuItem::Quit);
        menu.add_submenu("rui", true, app_menu);
    }

    for j in &items[i].submenu {
        let item = &items[*j];
        if !item.submenu.is_empty() {
            menu.add_submenu(
                item.name.as_str(),
                true,
                make_menu_rec(items, *j, command_map),
            );
        } else {
            let mut attrs = MenuItemAttributes::new(item.name.as_str());
            if let Some(key) = item.command.key {
                let accel = Accelerator::new(ModifiersState::SUPER, key);
                attrs = attrs.with_accelerators(&accel);
            }
            let id = menu.add_item(attrs).id();
            command_map.insert(id, item.command.path.clone());
        }
    }

    menu
}

pub(crate) fn build_menubar(commands: &Vec<CommandInfo>, command_map: &mut CommandMap) -> Menu {
    let mut items: Vec<MenuItem2> = vec![MenuItem2 {
        name: "root".into(),
        submenu: vec![],
        command: CommandInfo {
            path: "".into(),
            key: None,
        },
    }];

    for command in commands {
        let mut v = 0;
        for name in command.path.split(':') {
            if let Some(item) = items[v].submenu.iter().find(|x| items[**x].name == name) {
                v = *item;
            } else {
                let n = items.len();
                items[v].submenu.push(n);
                v = n;
                items.push(MenuItem2 {
                    name: name.into(),
                    submenu: vec![],
                    command: command.clone(),
                });
            }
        }
    }

    make_menu_rec(&items, 0, command_map)
}

/// Call this function to run your UI.
pub fn rui(view: impl View) {
    let event_loop = EventLoop::new();

    let builder = WindowBuilder::new().with_title("rui");
    let window = builder.build(&event_loop).unwrap();

    let setup = block_on(setup(&window));
    let surface = setup.surface;
    let device = setup.device;
    let size = setup.size;
    let adapter = setup.adapter;
    let queue = setup.queue;

    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface.get_preferred_format(&adapter).unwrap(),
        width: size.width,
        height: size.height,
        present_mode: wgpu::PresentMode::Mailbox,
    };
    surface.configure(&device, &config);

    *GLOBAL_EVENT_LOOP_PROXY.lock().unwrap() = Some(event_loop.create_proxy());

    let mut vger = Vger::new(&device, wgpu::TextureFormat::Bgra8UnormSrgb);
    let mut cx = Context::new(Some(window));
    let mut mouse_position = LocalPoint::zero();

    let mut commands = Vec::new();
    cx.commands(&view, &mut commands);
    let mut command_map = HashMap::new();
    cx.window
        .as_ref()
        .unwrap()
        .set_menu(Some(build_menubar(&commands, &mut command_map)));

    let mut access_nodes = vec![];

    event_loop.run(move |event, _, control_flow| {
        // ControlFlow::Poll continuously runs the event loop, even if the OS hasn't
        // dispatched any events. This is ideal for games and similar applications.
        // *control_flow = ControlFlow::Poll;

        // ControlFlow::Wait pauses the event loop if no events are available to process.
        // This is ideal for non-game applications that only update in response to user
        // input, and uses significantly less power/CPU time than ControlFlow::Poll.
        *control_flow = ControlFlow::Wait;

        match event {
            WEvent::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                println!("The close button was pressed; stopping");
                *control_flow = ControlFlow::Exit
            }
            WEvent::WindowEvent {
                event:
                    WindowEvent::Resized(size)
                    | WindowEvent::ScaleFactorChanged {
                        new_inner_size: &mut size,
                        ..
                    },
                ..
            } => {
                // println!("Resizing to {:?}", size);
                config.width = size.width.max(1);
                config.height = size.height.max(1);
                surface.configure(&device, &config);
                cx.window.as_ref().unwrap().request_redraw();
            }
            WEvent::UserEvent(_) => {
                // println!("received user event");

                // Process the work queue.
                while let Some(f) = GLOBAL_WORK_QUEUE.lock().unwrap().pop_front() {
                    f(&mut cx);
                }
            }
            WEvent::MainEventsCleared => {
                // Application update code.

                // Queue a RedrawRequested event.
                //
                // You only need to call this if you've determined that you need to redraw, in
                // applications which do not always need to. Applications that redraw continuously
                // can just render here instead.

                cx.update(
                    &view,
                    &mut vger,
                    &mut commands,
                    &mut command_map,
                    &mut access_nodes,
                );
            }
            WEvent::RedrawRequested(_) => {
                // Redraw the application.
                //
                // It's preferable for applications that do not render continuously to render in
                // this event rather than in MainEventsCleared, since rendering in here allows
                // the program to gracefully handle redraws requested by the OS.

                // println!("RedrawRequested");
                cx.render(&device, &surface, &config, &queue, &view, &mut vger);
            }
            WEvent::WindowEvent {
                event: WindowEvent::MouseInput { state, button, .. },
                ..
            } => {
                match state {
                    ElementState::Pressed => {
                        cx.mouse_button = match button {
                            WMouseButton::Left => Some(MouseButton::Left),
                            WMouseButton::Right => Some(MouseButton::Right),
                            WMouseButton::Middle => Some(MouseButton::Center),
                            _ => None,
                        };
                        let event = Event::TouchBegin {
                            id: 0,
                            position: mouse_position,
                        };
                        cx.process(&view, &event, &mut vger)
                    }
                    ElementState::Released => {
                        cx.mouse_button = None;
                        let event = Event::TouchEnd {
                            id: 0,
                            position: mouse_position,
                        };
                        cx.process(&view, &event, &mut vger)
                    }
                    _ => {}
                };
            }
            WEvent::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. },
                ..
            } => {
                let scale = cx.window.as_ref().unwrap().scale_factor() as f32;
                mouse_position = [
                    position.x as f32 / scale,
                    (config.height as f32 - position.y as f32) / scale,
                ]
                .into();
                let event = Event::TouchMove {
                    id: 0,
                    position: mouse_position,
                };
                cx.process(&view, &event, &mut vger)
            }
            WEvent::WindowEvent {
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } => {
                if event.state == ElementState::Pressed {
                    let key = match event.logical_key {
                        KeyPress::Character(c) => Some(Key::Character(c)),
                        KeyPress::Enter => Some(Key::Enter),
                        KeyPress::Tab => Some(Key::Tab),
                        KeyPress::Space => Some(Key::Space),
                        KeyPress::ArrowDown => Some(Key::ArrowDown),
                        KeyPress::ArrowLeft => Some(Key::ArrowLeft),
                        KeyPress::ArrowRight => Some(Key::ArrowRight),
                        KeyPress::ArrowUp => Some(Key::ArrowUp),
                        KeyPress::End => Some(Key::End),
                        KeyPress::Home => Some(Key::Home),
                        KeyPress::PageDown => Some(Key::PageDown),
                        KeyPress::PageUp => Some(Key::PageUp),
                        KeyPress::Backspace => Some(Key::Backspace),
                        KeyPress::Delete => Some(Key::Delete),
                        KeyPress::Escape => Some(Key::Escape),
                        KeyPress::F1 => Some(Key::F1),
                        KeyPress::F2 => Some(Key::F2),
                        KeyPress::F3 => Some(Key::F3),
                        KeyPress::F4 => Some(Key::F4),
                        KeyPress::F5 => Some(Key::F5),
                        KeyPress::F6 => Some(Key::F6),
                        KeyPress::F7 => Some(Key::F7),
                        KeyPress::F8 => Some(Key::F8),
                        KeyPress::F9 => Some(Key::F9),
                        KeyPress::F10 => Some(Key::F10),
                        KeyPress::F11 => Some(Key::F11),
                        KeyPress::F12 => Some(Key::F12),
                        _ => None,
                    };

                    if let Some(key) = key {
                        cx.process(&view, &Event::Key(key), &mut vger)
                    }
                }
            }
            WEvent::WindowEvent {
                event: WindowEvent::ModifiersChanged(mods),
                ..
            } => {
                // println!("modifiers changed: {:?}", mods);
                cx.key_mods = KeyboardModifiers {
                    shift: mods.shift_key(),
                    control: mods.control_key(),
                    alt: mods.alt_key(),
                    command: mods.super_key(),
                };
            }
            WEvent::MenuEvent { menu_id, .. } => {
                //println!("menu event");

                if let Some(command) = command_map.get(&menu_id) {
                    //println!("found command {:?}", command);
                    let event = Event::Command(command.clone());
                    cx.process(&view, &event, &mut vger)
                }
            }
            _ => (),
        }
    });
}
