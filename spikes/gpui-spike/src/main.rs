use gpui::{div, prelude::*, App, Bounds, WindowBounds, WindowOptions, size, px};

struct Hello;

impl Render for Hello {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        div().flex().size_full().items_center().justify_center().child("hello gpu")
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.0), px(260.0)), cx);
        cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |_, cx| cx.new(|_| Hello),
        ).unwrap();
    });
}
