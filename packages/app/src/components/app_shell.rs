use crate::Route;
use dioxus::prelude::*;

#[component]
pub fn AppShell() -> Element {
    rsx! {
        div { class: "flex min-h-screen flex-col bg-stone-950 text-stone-100",
            nav { class: "border-b border-stone-800 px-6 py-4",
                Link { class: "text-lg font-semibold", to: Route::Home {}, "Lattis" }
            }
            main { class: "flex flex-1 flex-col items-center justify-center px-6 py-12 text-center", Outlet::<Route> {} }
        }
    }
}
