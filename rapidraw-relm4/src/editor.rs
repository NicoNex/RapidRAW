//! Editor canvas: a `gtk::Picture` (wrapped in a `gtk::ScrolledWindow`) that
//! displays the opened base image.
//!
//! This is intentionally *not* a relm4 component. The AppModel is a single
//! `Component`, and the editor page is plain GTK widgets owned by an
//! `EditorCanvas` value that the model holds. The widget is added into the
//! Stack's "editor" page from `init`, and the model talks to the canvas
//! directly (`set_texture`) from `update_cmd`. This integrates more cleanly
//! than nesting a second component just to forward one texture.

use gtk::gdk;
use gtk::prelude::*;

/// Owns the editor-page widget tree (a `ScrolledWindow` containing a
/// `Picture`). The root widget is appended to the Stack's "editor" page;
/// `set_texture` swaps in a newly opened image.
pub struct EditorCanvas {
    /// Outer widget added to the Stack page.
    root: gtk::ScrolledWindow,
    picture: gtk::Picture,
}

impl EditorCanvas {
    pub fn new() -> Self {
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);
        scrolled.set_child(Some(&picture));

        Self {
            root: scrolled,
            picture,
        }
    }

    /// The widget to insert into the Stack's "editor" page.
    pub fn root(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Display a freshly opened image.
    pub fn set_texture(&self, texture: &gdk::MemoryTexture) {
        self.picture.set_paintable(Some(texture));
    }
}

impl Default for EditorCanvas {
    fn default() -> Self {
        Self::new()
    }
}
