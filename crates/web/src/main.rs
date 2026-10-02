//! Browser entry point: mounts the Leptos app on `<body>`.

use leptos::prelude::*;
use postage_web::app::App;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
