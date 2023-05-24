#[cfg(test)]
mod ecs_tests {
    use crate::ecs_tests::ecs_tests::create_test_world;
    use crate::serialization_system::Serialization;

    #[test]
    fn it_works() {
        let mut world = create_test_world();

        let serialization = Serialization::new();
        serialization.run(&mut world);
    }
}