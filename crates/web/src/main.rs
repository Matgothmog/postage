//! Browser entry point: mounts the Leptos app on `<body>`.

use leptos::prelude::*;

#[component]
fn App() -> impl IntoView {
    view! {
        <main>
            <h1>"Postage"</h1>
            <p>"Placeholder page."</p>
            {bridge_probe()}
        </main>
    }
}

/// Debug builds only: shows that `bridge.js` loaded and, when a Privy app id
/// was configured at build time, that Privy starts and reports its state.
#[cfg(debug_assertions)]
fn bridge_probe() -> impl IntoView {
    use postage_web::bridge::{self, privy::Privy, privy::PrivyConfig};
    use postage_web::config::PRIVY_APP_ID;

    let loaded = bridge::is_loaded();
    let privy = PRIVY_APP_ID.map(|app_id| Privy::start(&PrivyConfig::new(app_id)));
    let privy_status = move || match &privy {
        None => "Privy not configured".to_owned(),
        Some(Err(error)) => format!("Privy failed to start: {error}"),
        Some(Ok(privy)) => format!(
            "Privy ready: {}, signed in: {}",
            privy.ready().get(),
            privy.authenticated().get()
        ),
    };
    view! { <p id="bridge-probe">{format!("SDK bridge loaded: {loaded} · ")} {privy_status}</p> }
}

#[cfg(not(debug_assertions))]
fn bridge_probe() -> impl IntoView {}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
