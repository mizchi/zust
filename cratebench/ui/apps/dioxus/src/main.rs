use dioxus::prelude::*;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut count = use_signal(|| 0);
    rsx! {
        h1 { "Count: {count}" }
        button { onclick: move |_| count += 1, "+1" }
    }
}
