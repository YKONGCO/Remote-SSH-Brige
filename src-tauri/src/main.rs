#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    remote_ssh_brige_lib::run();
}
