#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    dump_core::run();
}
