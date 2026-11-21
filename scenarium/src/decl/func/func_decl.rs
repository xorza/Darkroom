//! [`FuncDecl`]: the one field list of a func declaration, which a
//! [`FuncBuilder`](crate::FuncBuilder) fills and a [`Func`](crate::Func) reads.

use crate::data::data_type::DataType;
use crate::decl::cache_mode::CacheMode;
use crate::decl::func::FuncBehavior;
use crate::decl::func::error::FuncValidationError;
use crate::decl::func::event_lambda::EventLambda;
use crate::decl::func::func_lambda::FuncLambda;
use crate::decl::ports::error::PortList;
use crate::decl::ports::{EventDecl, Numbered, Ports, check_ids};
use crate::identity::FuncId;

/// What a func declares: its identity, how it runs and the ports it shows. A builder writes it
/// through its setters alone, and a func reads it, so the list is written once.
#[derive(Clone, Debug)]
pub(crate) struct FuncDecl {
    pub(crate) id: FuncId,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) sink: bool,
    /// The [`CacheMode`] a new node of this func copies; existing nodes keep their own.
    pub(crate) default_cache_mode: CacheMode,
    pub(crate) behavior: FuncBehavior,
    pub(crate) description: Option<String>,
    pub(crate) ports: Ports,
    pub(crate) events: Vec<Numbered<EventDecl>>,
    /// One per event, in `events`' order: what fires each.
    pub(crate) event_lambdas: Vec<EventLambda>,
    pub(crate) lambda: FuncLambda,
}

impl FuncDecl {
    /// Check this declaration apart from any graph: the rules every declaration's ports pass
    /// ([`Ports::check`]), and a func's own, an id, an output or a sink for a pure func, no `Any`
    /// output and each event's number.
    pub(crate) fn check(&self) -> Result<(), FuncValidationError> {
        assert_eq!(
            self.events.len(),
            self.event_lambdas.len(),
            "a lambda fires each event"
        );
        if self.id.is_nil() {
            return Err(FuncValidationError::NilId);
        }
        let func_id = self.id;
        if self.ports.outputs.is_empty() && !self.sink && self.behavior != FuncBehavior::Impure {
            return Err(FuncValidationError::PureWithoutEffect { func_id });
        }
        self.ports
            .check()
            .and_then(|()| check_ids(self.events.iter().map(|event| event.id), PortList::Events))
            .map_err(|error| FuncValidationError::Ports { func_id, error })?;
        if let Some(output_idx) = self
            .ports
            .outputs
            .iter()
            .position(|output| output.data_type == DataType::Any)
        {
            return Err(FuncValidationError::AnyOutput {
                func_id,
                output_idx,
            });
        }
        Ok(())
    }
}
