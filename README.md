# Rusty lightning sensor

Basically that is practical playground for Rust: idea is to monitor thunderstorms and lights
and show output on wide screen with battery monitoring, now XIAO 7.5" ePaper Panel
as it has esp32c3 inside with an excellent Rust support

All details collected [in specification](doc/specs.md)

The device has a serial console over its USB-C port — how to connect to it, the
command list, and how to keep the port alive while debugging (light sleep takes
it away) are [in the console guide](doc/console.md).
