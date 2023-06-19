use crate::app::App;
use crate::base_app::run;

mod app;
mod base_app;

fn main() {
    run::<App>("Window");
}
