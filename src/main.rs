mod config;
mod render;

use anyhow::{Context, Result};
use calloop::{
    EventLoop,
    signals::{Signal, Signals},
};
use calloop_wayland_source::WaylandSource;
use config::Config;
use render::TextRenderer;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            KeyboardInteractivity, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};

struct Widget {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    width: u32,
    height: u32,
    scale: i32,
    configured: bool,
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    text: TextRenderer,
    config: Config,
    size: (u32, u32),
    widgets: Vec<Widget>,
    exit: bool,
}

fn main() -> Result<()> {
    let config = config::load()?;

    let conn = Connection::connect_to_env().context("не удалось подключиться к Wayland")?;

    let (globals, event_queue) = registry_queue_init::<App>(&conn)?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("нет wl_compositor")?;
    let shm = Shm::bind(&globals, &qh).context("нет wl_shm")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("нет zwlr_layer_shell_v1 — композитор не поддерживает layer-shell")?;

    let pool = SlotPool::new(4096, &shm).context("не удалось создать shm-пул")?;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        text: TextRenderer::new(),
        config,
        size: (0, 0),
        widgets: Vec::new(),
        exit: false,
    };
    app.update_size();

    let mut event_loop = EventLoop::<App>::try_new().context("не удалось создать цикл событий")?;
    let handle = event_loop.handle();

    WaylandSource::new(conn.clone(), event_queue)
        .insert(handle.clone())
        .map_err(|e| anyhow::anyhow!("не удалось добавить Wayland в цикл: {}", e.error))?;

    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM, Signal::SIGUSR1])?;
    handle
        .insert_source(signals, move |event, _, app| match event.signal() {
            Signal::SIGUSR1 => {
                println!("SIGUSR1: перечитываю конфиг");
                app.reload(&qh);
            }
            sig => {
                println!("{sig:?}: выходим");
                app.exit = true;
            }
        })
        .map_err(|e| anyhow::anyhow!("не удалось добавить сигналы в цикл: {}", e.error))?;

    while !app.exit {
        event_loop.dispatch(None, &mut app)?;
    }

    app.widgets.clear();
    conn.flush()?;
    Ok(())
}

impl Widget {
    fn new(app: &App, qh: &QueueHandle<App>, output: wl_output::WlOutput) -> Result<Self> {
        let surface = app.compositor.create_surface(qh);

        let layer = app.layer_shell.create_layer_surface(
            qh,
            surface,
            app.config.layer(),
            Some("hlactivate"),
            Some(&output),
        );

        let config = &app.config;
        let (width, height) = app.size;
        layer.set_anchor(config.anchor());
        layer.set_size(width, height);
        let [top, right, bottom, left] = config.margin;
        layer.set_margin(top, right, bottom, left);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);

        let empty = Region::new(&app.compositor).context("не удалось создать wl_region")?;
        layer.wl_surface().set_input_region(Some(empty.wl_region()));

        layer.commit();

        Ok(Self {
            output,
            layer,
            width,
            height,
            scale: 1,
            configured: false,
        })
    }

    fn draw(&mut self, pool: &mut SlotPool, text: &mut TextRenderer, config: &Config) {
        let (width, height) = (
            self.width as i32 * self.scale,
            self.height as i32 * self.scale,
        );
        let stride = width * 4;

        let (buffer, canvas) = pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
            .expect("не удалось создать буфер");

        let pixmap = text.render(width as u32, height as u32, self.scale as f32, config);
        render::copy_to_argb8888(&pixmap, canvas);

        let surface = self.layer.wl_surface();
        surface.set_buffer_scale(self.scale);
        surface.damage_buffer(0, 0, width, height);
        buffer
            .attach_to(surface)
            .expect("не удалось прикрепить буфер");
        self.layer.commit();
    }
}

impl App {
    fn output_name(&self, output: &wl_output::WlOutput) -> String {
        self.output_state
            .info(output)
            .and_then(|info| info.name)
            .unwrap_or_else(|| "?".into())
    }

    fn add_widget(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let name = self.output_state.info(&output).and_then(|info| info.name);
        if !self.config.wants_output(name.as_deref()) {
            println!("  пропускаю: нет в outputs");
            return;
        }
        match Widget::new(self, qh, output) {
            Ok(widget) => self.widgets.push(widget),
            Err(err) => eprintln!("не удалось создать надпись: {err:#}"),
        }
    }

    fn update_size(&mut self) {
        let (text_w, text_h) = self.text.measure(&self.config);
        self.size = (
            self.config.width.unwrap_or(text_w.ceil() as u32).max(1),
            self.config.height.unwrap_or(text_h.ceil() as u32).max(1),
        );
        println!(
            "текст {text_w:.1}×{text_h:.1}, поверхность {}×{}",
            self.size.0, self.size.1
        );
    }

    fn reload(&mut self, qh: &QueueHandle<Self>) {
        match config::load() {
            Ok(config) => {
                self.config = config;
                self.update_size();
            }
            Err(err) => {
                eprintln!("{err:#}\nоставляю старый конфиг");
                return;
            }
        }
        self.widgets.clear();
        for output in self.output_state.outputs() {
            self.add_widget(qh, output);
        }
    }

    fn redraw(&mut self, surface: &wl_surface::WlSurface) {
        let Some(widget) = self
            .widgets
            .iter_mut()
            .find(|w| w.layer.wl_surface() == surface)
        else {
            return;
        };
        if widget.configured {
            widget.draw(&mut self.pool, &mut self.text, &self.config);
        }
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        println!("монитор подключён: {}", self.output_name(&output));
        self.add_widget(qh, output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        println!("монитор отключён: {}", self.output_name(&output));
        self.widgets.retain(|w| w.output != output);
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        println!("scale -> {new_factor}");
        if let Some(widget) = self
            .widgets
            .iter_mut()
            .find(|w| w.layer.wl_surface() == surface)
        {
            widget.scale = new_factor;
        }
        self.redraw(surface);
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        println!("closed");
        self.widgets.retain(|w| &w.layer != layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        serial: u32,
    ) {
        println!("configure serial={serial} size={:?}", configure.new_size);
        let Some(widget) = self.widgets.iter_mut().find(|w| &w.layer == layer) else {
            return;
        };
        let (w, h) = configure.new_size;
        if w != 0 {
            widget.width = w;
        }
        if h != 0 {
            widget.height = h;
        }
        widget.configured = true;
        widget.draw(&mut self.pool, &mut self.text, &self.config);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers!(OutputState);
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
