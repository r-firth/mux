//! The ground on the GPU.
//!
//! GPUI has no way to run a shader of ours, so on macOS the ground is a
//! Metal layer of its own, in a view set under GPUI's. GPUI's window is made
//! transparent, and wherever it draws nothing the layer shows. The layer
//! follows the display's own frames and nothing of the window is drawn again
//! to move it, so the ground runs at the display's full rate.
//!
//! Here the ground is a panel of round dots, the same dots the session's
//! name is set in: every one always faintly there, and brought up by the
//! light through an ordered dither. The light itself is what `Field` works
//! out, from the same numbers. `Field` stays, with its plainer grain: it
//! lights the gaps an agent's launcher shows the ground through, it is what
//! the tests ask, and it is the ground everywhere this layer cannot be had.
//!
//! This is the one place the app talks to `AppKit` and Metal directly, and so
//! the one place it needs `unsafe`.
#![allow(unsafe_code)]
// The `objc` macros test a `cfg` this crate has never heard of.
#![allow(unexpected_cfgs)]

use super::*;
use metal::{
    CommandQueue, Device, MTLClearColor, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRegion, MTLResourceOptions, MTLStoreAction, MetalLayer, RenderPassDescriptor,
    RenderPipelineDescriptor, RenderPipelineState, Texture, TextureDescriptor,
};
use objc::runtime::{Object, YES};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// How far apart the ground's dots are, in points; how big a dot is, as a
/// share of that, and how much bigger it burns; how bright a dot the light
/// has not reached is, against one it has; and how brightly the light glows
/// on the ground under the dots.
const PITCH: f32 = 10.0 / 3.0;
const DOT_SIZE: f32 = 0.26;
const DOT_SWELL: f32 = 0.2;
const UNLIT: f32 = 0.1;
const WASH: f32 = 0.07;
/// How much the light is sharpened into dark reaches and bright ones, and
/// how much stronger the accent inks show than the field has them.
const CONTRAST: f32 = 1.35;
const ACCENT: f32 = 2.2;

/// The most busy panes and callers the shader lights; more than this at
/// once are not shown.
const MOST: usize = 8;

/// Everything the shader is told for one frame, as rows of four floats so it
/// is laid out the same in Rust and in Metal.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GpuFrame {
    /// The window's width and height in points, a cell's side, and how tall
    /// the strip is.
    view: [f32; 4],
    /// The tide's clock in seconds, whether it runs, what this step adds to
    /// each dot's shimmer number, and how many pixels there are to a point.
    clock: [f32; 4],
    /// How strongly the slow fields show in each ink, and the floor.
    gain: [f32; 4],
    focus: [f32; 4],
    /// The focus light's strength, and whether there is one.
    focus_light: [f32; 4],
    chip: [f32; 4],
    /// Whether there is a chip to light.
    chip_light: [f32; 4],
    /// Where a wake set out from, how far its front has got, how strong.
    wake: [f32; 4],
    /// Where a new ink spreads from, how far it has got, whether one is.
    leaving: [f32; 4],
    /// How many busy panes and callers follow.
    counts: [f32; 4],
    /// Each slow field: where it is and how wide in points, how strong.
    blobs: [[f32; 4]; 8],
    /// Which ink each slow field is in.
    blob_inks: [[f32; 4]; 2],
    busy: [[f32; 4]; MOST],
    busy_light: [[f32; 4]; 2],
    calls: [[f32; 4]; MOST],
    /// Each caller's two rings: how far out and how bright.
    rings: [[f32; 4]; MOST],
    /// Whether each caller's rings are moving.
    ringing: [[f32; 4]; 2],
    /// What each level of each ink adds to the ground, from 0 to 1.
    tones: [[f32; 4]; 16],
    /// The same for an ink dissolving away.
    old_tones: [[f32; 4]; 16],
    ground: [f32; 4],
}

fn rect(rect: layout::Rect) -> [f32; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

fn tones(palette: &Palette) -> [[f32; 4]; 16] {
    let mut tones = [[0.0; 4]; 16];
    for (ink, levels) in palette.0.iter().enumerate() {
        for (level, tone) in levels.iter().enumerate() {
            tones[ink * 4 + level] = [tone[0] / 255.0, tone[1] / 255.0, tone[2] / 255.0, 0.0];
        }
    }
    tones
}

impl Ground {
    /// Move the ground on to `now` for `scene` and say what the shader
    /// needs to light it, and the grain's field too when `field` asks, for
    /// what is still lit here.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn gpu_frame(
        &mut self,
        scene: Scene,
        now: Instant,
        scale: f32,
        field: bool,
    ) -> (GpuFrame, Option<Rc<Grain>>) {
        let laid = self.frame(scene, now);
        let scene = &laid.scene;
        let (width, height) = scene.viewport;
        let span = width.max(height).max(1.0);
        let drift = Self::seconds(laid.drift);
        let saturation = 1.0 - 0.4 * laid.past;
        let gain = Self::gain(&laid);

        let mut frame = GpuFrame {
            view: [width, height, CELL, layout::TAB_BAR_HEIGHT],
            clock: [
                Self::seconds(laid.step),
                if scene.still { 0.0 } else { 1.0 },
                ((laid.step / SHIMMER_STEPS * 29) & 255) as f32,
                scale,
            ],
            gain: [gain[0], gain[1], gain[2], FLOOR],
            counts: [0.0; 4],
            tones: tones(&Palette::new(scene.ink, saturation)),
            ground: [
                GROUND_RGB[0] / 255.0,
                GROUND_RGB[1] / 255.0,
                GROUND_RGB[2] / 255.0,
                1.0,
            ],
            ..GpuFrame::default()
        };
        if let Some((focus, strength)) = Self::focus(scene) {
            frame.focus = rect(focus);
            frame.focus_light = [strength, 1.0, 0.0, 0.0];
        }
        if let Some(chip) = scene.chip {
            frame.chip = rect(chip);
            frame.chip_light = [1.0, 0.0, 0.0, 0.0];
        }
        if let Some((origin, front, strength)) = laid.wake {
            frame.wake = [origin.0, origin.1, front, strength];
        }
        if let Some((from, origin, reached)) = laid.dissolve {
            frame.leaving = [origin.0, origin.1, reached, 1.0];
            frame.old_tones = tones(&Palette::new(from, saturation));
        }
        for (index, blob) in BLOBS.iter().enumerate() {
            let x = (blob.x + blob.drift.0 * (drift * blob.pace + blob.phase).sin()) * width;
            let y = (blob.y + blob.drift.1 * (drift * blob.pace * 0.8 + blob.phase * 1.7).cos())
                * height;
            frame.blobs[index] = [x, y, blob.radius * span, blob.strength];
            frame.blob_inks[index / 4][index % 4] = blob.ink as f32;
        }
        let busy = Self::busy(&laid);
        for (index, &(pane, strength)) in busy.iter().take(MOST).enumerate() {
            frame.busy[index] = rect(pane);
            frame.busy_light[index / 4][index % 4] = strength;
        }
        frame.counts[0] = busy.len().min(MOST) as f32;
        let calls = self.ringing(&laid);
        for (index, call) in calls.iter().take(MOST).enumerate() {
            frame.calls[index] = rect(call.from);
            if let Some([(first, first_bright), (second, second_bright)]) = call.rings {
                frame.rings[index] = [first, first_bright, second, second_bright];
                frame.ringing[index / 4][index % 4] = 1.0;
            }
        }
        frame.counts[1] = calls.len().min(MOST) as f32;

        let lit_here = field.then(|| Rc::new(self.grain(&laid, false)));
        (frame, lit_here)
    }
}

/// The shader, with the ground's numbers written into it.
fn shader() -> String {
    let number = |value: f32| format!("{value:?}");
    [
        ("most", MOST.to_string()),
        ("pitch", number(PITCH)),
        ("dot_size", number(DOT_SIZE)),
        ("dot_swell", number(DOT_SWELL)),
        ("unlit", number(UNLIT)),
        ("wash", number(WASH)),
        ("contrast", number(CONTRAST)),
        ("accent", number(ACCENT)),
        ("tide_in_strip", number(TIDE_IN_STRIP)),
        ("wave_apart", number(TIDE_WAVE.0)),
        ("wave_pace", number(TIDE_WAVE.1)),
        ("cross_apart", number(TIDE_CROSS.0)),
        ("cross_pace", number(TIDE_CROSS.1)),
        ("trough", number(TIDE_TROUGH)),
        ("crest", number(TIDE_CREST)),
        ("wake_trail", number(WAKE_TRAIL)),
        ("focus_reach", number(FOCUS_REACH)),
        ("focus_rim", number(FOCUS_RIM)),
        ("chip_glow", number(CHIP_GLOW)),
        ("busy_reach", number(BUSY_REACH)),
        ("ring_width", number(RING_WIDTH)),
        ("shimmer_share", SHIMMER_SHARE.to_string()),
        ("shimmer", number(SHIMMER)),
        ("fray", number(FRAY)),
    ]
    .into_iter()
    .fold(
        include_str!("ground.metal").to_owned(),
        |source, (name, value)| source.replace(&format!("${name}$"), &value),
    )
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NsRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NsSize {
    width: f64,
    height: f64,
}

/// The ground's layer, and what draws into it.
pub(super) struct Gpu {
    /// The view that hosts the layer; ours, retained until dropped.
    view: *mut Object,
    layer: MetalLayer,
    queue: CommandQueue,
    pipeline: RenderPipelineState,
    noise: Texture,
    buffer: metal::Buffer,
    /// The layer's size in pixels.
    size: (u64, u64),
}

impl Gpu {
    /// Put the ground's layer under `window`'s content, or say why not.
    pub(super) fn attach(window: &Window) -> Result<Self> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow!("no native window: {error}"))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err(anyhow!("not an AppKit window"));
        };
        let content: *mut Object = handle.ns_view.as_ptr().cast();
        let device = Device::system_default().ok_or_else(|| anyhow!("no Metal device"))?;

        let library = device
            .new_library_with_source(&shader(), &metal::CompileOptions::new())
            .map_err(|error| anyhow!("ground shader: {error}"))?;
        let descriptor = RenderPipelineDescriptor::new();
        let vertex = library
            .get_function("ground_vertex", None)
            .map_err(|error| anyhow!("ground shader: {error}"))?;
        let fragment = library
            .get_function("ground_fragment", None)
            .map_err(|error| anyhow!("ground shader: {error}"))?;
        descriptor.set_vertex_function(Some(&vertex));
        descriptor.set_fragment_function(Some(&fragment));
        descriptor
            .color_attachments()
            .object_at(0)
            .ok_or_else(|| anyhow!("no colour attachment"))?
            .set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        let pipeline = device
            .new_render_pipeline_state(&descriptor)
            .map_err(|error| anyhow!("ground pipeline: {error}"))?;

        let texture = TextureDescriptor::new();
        texture.set_pixel_format(MTLPixelFormat::R8Unorm);
        texture.set_width(64);
        texture.set_height(64);
        let noise = device.new_texture(&texture);
        noise.replace_region(
            MTLRegion::new_2d(0, 0, 64, 64),
            0,
            NOISE.as_ptr().cast(),
            64,
        );

        let buffer = device.new_buffer(
            size_of::<GpuFrame>() as u64,
            MTLResourceOptions::StorageModeShared,
        );

        let layer = MetalLayer::new();
        layer.set_device(&device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        layer.set_framebuffer_only(true);
        layer.set_opaque(true);
        layer.set_presents_with_transaction(false);
        // Each frame is shown as soon as it is drawn rather than held for the
        // display, so asking for the next buffer never waits on the last.
        layer.set_display_sync_enabled(false);

        // SAFETY: `content` is GPUI's live content view, on the main thread
        // that owns it, for as long as `window` is borrowed. The view made
        // here is ours: `alloc`/`init` hand it over retained, and `Drop`
        // releases it. Its superview keeps it for as long as it is shown.
        let view = unsafe {
            let parent: *mut Object = msg_send![content, superview];
            if parent.is_null() {
                return Err(anyhow!("the content view has no parent"));
            }
            let frame: NsRect = msg_send![parent, bounds];
            let view: *mut Object = msg_send![class!(NSView), alloc];
            let view: *mut Object = msg_send![view, initWithFrame: frame];
            let layer_object: *mut Object = std::ptr::from_ref::<metal::MetalLayerRef>(&layer)
                .cast_mut()
                .cast();
            // A layer set before `wantsLayer` makes the view host it.
            let _: () = msg_send![view, setLayer: layer_object];
            let _: () = msg_send![view, setWantsLayer: YES];
            // Width and height follow the parent's.
            let _: () = msg_send![view, setAutoresizingMask: 18_u64];
            // Below GPUI's view: -1 is `NSWindowBelow`.
            let _: () =
                msg_send![parent, addSubview: view positioned: -1_isize relativeTo: content];
            view
        };

        Ok(Self {
            view,
            layer,
            queue: device.new_command_queue(),
            pipeline,
            noise,
            buffer,
            size: (0, 0),
        })
    }

    /// Draw one frame of the ground.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(super) fn draw(&mut self, frame: &GpuFrame) {
        let scale = frame.clock[3].max(1.0);
        let size = (
            (frame.view[0] * scale).ceil().max(1.0) as u64,
            (frame.view[1] * scale).ceil().max(1.0) as u64,
        );
        if size != self.size {
            self.size = size;
            #[allow(clippy::cast_precision_loss)]
            let size = NsSize {
                width: size.0 as f64,
                height: size.1 as f64,
            };
            let layer: *mut Object = std::ptr::from_ref::<metal::MetalLayerRef>(&self.layer)
                .cast_mut()
                .cast();
            // SAFETY: the layer is alive, and this is its own setter.
            unsafe {
                let _: () = msg_send![layer, setDrawableSize: size];
            }
        }
        // SAFETY: the buffer was made `size_of::<GpuFrame>()` long and is
        // shared with the GPU; `GpuFrame` is plain floats.
        unsafe {
            std::ptr::copy_nonoverlapping(
                std::ptr::from_ref(frame).cast::<u8>(),
                self.buffer.contents().cast::<u8>(),
                size_of::<GpuFrame>(),
            );
        }
        let Some(drawable) = self.layer.next_drawable() else {
            return;
        };
        let pass = RenderPassDescriptor::new();
        let Some(attachment) = pass.color_attachments().object_at(0) else {
            return;
        };
        attachment.set_texture(Some(drawable.texture()));
        attachment.set_load_action(MTLLoadAction::Clear);
        attachment.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 1.0));
        attachment.set_store_action(MTLStoreAction::Store);
        let commands = self.queue.new_command_buffer();
        let encoder = commands.new_render_command_encoder(pass);
        encoder.set_render_pipeline_state(&self.pipeline);
        encoder.set_fragment_buffer(0, Some(&self.buffer), 0);
        encoder.set_fragment_texture(0, Some(&self.noise));
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 3);
        encoder.end_encoding();
        commands.present_drawable(drawable);
        commands.commit();
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: `view` is the view `attach` made and kept one hold on.
        unsafe {
            let _: () = msg_send![self.view, removeFromSuperview];
            let _: () = msg_send![self.view, release];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_number_is_written_into_the_shader() {
        let source = shader();
        assert!(!source.contains('$'), "a name was left unfilled");
        assert!(source.contains("float4 busy[8];"));
    }

    #[test]
    fn a_frame_is_rows_of_four_floats() {
        assert_eq!(size_of::<GpuFrame>() % 16, 0);
        assert_eq!(align_of::<GpuFrame>(), 4);
    }

    #[test]
    fn a_frame_says_where_the_light_is() {
        let pane = layout::Rect {
            x: 10.0,
            y: 40.0,
            width: 385.0,
            height: 450.0,
        };
        let scene = Scene {
            viewport: (800.0, 500.0),
            ink: Ink::Peach,
            slabs: vec![pane],
            focus: Some(pane),
            bloom: 0.0,
            tab: None,
            chip: None,
            busy: vec![(pane, 1.0)],
            calls: Vec::new(),
            past: false,
            active: true,
            still: false,
        };
        let mut ground = Ground::new(Instant::now());
        let (frame, field) = ground.gpu_frame(scene, Instant::now(), 2.0, false);
        assert!(field.is_none());
        assert!((frame.view[0] - 800.0).abs() < f32::EPSILON);
        assert!(frame.focus_light[1] > 0.5 && frame.chip_light[0] < 0.5);
        assert!((frame.counts[0] - 1.0).abs() < f32::EPSILON);
        assert!(
            frame.clock[1] > 0.5,
            "the tide runs while the window is in front"
        );
        // The tab's own ink at its brightest is not black.
        assert!(frame.tones[3][0] > 0.2);
    }
}
