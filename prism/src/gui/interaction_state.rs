//! Graph UI interaction state management.
//!
//! This module centralizes the tracking of UI interaction modes and related state,
//! separating concerns between domain logic and UI rendering.

use egui::Pos2;

use crate::gui::connection_breaker::ConnectionBreaker;

/// High-level interaction mode for the graph UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InteractionMode {
    /// No active interaction - ready for any user input.
    #[default]
    Idle,
    /// User is drawing a line to break connections by crossing them.
    BreakingConnections,
    /// User is dragging from a port to create a new connection.
    DraggingNewConnection,
    /// User is panning the graph view with middle mouse button.
    PanningGraph,
}

/// Centralized UI interaction state for the graph editor.
///
/// This struct tracks the current interaction mode and related state that affects
/// how the UI renders and responds to input. It separates UI state from domain logic,
/// making it easier to reason about interaction behavior.
#[derive(Debug, Default)]
pub struct GraphInteractionState {
    /// Current interaction mode.
    mode: InteractionMode,

    /// Connection breaker tool for breaking connections by crossing them.
    connection_breaker: ConnectionBreaker,
}

impl GraphInteractionState {
    /// Returns the current interaction mode.
    pub fn mode(&self) -> InteractionMode {
        self.mode
    }

    /// Returns true if the UI is in idle mode (no active interaction).
    pub fn is_idle(&self) -> bool {
        self.mode == InteractionMode::Idle
    }

    /// Returns true if the user is currently breaking connections.
    pub fn is_breaking_connections(&self) -> bool {
        self.mode == InteractionMode::BreakingConnections
    }

    /// Returns true if the user is dragging a new connection.
    pub fn is_dragging_connection(&self) -> bool {
        self.mode == InteractionMode::DraggingNewConnection
    }

    /// Returns true if the user is panning the graph.
    pub fn is_panning(&self) -> bool {
        self.mode == InteractionMode::PanningGraph
    }

    /// Returns a reference to the connection breaker if breaking mode is active.
    pub fn breaker(&self) -> Option<&ConnectionBreaker> {
        self.is_breaking_connections()
            .then_some(&self.connection_breaker)
    }

    /// Returns a mutable reference to the connection breaker (regardless of mode).
    pub fn breaker_mut(&mut self) -> &mut ConnectionBreaker {
        &mut self.connection_breaker
    }

    /// Transitions to idle mode, resetting all interaction state.
    pub fn reset_to_idle(&mut self) {
        self.transition_to(InteractionMode::Idle);
    }

    /// Transitions to breaking connections mode.
    pub fn start_breaking(&mut self, start_pos: Pos2) {
        self.transition_to(InteractionMode::BreakingConnections);
        self.connection_breaker.start(start_pos);
    }

    /// Adds a point to the connection breaker line.
    /// Only valid when in breaking connections mode.
    pub fn add_breaker_point(&mut self, pos: Pos2) {
        assert!(
            self.is_breaking_connections(),
            "add_breaker_point called outside breaking mode"
        );
        self.connection_breaker.add_point(pos);
    }

    /// Transitions to dragging new connection mode.
    pub fn start_dragging_connection(&mut self) {
        self.transition_to(InteractionMode::DraggingNewConnection);
    }

    /// Transitions to panning mode.
    pub fn start_panning(&mut self) {
        self.transition_to(InteractionMode::PanningGraph);
    }

    /// Transitions to a specific mode, resetting breaker state.
    pub fn transition_to(&mut self, mode: InteractionMode) {
        tracing::info!("Graph UI transitioning to {:?}", mode);
        self.mode = mode;
        self.connection_breaker.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_idle() {
        let state = GraphInteractionState::default();
        assert!(state.is_idle());
        assert_eq!(state.mode(), InteractionMode::Idle);
    }

    #[test]
    fn transition_to_breaking() {
        let mut state = GraphInteractionState::default();
        state.start_breaking(Pos2::new(10.0, 20.0));
        assert!(state.is_breaking_connections());
        assert!(state.breaker().is_some());
    }

    #[test]
    fn reset_to_idle_clears_mode() {
        let mut state = GraphInteractionState::default();
        state.start_breaking(Pos2::new(10.0, 20.0));
        state.reset_to_idle();
        assert!(state.is_idle());
        assert!(state.breaker().is_none());
    }

    #[test]
    fn breaker_only_available_in_breaking_mode() {
        let mut state = GraphInteractionState::default();
        assert!(state.breaker().is_none());

        state.start_dragging_connection();
        assert!(state.breaker().is_none());

        state.start_panning();
        assert!(state.breaker().is_none());

        state.start_breaking(Pos2::ZERO);
        assert!(state.breaker().is_some());
    }
}
