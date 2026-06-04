use dioxus::prelude::*;

#[component]
pub fn Home() -> Element {
    rsx! {
        h1 { class: "text-4xl font-bold tracking-tight", "Lattis" }
        p { class: "mt-4 max-w-xl text-lg leading-8 text-stone-300",
            "A connected space where your devices can work together."
        }
    }
}
