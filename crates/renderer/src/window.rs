use dioxus_core::VirtualDom;
use freya_common::EventMessage;
use freya_core::{
    accessibility::AccessibilityFocusDirection,
    dom::SafeDOM,
    events::{EventName, PlatformEvent},
    navigation_mode::NavigationMode,
};
use freya_elements::events::{
    map_winit_key, map_winit_modifiers, map_winit_physical_key, Code, Key,
};
use freya_engine::prelude::*;
use gl::{types::*, *};
use glutin::{
    config::{ConfigTemplateBuilder, GlConfig},
    context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext},
    display::{GetGlDisplay, GlDisplay},
    surface::{Surface as GlutinSurface, SurfaceAttributesBuilder, WindowSurface},
};
use glutin::{context::GlProfile, surface::GlSurface};
use glutin::{context::NotCurrentGlContext, surface::SwapInterval};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasRawWindowHandle;
use std::{ffi::CString, sync::Arc};
use std::{mem, num::NonZeroU32, path::PathBuf};
use tokio::sync::Notify;
use torin::geometry::CursorPoint;

use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{
        ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, StartCause, Touch, TouchPhase,
        WindowEvent,
    },
    event_loop::EventLoopProxy,
    keyboard::ModifiersState,
};
use winit::{event_loop::EventLoop, window::Window};

use crate::{app::Application, config::WindowConfig, HoveredNode, LaunchConfig};

const WHEEL_SPEED_MODIFIER: f32 = 53.0;

pub struct NotCreatedState<'a, State: Clone + 'static> {
    pub(crate) sdom: SafeDOM,
    pub(crate) vdom: VirtualDom,
    pub(crate) mutations_notifier: Option<Arc<Notify>>,
    pub(crate) config: LaunchConfig<'a, State>,
}

pub struct CreatedState {
    pub(crate) gr_context: DirectContext,
    pub(crate) surface: Surface,
    pub(crate) gl_surface: GlutinSurface<WindowSurface>,
    pub(crate) gl_context: PossiblyCurrentContext,
    pub(crate) window: Window,
    pub(crate) window_config: WindowConfig,
    pub(crate) fb_info: FramebufferInfo,
    pub(crate) num_samples: usize,
    pub(crate) stencil_size: usize,
    pub(crate) app: Application,
}

#[derive(Default)]
pub enum WindowState<'a, State: Clone + 'static> {
    #[default]
    None,
    NotCreated(NotCreatedState<'a, State>),
    Created(CreatedState),
}

impl<'a, State: Clone + 'a> WindowState<'a, State> {
    pub fn created_state(&mut self) -> &mut CreatedState {
        match self {
            Self::Created(created) => created,
            _ => {
                panic!("Unexpected.")
            }
        }
    }

    pub fn not_created_state(self) -> NotCreatedState<'a, State> {
        match self {
            Self::NotCreated(not_created) => not_created,
            _ => {
                panic!("Unexpected.")
            }
        }
    }

    pub fn app(&mut self) -> &mut Application {
        match self {
            Self::Created(CreatedState { app, .. }) => app,
            _ => {
                panic!("Unexpected.")
            }
        }
    }
}

/// Manager for a Window
pub struct DesktopRenderer<'a, State: Clone + 'static> {
    pub(crate) proxy: EventLoopProxy<EventMessage>,
    pub(crate) state: WindowState<'a, State>,
    pub(crate) hovered_node: HoveredNode,

    pub(crate) cursor_pos: CursorPoint,
    pub(crate) modifiers_state: ModifiersState,
    pub(crate) dropped_file_path: Option<PathBuf>,
}

impl<'a, State: Clone> ApplicationHandler<EventMessage> for DesktopRenderer<'a, State> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let state = match &mut self.state {
            WindowState::Created(created) => created,
            WindowState::NotCreated(_) => {
                let NotCreatedState {
                    sdom,
                    vdom,
                    mutations_notifier,
                    mut config,
                } = mem::take(&mut self.state).not_created_state();

                let mut window_attributes = Window::default_attributes()
                    .with_visible(false)
                    .with_title(config.window_config.title)
                    .with_decorations(config.window_config.decorations)
                    .with_transparent(config.window_config.transparent)
                    .with_window_icon(config.window_config.icon.take())
                    .with_inner_size(LogicalSize::<f64>::new(
                        config.window_config.width,
                        config.window_config.height,
                    ));

                set_resource_cache_total_bytes_limit(1000000); // 1MB
                set_resource_cache_single_allocation_byte_limit(Some(500000)); // 0.5MB

                if let Some(min_size) = config
                    .window_config
                    .min_width
                    .zip(config.window_config.min_height)
                {
                    window_attributes =
                        window_attributes.with_min_inner_size(LogicalSize::<f64>::from(min_size))
                }

                if let Some(max_size) = config
                    .window_config
                    .max_width
                    .zip(config.window_config.max_height)
                {
                    window_attributes =
                        window_attributes.with_max_inner_size(LogicalSize::<f64>::from(max_size))
                }

                if let Some(with_window_builder) = &config.window_config.window_builder_hook {
                    window_attributes = (with_window_builder)(window_attributes);
                }

                let template = ConfigTemplateBuilder::new()
                    .with_alpha_size(8)
                    .with_transparency(config.window_config.transparent);

                let display_builder =
                    DisplayBuilder::new().with_window_attributes(Some(window_attributes));
                let (window, gl_config) = display_builder
                    .build(event_loop, template, |configs| {
                        configs
                            .reduce(|accum, config| {
                                let transparency_check =
                                    config.supports_transparency().unwrap_or(false)
                                        & !accum.supports_transparency().unwrap_or(false);

                                if transparency_check || config.num_samples() < accum.num_samples()
                                {
                                    config
                                } else {
                                    accum
                                }
                            })
                            .unwrap()
                    })
                    .unwrap();

                let mut window = window.expect("Could not create window with OpenGL context");

                // Allow IME
                window.set_ime_allowed(true);

                // Workaround for accesskit
                window.set_visible(true);

                let raw_window_handle = window.raw_window_handle();

                let context_attributes = ContextAttributesBuilder::new()
                    .with_profile(GlProfile::Core)
                    .build(Some(raw_window_handle));

                let fallback_context_attributes = ContextAttributesBuilder::new()
                    .with_profile(GlProfile::Core)
                    .with_context_api(ContextApi::Gles(None))
                    .build(Some(raw_window_handle));

                let not_current_gl_context = unsafe {
                    gl_config
                        .display()
                        .create_context(&gl_config, &context_attributes)
                        .unwrap_or_else(|_| {
                            gl_config
                                .display()
                                .create_context(&gl_config, &fallback_context_attributes)
                                .expect("failed to create context")
                        })
                };

                let (width, height): (u32, u32) = window.inner_size().into();

                let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
                    raw_window_handle,
                    NonZeroU32::new(width).unwrap(),
                    NonZeroU32::new(height).unwrap(),
                );

                let gl_surface = unsafe {
                    gl_config
                        .display()
                        .create_window_surface(&gl_config, &attrs)
                        .expect("Could not create gl window surface")
                };

                let gl_context = not_current_gl_context
                    .make_current(&gl_surface)
                    .expect("Could not make GL context current when setting up skia renderer");

                load_with(|s| {
                    gl_config
                        .display()
                        .get_proc_address(CString::new(s).unwrap().as_c_str())
                });
                let interface = Interface::new_load_with(|name| {
                    if name == "eglGetCurrentDisplay" {
                        return std::ptr::null();
                    }
                    gl_config
                        .display()
                        .get_proc_address(CString::new(name).unwrap().as_c_str())
                })
                .expect("Could not create interface");

                let mut gr_context = DirectContext::new_gl(interface, None)
                    .expect("Could not create direct context");

                let fb_info = {
                    let mut fboid: GLint = 0;
                    unsafe { GetIntegerv(FRAMEBUFFER_BINDING, &mut fboid) };

                    FramebufferInfo {
                        fboid: fboid.try_into().unwrap(),
                        format: Format::RGBA8.into(),
                        ..Default::default()
                    }
                };

                let num_samples = gl_config.num_samples() as usize;
                let stencil_size = gl_config.stencil_size() as usize;

                let mut surface = create_surface(
                    &mut window,
                    fb_info,
                    &mut gr_context,
                    num_samples,
                    stencil_size,
                );

                let scale_factor = window.scale_factor() as f32;
                surface.canvas().scale((scale_factor, scale_factor));

                let mut app = Application::new(
                    sdom,
                    vdom,
                    &self.proxy,
                    mutations_notifier,
                    &window,
                    &config.embedded_fonts,
                    config.plugins,
                    config.default_fonts,
                );

                app.init_doms(scale_factor, config.state.clone());
                app.process_layout(window.inner_size(), scale_factor);

                self.state = WindowState::Created(CreatedState {
                    gr_context,
                    surface,
                    gl_surface,
                    gl_context,
                    window,
                    fb_info,
                    num_samples,
                    stencil_size,
                    app,
                    window_config: config.window_config,
                });

                self.state.created_state()
            }
            _ => {
                panic!("Unexpected.")
            }
        };

        // Try setting vsync.
        if let Err(res) = state.gl_surface.set_swap_interval(
            &state.gl_context,
            SwapInterval::Wait(NonZeroU32::new(1).unwrap()),
        ) {
            eprintln!("Error setting vsync: {res:?}");
        }
    }

    fn new_events(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        if cause == StartCause::Init {
            self.proxy.send_event(EventMessage::PollVDOM).unwrap();
        }
    }

    fn user_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        event: EventMessage,
    ) {
        let scale_factor = self.scale_factor();
        let CreatedState { window, app, .. } = self.state.created_state();
        match event {
            EventMessage::FocusAccessibilityNode(id) => {
                app.accessibility.set_accessibility_focus(id, window);
            }
            EventMessage::RequestRerender => {
                window.request_redraw();
            }
            EventMessage::RemeasureTextGroup(text_id) => {
                app.measure_text_group(&text_id, scale_factor);
            }
            EventMessage::Accessibility(accesskit_winit::WindowEvent::ActionRequested(request)) => {
                if accesskit::Action::Focus == request.action {
                    app.accessibility
                        .set_accessibility_focus(request.target, window);
                }
            }
            EventMessage::Accessibility(accesskit_winit::WindowEvent::InitialTreeRequested) => {
                app.accessibility.process_initial_tree();
            }
            EventMessage::SetCursorIcon(icon) => window.set_cursor(icon),
            EventMessage::FocusPrevAccessibilityNode => {
                app.set_navigation_mode(NavigationMode::Keyboard);
                app.focus_next_node(AccessibilityFocusDirection::Backward, window);
            }
            EventMessage::FocusNextAccessibilityNode => {
                app.set_navigation_mode(NavigationMode::Keyboard);
                app.focus_next_node(AccessibilityFocusDirection::Forward, window);
            }
            ev => {
                if let EventMessage::UpdateTemplate(template) = ev {
                    app.vdom_replace_template(template);
                }

                if matches!(ev, EventMessage::PollVDOM)
                    || matches!(ev, EventMessage::UpdateTemplate(_))
                {
                    app.poll_vdom(window);
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let scale_factor = self.scale_factor();
        let CreatedState {
            gr_context,
            surface,
            gl_surface,
            gl_context,
            window,
            app,
            window_config,
            fb_info,
            num_samples,
            stencil_size,
            ..
        } = self.state.created_state();
        app.accessibility
            .process_accessibility_event(&event, window);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.send_event(PlatformEvent::Keyboard {
                    name: EventName::KeyDown,
                    key: Key::Character(text),
                    code: Code::Unidentified,
                    modifiers: map_winit_modifiers(self.modifiers_state),
                });
            }
            WindowEvent::RedrawRequested => {
                if app.measure_layout_on_next_render {
                    app.process_layout(window.inner_size(), scale_factor);

                    app.measure_layout_on_next_render = false;
                }
                surface.canvas().clear(window_config.background);
                app.render(&self.hovered_node, surface.canvas(), window);
                app.event_loop_tick();
                window.pre_present_notify();
                gr_context.flush_and_submit();
                gl_surface.swap_buffers(gl_context).unwrap();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                app.set_navigation_mode(NavigationMode::NotKeyboard);

                let name = match state {
                    ElementState::Pressed => EventName::MouseDown,
                    ElementState::Released => match button {
                        MouseButton::Middle => EventName::MiddleClick,
                        MouseButton::Right => EventName::RightClick,
                        MouseButton::Left => EventName::Click,
                        _ => EventName::PointerUp,
                    },
                };

                self.send_event(PlatformEvent::Mouse {
                    name,
                    cursor: self.cursor_pos,
                    button: Some(button),
                });
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                if TouchPhase::Moved == phase {
                    let scroll_data = {
                        match delta {
                            MouseScrollDelta::LineDelta(x, y) => (
                                (x * WHEEL_SPEED_MODIFIER) as f64,
                                (y * WHEEL_SPEED_MODIFIER) as f64,
                            ),
                            MouseScrollDelta::PixelDelta(pos) => (pos.x, pos.y),
                        }
                    };

                    self.send_event(PlatformEvent::Wheel {
                        name: EventName::Wheel,
                        scroll: CursorPoint::from(scroll_data),
                        cursor: self.cursor_pos,
                    });
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers_state = modifiers.state();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key,
                        logical_key,
                        state,
                        ..
                    },
                ..
            } => {
                let name = match state {
                    ElementState::Pressed => EventName::KeyDown,
                    ElementState::Released => EventName::KeyUp,
                };
                self.send_event(PlatformEvent::Keyboard {
                    name,
                    key: map_winit_key(&logical_key),
                    code: map_winit_physical_key(&physical_key),
                    modifiers: map_winit_modifiers(self.modifiers_state),
                })
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_pos = CursorPoint::new(-1.0, -1.0);

                self.send_event(PlatformEvent::Mouse {
                    name: EventName::MouseOver,
                    cursor: self.cursor_pos,
                    button: None,
                });
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = CursorPoint::from((position.x, position.y));

                self.send_event(PlatformEvent::Mouse {
                    name: EventName::MouseOver,
                    cursor: self.cursor_pos,
                    button: None,
                });

                if let Some(dropped_file_path) = self.dropped_file_path.take() {
                    self.send_event(PlatformEvent::File {
                        name: EventName::FileDrop,
                        file_path: Some(dropped_file_path),
                        cursor: self.cursor_pos,
                    });
                }
            }
            WindowEvent::Touch(Touch {
                location,
                phase,
                id,
                force,
                ..
            }) => {
                self.cursor_pos = CursorPoint::from((location.x, location.y));

                let name = match phase {
                    TouchPhase::Cancelled => EventName::TouchCancel,
                    TouchPhase::Ended => EventName::TouchEnd,
                    TouchPhase::Moved => EventName::TouchMove,
                    TouchPhase::Started => EventName::TouchStart,
                };

                self.send_event(PlatformEvent::Touch {
                    name,
                    location: self.cursor_pos,
                    finger_id: id,
                    phase,
                    force,
                });
            }
            WindowEvent::Resized(size) => {
                *surface =
                    create_surface(window, *fb_info, gr_context, *num_samples, *stencil_size);

                gl_surface.resize(
                    gl_context,
                    NonZeroU32::new(size.width.max(1)).unwrap(),
                    NonZeroU32::new(size.height.max(1)).unwrap(),
                );

                window.request_redraw();

                app.resize(size);

                app.resize(size);
            }
            WindowEvent::DroppedFile(file_path) => {
                self.dropped_file_path = Some(file_path);
            }
            WindowEvent::HoveredFile(file_path) => {
                self.send_event(PlatformEvent::File {
                    name: EventName::GlobalFileHover,
                    file_path: Some(file_path),
                    cursor: self.cursor_pos,
                });
            }
            WindowEvent::HoveredFileCancelled => {
                self.send_event(PlatformEvent::File {
                    name: EventName::GlobalFileHoverCancelled,
                    file_path: None,
                    cursor: self.cursor_pos,
                });
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        self.run_on_exit();
    }
}

impl<T: Clone> Drop for DesktopRenderer<'_, T> {
    fn drop(&mut self) {
        // if let WindowState::Created {
        //     gl_context,
        //     gl_surface,
        //     mut gr_context,
        //     ..
        // } = self.state
        // {
        //     if !gl_context.is_current() && gl_context.make_current(&gl_surface).is_err() {
        //         gr_context.abandon();
        //     }
        // }
    }
}

impl<'a, State: Clone + 'static> DesktopRenderer<'a, State> {
    /// Run the Desktop Renderer.
    pub fn launch(
        vdom: VirtualDom,
        sdom: SafeDOM,
        config: LaunchConfig<State>,
        mutations_notifier: Option<Arc<Notify>>,
        hovered_node: HoveredNode,
    ) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let _guard = rt.enter();

        let event_loop = EventLoop::<EventMessage>::with_user_event()
            .build()
            .expect("Failed to create event loop.");
        let proxy = event_loop.create_proxy();

        // Hotreload support for Dioxus
        #[cfg(feature = "hot-reload")]
        {
            use std::process::exit;
            let proxy = proxy.clone();
            dioxus_hot_reload::connect(move |msg| match msg {
                dioxus_hot_reload::HotReloadMsg::UpdateTemplate(template) => {
                    let _ = proxy.send_event(EventMessage::UpdateTemplate(template));
                }
                dioxus_hot_reload::HotReloadMsg::Shutdown => exit(0),
                dioxus_hot_reload::HotReloadMsg::UpdateAsset(_) => {}
            });
        }

        let mut desktop_renderer =
            DesktopRenderer::new(vdom, sdom, config, mutations_notifier, hovered_node, proxy);

        event_loop.run_app(&mut desktop_renderer).unwrap();
    }

    pub fn new(
        vdom: VirtualDom,
        sdom: SafeDOM,
        config: LaunchConfig<'a, State>,
        mutations_notifier: Option<Arc<Notify>>,
        hovered_node: HoveredNode,
        proxy: EventLoopProxy<EventMessage>,
    ) -> Self {
        DesktopRenderer {
            state: WindowState::NotCreated(NotCreatedState {
                sdom,
                mutations_notifier,
                vdom,
                config,
            }),
            hovered_node,
            proxy,
            cursor_pos: CursorPoint::default(),
            modifiers_state: ModifiersState::default(),
            dropped_file_path: None,
        }
    }

    fn send_event(&mut self, event: PlatformEvent) {
        let scale_factor = self.scale_factor();
        self.state.app().send_event(event, scale_factor);
    }

    fn scale_factor(&self) -> f32 {
        match &self.state {
            WindowState::Created(CreatedState { window, .. }) => window.scale_factor() as f32,
            _ => 0.0,
        }
    }

    /// Run the `on_setup` callback that was passed to the launch function
    pub fn run_on_setup(&mut self) {
        // let on_setup = config.window.on_setup.clone();
        // if let Some(on_setup) = on_setup {
        //     (on_setup)(&mut self.state.window())
        // }
    }

    /// Run the `on_exit` callback that was passed to the launch function
    pub fn run_on_exit(&mut self) {
        // let on_exit = config.window.on_exit.clone();
        // if let Some(on_exit) = on_exit {
        //     (on_exit)(&mut self.state.window())
        // }
    }
}

/// Create the surface for Skia to render in
fn create_surface(
    window: &mut Window,
    fb_info: FramebufferInfo,
    gr_context: &mut DirectContext,
    num_samples: usize,
    stencil_size: usize,
) -> Surface {
    let size = window.inner_size();
    let size = (
        size.width.try_into().expect("Could not convert width"),
        size.height.try_into().expect("Could not convert height"),
    );
    let backend_render_target =
        backend_render_targets::make_gl(size, num_samples, stencil_size, fb_info);
    wrap_backend_render_target(
        gr_context,
        &backend_render_target,
        SurfaceOrigin::BottomLeft,
        ColorType::RGBA8888,
        None,
        None,
    )
    .expect("Could not create skia surface")
}
