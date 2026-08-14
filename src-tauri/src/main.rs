// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(exit_code) = mendimaru_lib::cli::dispatch_from_env() {
        std::process::exit(exit_code);
    }
    mendimaru_lib::run()
}
