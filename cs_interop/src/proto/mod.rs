#![allow(unused_imports)]

mod graph;

pub use graph::*;

#[cfg(test)]
mod tests {
    use prost::Message;
    use super::*;

    #[test]
    fn proto_works() {}
}