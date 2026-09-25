//! Generates the Tauri context, which validates `frontendDist=../dist`, so
//! `pnpm vite build` has to have run before any cargo command.

fn main() {
    tauri_build::build()
}
