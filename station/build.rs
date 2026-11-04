//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
//! Hand the ESP-IDF link arguments that esp-idf-sys computed to this binary.
fn main() {
    embuild::espidf::sysenv::output();
}
