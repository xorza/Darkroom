use palantir::prelude::*;
use palantir::{Anchor, ClickOutside, CloseHandle};

use crate::gui::pane::graph::gesture::slot::GestureSlot;

/// Shared open/close lifecycle + chrome for the canvas's anchored context
/// popups (the node menu and the new-node palette). Owns
/// only the surface-space anchor and the
/// dismiss bookkeeping; each caller stores its own per-open extras (target
/// node, drop position, …) as plain fields set at open-time and read at
/// pick-time.
///
/// Carries what all three controllers need: the
/// Esc-to-close guard, the identical `Popup` chrome (the `context_menu`
/// theme slot's panel, padding, and width floor, hug sizing,
/// click-outside dismiss), and the "a pick or an outside dismiss closes
/// the menu" resolution.
///
#[derive(Default, Debug)]
pub(crate) struct AnchoredMenu {
    /// The surface-space anchor the menu opened at.
    anchor: GestureSlot<Vec2>,
}

impl AnchoredMenu {
    /// Close the menu without a pick.
    pub(crate) fn reset(&mut self) {
        self.anchor.clear();
    }

    /// Open (or re-anchor) the menu at a surface-space point.
    pub(crate) fn open_at(&mut self, anchor: Vec2) {
        self.anchor.latch(anchor);
    }

    /// Whether the menu is open.
    ///
    /// For a caller with per-frame setup to skip: [`Self::show`] answers
    /// `None` for a closed menu anyway, but only *after* its arguments
    /// have been built.
    pub(crate) fn is_open(&self) -> bool {
        !self.anchor.is_idle()
    }

    /// Show the menu when open, recording `body` inside the shared popup chrome. `body` records the
    /// items and returns the pick (if any); returning `Some` — or an Esc /
    /// outside-click dismiss — closes the menu. The pick is handed back for
    /// the caller to act on. `max_height` caps the popup so a tall body
    /// wraps/scrolls (the new-node palette); `None` hugs the content (the
    /// small context menus).
    pub(crate) fn show<T>(
        &mut self,
        ui: &mut Ui,
        id_salt: &'static str,
        max_height: Option<f32>,
        body: impl FnOnce(&mut Ui, &CloseHandle) -> Option<T>,
    ) -> Option<T> {
        // `None` for a menu that isn't open.
        let anchor = *self.anchor.get()?;
        // Esc dismissal is owned by the `Dismiss` popup below (folds into
        // `resp.dismissed`) — no separate `escape_pressed` here.
        //
        // Chrome, padding, and the width floor all come off the same theme
        // slot `ContextMenu::show` reads, so a canvas menu and a menu-bar
        // menu are the same object; these popups only opt out of
        // `ContextMenu` for its per-trigger open lifecycle, not its look.
        let ctx = &ui.theme().context_menu;
        let chrome = ctx.panel.clone();
        let padding = ctx.padding;
        let min_width = ctx.min_width;
        let gap = ctx.gap;
        let mut pick = None;
        let mut popup = Popup::new(Anchor::at_point(anchor))
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt(id_salt)
            .size((Sizing::HUG, Sizing::HUG))
            .min_size((min_width, 0.0))
            .padding(padding)
            .gap(gap);
        if let Some(h) = max_height {
            popup = popup.max_size((f32::INFINITY, h));
        }
        let resp = popup.show(ui, |ui, popup| {
            pick = body(ui, popup);
        });
        if pick.is_some() || resp.dismissed || resp.close_requested {
            self.anchor.clear();
        }
        pick
    }
}
