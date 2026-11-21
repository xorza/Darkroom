use crate::data::data_type::DataType;
use crate::decl::port_map::{NodeRemap, PortMap, PortMapError};
use crate::decl::ports::Numbered;
use crate::decl::saved_decl::{SavedDecl, SavedEvent, SavedInput, SavedPort};
use crate::identity::PortId;

fn p(id: u32) -> PortId {
    PortId::new(id)
}

/// A map is an injection: an old id twice or a new id twice is refused, a lookup answers the
/// pairs alone, and one map followed by another drops what the second does not name.
#[test]
fn a_port_map_is_an_injection_and_composes() {
    assert_eq!(
        PortMap::checked([(p(1), p(2)), (p(1), p(3))]),
        Err(PortMapError::OldTwice(p(1)))
    );
    assert_eq!(
        PortMap::checked([(p(1), p(3)), (p(2), p(3))]),
        Err(PortMapError::NewTwice(p(3)))
    );
    let first = PortMap::new([(p(2), p(20)), (p(1), p(10))]);
    assert_eq!(first.pairs(), [(p(1), p(10)), (p(2), p(20))]);
    assert_eq!(first.get(p(2)), Some(p(20)));
    assert_eq!(first.get(p(3)), None);
    let second = PortMap::new([(p(10), p(100))]);
    assert_eq!(first.then(&second).pairs(), [(p(1), p(100))]);
}

/// A saved declaration of `inputs`, `outputs` and `events`, each an id, a name and, but for an
/// event, a type.
fn decl(
    inputs: &[(u32, &str, DataType)],
    outputs: &[(u32, &str, DataType)],
    events: &[(u32, &str)],
) -> SavedDecl {
    SavedDecl {
        name: "decl".to_owned(),
        category: String::new(),
        inputs: inputs
            .iter()
            .map(|&(id, name, data_type)| {
                Numbered::new(
                    p(id),
                    SavedInput {
                        name: name.to_owned(),
                        data_type,
                        default_value: None,
                    },
                )
            })
            .collect(),
        outputs: outputs
            .iter()
            .map(|&(id, name, data_type)| {
                Numbered::new(
                    p(id),
                    SavedPort {
                        name: name.to_owned(),
                        data_type,
                    },
                )
            })
            .collect(),
        events: events
            .iter()
            .map(|&(id, name)| {
                Numbered::new(
                    p(id),
                    SavedEvent {
                        name: name.to_owned(),
                    },
                )
            })
            .collect(),
    }
}

/// A proposal pairs a port with the new one of its id and type, else an unpaired one of its name
/// and type, and an event, which has no type, by its id, else its name. `blur` to `sharpen`:
/// `x` 1 goes to `x` 5 by name, as 1 is a `String` there; `y` stays unpaired, as `sharpen` has no
/// `Float` of its name; `out` 1 keeps 1 by id; `tick` 1 keeps 1 by id and `tock` 2 goes to 7 by
/// name.
#[test]
fn a_proposal_pairs_by_id_and_type_then_name_and_type() {
    let blur = decl(
        &[(1, "x", DataType::Int), (2, "y", DataType::Float)],
        &[(1, "out", DataType::Int)],
        &[(1, "tick"), (2, "tock")],
    );
    let sharpen = decl(
        &[(5, "x", DataType::Int), (1, "z", DataType::String)],
        &[(1, "out", DataType::Int)],
        &[(1, "tick"), (7, "tock")],
    );
    let remap = NodeRemap::propose(&blur, &sharpen);
    assert_eq!(remap.inputs.pairs(), [(p(1), p(5))]);
    assert_eq!(remap.outputs.pairs(), [(p(1), p(1))]);
    assert_eq!(remap.events.pairs(), [(p(1), p(1)), (p(2), p(7))]);

    // A name in the other type pairs nothing: `y` beside a `String` `y`.
    let retyped = decl(&[(3, "y", DataType::String)], &[], &[]);
    assert!(
        NodeRemap::propose(&blur, &retyped)
            .inputs
            .pairs()
            .is_empty()
    );
}
