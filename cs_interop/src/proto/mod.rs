#![allow(unused_imports)]

mod graph {
    include!(concat!(env!("OUT_DIR"), "/graph.rs"));
}

pub use graph::*;