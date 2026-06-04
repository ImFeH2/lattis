use dioxus::prelude::*;

mod components;
mod views;

use components::AppShell;
use views::Home;

const APP_CSS: Asset = asset!("/assets/app.css");

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(AppShell)]
        #[route("/")]
        Home {},
}

#[component]
pub fn App() -> Element {
    rsx! {
        document::Stylesheet { href: APP_CSS }
        Router::<Route> {}
    }
}
