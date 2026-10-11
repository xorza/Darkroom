//! Every entry that plans its memory keeps its live heap within the budget it planned against,
//! and every stage without a plan holds the same heap however much it is given to do.
//!
//! One binary, whose allocator counts the bytes each allocation holds: [`live_heap`]. Each test
//! takes it through a [`session`], runs its entry on synthetic exposures, [`star_field`], and holds
//! the run to its [`budget`].

mod budget;
mod detection;
mod drizzle;
mod image_ops;
mod live_heap;
mod registered;
mod session;
mod stack;
mod star_field;

use crate::live_heap::LiveHeap;

// The allocator a binary uses is a static by the language's rule, and the counts live in it.
#[global_allocator]
static HEAP: LiveHeap = LiveHeap::new();
