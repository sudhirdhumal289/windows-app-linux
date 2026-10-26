//! Widget that displays the remote desktop (or one monitor's part of it), stretched to its
//! allocation.

use crate::rdp::input::{map_coords, map_viewport_coords};
use crate::rdp::{Frame, Viewport};
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene, gsk, prelude::*};

type ResizeHandler = Box<dyn Fn(i32, i32)>;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct RdpCanvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub viewport: Cell<Option<Viewport>>,
        /// Show the whole frame when it doesn't contain the viewport (e.g. while the host
        /// is still applying a new layout); otherwise such frames show black.
        pub full_frame_fallback: Cell<bool>,
        pub on_resize: RefCell<Option<ResizeHandler>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RdpCanvas {
        const NAME: &'static str = "WindowsAppRdpCanvas";
        type Type = super::RdpCanvas;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for RdpCanvas {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_focusable(true);
            obj.set_focus_on_click(true);
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_overflow(gtk4::Overflow::Hidden);
            self.full_frame_fallback.set(true);
        }
    }

    impl WidgetImpl for RdpCanvas {
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            snapshot.append_color(&gdk::RGBA::BLACK, &bounds);
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let frame = (texture.width() as u32, texture.height() as u32);
            match self.viewport.get() {
                Some(vp) if vp.fits(frame) => {
                    // Scale the whole texture so that the viewport fills the widget.
                    let sx = w / vp.width as f32;
                    let sy = h / vp.height as f32;
                    let rect = graphene::Rect::new(
                        -(vp.x as f32) * sx,
                        -(vp.y as f32) * sy,
                        frame.0 as f32 * sx,
                        frame.1 as f32 * sy,
                    );
                    snapshot.push_clip(&bounds);
                    snapshot.append_scaled_texture(&texture, gsk::ScalingFilter::Linear, &rect);
                    snapshot.pop();
                }
                Some(_) if !self.full_frame_fallback.get() => {}
                _ => snapshot.append_scaled_texture(&texture, gsk::ScalingFilter::Linear, &bounds),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if let Some(on_resize) = self.on_resize.borrow().as_ref() {
                on_resize(width, height);
            }
        }
    }
}

glib::wrapper! {
    pub struct RdpCanvas(ObjectSubclass<imp::RdpCanvas>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for RdpCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl RdpCanvas {
    /// A canvas for a secondary monitor: it stays black unless the frame contains its
    /// viewport.
    pub fn secondary() -> Self {
        let canvas = Self::default();
        canvas.imp().full_frame_fallback.set(false);
        canvas
    }

    /// Converts `frame` into a texture that can be shown by several canvases.
    pub fn texture_for(frame: &Frame) -> Option<gdk::Texture> {
        let (Ok(width), Ok(height)) = (i32::try_from(frame.width), i32::try_from(frame.height))
        else {
            return None;
        };
        if width == 0 || height == 0 || frame.pixels.is_empty() {
            return None;
        }
        let bytes = glib::Bytes::from(frame.pixels.as_slice());
        let texture = gdk::MemoryTexture::new(
            width,
            height,
            gdk::MemoryFormat::B8g8r8x8,
            &bytes,
            frame.stride as usize,
        );
        Some(texture.upcast())
    }

    /// Replaces the displayed image.
    pub fn set_texture(&self, texture: &gdk::Texture) {
        self.imp().texture.replace(Some(texture.clone()));
        self.queue_draw();
    }

    /// Shows only `viewport` of the remote desktop (`None`: all of it).
    pub fn set_viewport(&self, viewport: Option<Viewport>) {
        if self.imp().viewport.replace(viewport) != viewport {
            self.queue_draw();
        }
    }

    /// Maps a widget position to remote desktop coordinates.
    pub fn to_remote(&self, x: f64, y: f64, frame_size: (u32, u32)) -> (u16, u16) {
        let widget = (f64::from(self.width()), f64::from(self.height()));
        match self.imp().viewport.get() {
            Some(vp) => map_viewport_coords(x, y, widget, vp, frame_size),
            None => map_coords(x, y, widget, frame_size),
        }
    }

    /// Calls `f` with the new size (logical pixels) after every allocation.
    pub fn connect_resized(&self, f: impl Fn(i32, i32) + 'static) {
        self.imp().on_resize.replace(Some(Box::new(f)));
    }
}
