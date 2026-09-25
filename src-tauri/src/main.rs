//! The desktop binary. Everything it does lives in `radiodiodj_lib`, which is
//! also what the tests link against.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    radiodiodj_lib::run();
}
