//! Browser entry point: mounts the Leptos app on `<body>`.

use leptos::prelude::*;

#[component]
fn App() -> impl IntoView {
    view! { <main><h1>"Postage"</h1><p>"Placeholder page."</p></main> }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
