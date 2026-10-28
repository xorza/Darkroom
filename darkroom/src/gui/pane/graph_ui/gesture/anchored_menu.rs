use glam::Vec2;
use palantir::{Anchor, ClickOutside, CloseHandle, Configure, Popup, Sizing, Ui};

/// Shared open/close lifecycle + look for the canvas's anchored context
/// popups (the node menu and the new-node palette). Owns only the
/// surface-space anchor and the dismiss bookkeeping; each caller stores its
/// own per-open extras (target node, drop position, …) as plain fields set at
/// open-time and read at pick-time.
///
/// Carries what both controllers need: the Esc-to-close guard, the identical
/// `Popup` look (the `ctx_menu` theme slot's panel, padding, and width
/// floor, hug sizing, click-outside dismiss), and the "a pick or an outside
/// dismiss closes the menu" resolution.
#[derive(Default, Debug)]
pub(crate) struct AnchoredMenu {
    anchor: Option<Place>,
}

/// Where an open menu records. The window clamps a popup each frame, so one anchored at the
/// point it opened at moves as its body grows or shrinks: a palette opened near the bottom edge
/// sat high while it listed the library, and dropped to the point at the first letter typed.
/// So it keeps the place its first frame took.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// The surface-space point the menu opened at, until its first frame lays it out.
    Opened(Vec2),
    /// The top-left corner its first frame landed at.
    Landed(Vec2),
}

/// Which canvas popup a menu records as, and how tall it may grow.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PopupShape {
    pub(crate) id_salt: &'static str,
    /// Caps the popup so a tall body wraps/scrolls (the new-node palette);
    /// `None` hugs the content (the small context menus).
    pub(crate) max_height: Option<f32>,
}

impl AnchoredMenu {
    /// Close the menu without a pick.
    pub(crate) const fn reset(&mut self) {
        self.anchor = None;
    }

    /// Open (or re-anchor) the menu at a surface-space point.
    pub(crate) const fn open_at(&mut self, anchor: Vec2) {
        self.anchor = Some(Place::Opened(anchor));
    }

    /// Whether the menu is open.
    ///
    /// For a caller with per-frame setup to skip: [`Self::show`] answers
    /// `None` for a closed menu anyway, but only *after* its arguments
    /// have been built.
    pub(crate) const fn is_open(&self) -> bool {
        self.anchor.is_some()
    }

    /// Show the menu when open, recording `body` inside the shared popup
    /// look shaped by `shape`. `body` records the items and returns the pick
    /// (if any); returning `Some` — or an Esc / outside-click dismiss — closes
    /// the menu. The pick is handed back for the caller to act on.
    pub(crate) fn show<T>(
        &mut self,
        ui: &mut Ui,
        shape: PopupShape,
        body: impl FnOnce(&mut Ui, &CloseHandle) -> Option<T>,
    ) -> Option<T> {
        let PopupShape {
            id_salt,
            max_height,
        } = shape;
        // `None` for a menu that isn't open.
        let place = self.anchor?;
        let (Place::Opened(anchor) | Place::Landed(anchor)) = place;
        // Esc dismissal is owned by the `Dismiss` popup below (folds into
        // `resp.dismissed`) — no separate Escape poll here.
        //
        // Chrome, padding, and the width floor all come off the same theme
        // slot `CtxMenu::show` reads, so a canvas menu and a menu-bar
        // menu are the same object; these popups only opt out of
        // `CtxMenu` for its per-trigger open lifecycle, not its look.
        let ctx = &ui.theme().context_menu;
        let chrome = ctx.panel.clone();
        let padding = ctx.padding;
        let min_width = ctx.min_width;
        let gap = ctx.gap;
        let mut pick = None;
        let mut popup = Popup::new(Anchor::at_point(anchor))
            .click_outside(ClickOutside::Dismiss)
            .chrome(chrome)
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
            self.anchor = None;
        } else if let Place::Opened(_) = place
            && let Some(rect) = ui.response_for(resp.id).rect
        {
            self.anchor = Some(Place::Landed(rect.min));
        }
        pick
    }
}
