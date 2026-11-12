use glam::Vec2;
use palantir::{Configure, Menu, Sizing, Ui};

/// Shared open/close lifecycle and look for the canvas's anchored context popups (the node
/// menu and the new-node palette). Owns only the surface-space anchor and the dismiss
/// bookkeeping; each caller stores its own per-open extras (target node, drop position, …).
///
/// Carries what both controllers need: a palantir [`Menu`] at the anchor, whose open state this
/// keeps, in the menu look, and the "a pick or an outside dismiss closes the menu" resolution.
#[derive(Default, Debug)]
pub(crate) struct AnchoredMenu {
    /// The surface-space point the open menu was raised at; palantir's popup keeps the place its
    /// first frame landed it at, so the palette does not move as a query shrinks its list.
    anchor: Option<Vec2>,
}

/// Which canvas popup a menu records as, and how tall it may grow.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PopupShape {
    pub(crate) id_salt: &'static str,
    /// Caps the popup so a tall body scrolls (the new-node palette); `None` hugs the content.
    pub(crate) max_height: Option<f32>,
}

impl AnchoredMenu {
    /// Close the menu without a pick.
    pub(crate) const fn reset(&mut self) {
        self.anchor = None;
    }

    /// Open (or re-anchor) the menu at a surface-space point.
    pub(crate) const fn open_at(&mut self, anchor: Vec2) {
        self.anchor = Some(anchor);
    }

    /// Whether the menu is open, for a caller with per-frame setup to skip: [`Self::show`]
    /// answers `None` for a closed menu only after its arguments have been built.
    pub(crate) const fn is_open(&self) -> bool {
        self.anchor.is_some()
    }

    /// Show the menu when open, recording `body` inside a palantir [`Menu`] shaped by `shape`.
    /// `body` records the items and returns the pick (if any); returning `Some`, or an Esc or
    /// outside-click dismiss, closes the menu. The pick is handed back for the caller to act on.
    pub(crate) fn show<T>(
        &mut self,
        ui: &mut Ui,
        shape: PopupShape,
        body: impl FnOnce(&mut Ui) -> Option<T>,
    ) -> Option<T> {
        let PopupShape {
            id_salt,
            max_height,
        } = shape;
        // `None` for a menu that isn't open.
        let anchor = self.anchor?;
        let mut pick = None;
        let mut menu = Menu::at(anchor)
            .id_salt(id_salt)
            .size((Sizing::HUG, Sizing::HUG));
        if let Some(h) = max_height {
            menu = menu.max_size((f32::INFINITY, h));
        }
        let closed = menu
            .show(ui, |ui| {
                pick = body(ui);
            })
            .is_some_and(|resp| resp.closed());
        // A pick closes the menu through its row or `Ui::close_overlay`, so its input goes back with it.
        debug_assert!(pick.is_none() || closed, "a pick closes the menu");
        if closed {
            self.anchor = None;
        }
        pick
    }
}
