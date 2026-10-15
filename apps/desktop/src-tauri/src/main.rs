// No console window in release builds on Windows. Debug builds keep it for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    meet_assist_lib::run()
}
