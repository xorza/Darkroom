//! Shared fixtures for execution-layer unit tests.

use std::ops::Deref;
use std::sync::Arc;

use crate::execution::compiled::CompiledGraph;
use crate::execution::program::Program;

/// Convenience owner for tests that hand-build a [`Program`] but still install
/// the same outer artifact production uses.
#[derive(Debug)]
pub(crate) struct TestCompiledGraph {
    compiled: Arc<CompiledGraph>,
}

impl TestCompiledGraph {
    pub(crate) fn new(program: Program) -> Self {
        Self {
            compiled: Arc::new(CompiledGraph::from_program_for_test(program)),
        }
    }

    pub(crate) fn program_mut(&mut self) -> &mut Program {
        &mut Arc::get_mut(&mut self.compiled)
            .expect("the fixture is built before its artifact is shared")
            .program
    }

    pub(crate) fn shared(&self) -> &Arc<CompiledGraph> {
        &self.compiled
    }
}

impl Default for TestCompiledGraph {
    fn default() -> Self {
        Self::new(Program::default())
    }
}

impl Deref for TestCompiledGraph {
    type Target = Program;

    fn deref(&self) -> &Self::Target {
        &self.compiled.program
    }
}
