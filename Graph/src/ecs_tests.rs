#[cfg(test)]
pub mod ecs_tests {
    use bevy_ecs::prelude::*;

    #[test]
    fn it_works() {
        #[derive(Debug, PartialEq, Component)]
        struct Position(f32, f32);

        #[derive(Debug, PartialEq, Component)]
        struct Velocity(f32, f32);

        let mut world = World::new();
        let entt0_id = world.spawn((Position(0.0, 0.0), Velocity(1.0, 4.0))).id();
        let _entt1_id = world.spawn((Position(2.0, 5.0), Velocity(0.0, 1.0))).id();
        let entt2_id = world.spawn(Position(5.0, 2.0)).id();

        let mut entt2 = world.entity_mut(entt2_id);
        let pos = entt2.get::<Position>().unwrap();
        let vel = entt2.get::<Velocity>();

        assert_eq!(*pos, Position(5.0, 2.0));
        assert_eq!(vel, None);

        entt2.insert(Velocity(1.0, 6.0));

        let entt2 = world.entity(entt2_id);
        let vel = entt2.get::<Velocity>().unwrap();
        assert_eq!(*vel, Velocity(1.0, 6.0));

        let movement = |mut query: Query<(&mut Position, &Velocity)>| {
            for (mut position, velocity) in &mut query {
                position.0 += velocity.0;
                position.1 += velocity.1;
            }
        };

        let mut schedule = Schedule::default();
        schedule.add_system(movement);
        schedule.run(&mut world);

        let entt0 = world.entity(entt0_id);
        let pos = entt0.get::<Position>().unwrap();
        assert_eq!(*pos, Position(1.0, 4.0));

        let mut query = world.query::<(&mut Position, &Velocity)>();
        for mut q in query.iter_mut(&mut world) {
            q.0.1 += q.1.1;
        }
    }

    #[test]
    pub fn graph() {
        // let world = create_test_world();
        // drop(world);
    }

    // pub fn create_test_world() -> World {
    //     let workspace = Workspace::from_json_file("./test_resources/test_workspace.json");
    //     let mut world = World::new();
    //
    //     #[derive(Bundle)]
    //     struct NodeFunction {
    //         node: Node,
    //         function: Function,
    //     }
    //
    //     for node in workspace.graph().nodes().iter() {
    //         let function = workspace.function_graph().function_by_node_id(node.id()).unwrap();
    //
    //         let _entt_id = world.spawn((node.clone(), function.clone())).id();
    //     }
    //
    //     world
    // }
}
