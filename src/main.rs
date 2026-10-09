mod config;
mod render;

use anyhow::{Context, Result, bail};
use calloop::{
    EventLoop,
    signals::{Signal, Signals},
};
use calloop_wayland_source::WaylandSource;
use config::{Config, MAX_SIZE};
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

const MAX_SCALE: i32 = 16;
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;

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

    let mut text = TextRenderer::new(&config.font, config.lang);
    let size = surface_size(&config, &mut text)?;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        text,
        config,
        size,
        widgets: Vec::new(),
        exit: false,
    };

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
        let Some((width, height, stride)) = buffer_dims(self.width, self.height, self.scale) else {
            eprintln!(
                "буфер {}×{} при scale {} слишком большой, пропускаю кадр",
                self.width, self.height, self.scale
            );
            return;
        };

        let (buffer, canvas) =
            match pool.create_buffer(width, height, stride, wl_shm::Format::Argb8888) {
                Ok(created) => created,
                Err(err) => {
                    eprintln!("не удалось создать буфер: {err}");
                    return;
                }
            };

        text.render(
            canvas,
            width as u32,
            height as u32,
            self.scale as f32,
            config,
        );

        let surface = self.layer.wl_surface();
        surface.set_buffer_scale(self.scale);
        surface.damage_buffer(0, 0, width, height);
        if let Err(err) = buffer.attach_to(surface) {
            eprintln!("не удалось прикрепить буфер: {err}");
            return;
        }
        self.layer.commit();
    }
}

fn buffer_dims(width: u32, height: u32, scale: i32) -> Option<(i32, i32, i32)> {
    if !(1..=MAX_SCALE).contains(&scale) {
        return None;
    }
    let width = i32::try_from(width).ok()?.checked_mul(scale)?;
    let height = i32::try_from(height).ok()?.checked_mul(scale)?;
    let stride = width.checked_mul(4)?;
    let bytes = usize::try_from(stride)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    (width > 0 && height > 0 && bytes <= MAX_BUFFER_BYTES).then_some((width, height, stride))
}

fn surface_size(config: &Config, text: &mut TextRenderer) -> Result<(u32, u32)> {
    let (text_w, text_h) = text.measure(config);
    let side = |fixed: Option<u32>, measured: f32, name: &str| -> Result<u32> {
        if let Some(v) = fixed {
            return Ok(v);
        }
        let v = measured.ceil();
        if !(v.is_finite() && v <= MAX_SIZE as f32) {
            bail!("текст не помещается: {name} {measured} больше {MAX_SIZE}");
        }
        Ok((v as u32).max(1))
    };
    let size = (
        side(config.width, text_w, "ширина")?,
        side(config.height, text_h, "высота")?,
    );
    println!(
        "текст {text_w:.1}×{text_h:.1}, поверхность {}×{}",
        size.0, size.1
    );
    Ok(size)
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

    fn reload(&mut self, qh: &QueueHandle<Self>) {
        let loaded = config::load().and_then(|config| {
            let mut text = TextRenderer::new(&config.font, config.lang);
            let size = surface_size(&config, &mut text)?;
            Ok((config, text, size))
        });
        match loaded {
            Ok((config, text, size)) => {
                self.config = config;
                self.text = text;
                self.size = size;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_dims_normal() {
        assert_eq!(buffer_dims(243, 64, 2), Some((486, 128, 1944)));
        assert_eq!(buffer_dims(1, 1, 1), Some((1, 1, 4)));
    }

    #[test]
    fn buffer_dims_rejects_overflow_and_limits() {
        for (w, h, scale) in [
            (u32::MAX, 1, 1),
            (1, u32::MAX, 1),
            (i32::MAX as u32, 1, 2),
            (600_000_000, 1, 1),
            (100_000, 100_000, 1),
            (8192, 8192, 1),
            (0, 10, 1),
            (10, 0, 1),
            (10, 10, 0),
            (10, 10, -1),
            (10, 10, MAX_SCALE + 1),
        ] {
            assert_eq!(buffer_dims(w, h, scale), None, "{w}×{h} scale {scale}");
        }
    }

    #[test]
    fn surface_size_rejects_too_long_text() {
        let mut config = config::parse("").unwrap();
        let mut text = TextRenderer::new(&config.font, config.lang);
        assert_eq!(surface_size(&config, &mut text).unwrap(), (243, 64));
        config.lines[0].text = "Activate Linux ".repeat(1000);
        assert!(surface_size(&config, &mut text).is_err());
        config.width = Some(500);
        config.lines[0].text = "Activate Linux".into();
        assert_eq!(surface_size(&config, &mut text).unwrap().0, 500);
    }
}
