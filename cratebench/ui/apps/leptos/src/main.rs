use leptos::prelude::*;

fn main() {
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let (count, set_count) = signal(0);
    view! {
        <h1>"Count: " {count}</h1>
        <button on:click=move |_| set_count.update(|n| *n += 1)>"+1"</button>
    }
}
