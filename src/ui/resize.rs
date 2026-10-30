//! Details divider. Drag coordinates stay anchored to the initial press even
//! as the divider moves; retained state is local to each window's widget tree.
use super::{Message, fluent::*};
use iced::advanced::Renderer as _;
use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, renderer,
        widget::{Operation, Tree, operation::Focusable, tree},
    },
    keyboard::{self, key::Named},
    widget::{text, tooltip},
    window,
};

pub fn maximum_width(available: f32) -> f32 {
    (available - LIBRARY_MIN_WIDTH - DETAILS_RESIZE_WIDTH)
        .clamp(DETAILS_MIN_WIDTH, DETAILS_MAX_WIDTH)
}

pub fn handle<'a>(pending: bool, width: f32, maximum: f32, enabled: bool) -> Element<'a, Message> {
    let enabled = enabled && maximum > DETAILS_MIN_WIDTH;
    tooltip(
        Element::new(ResizeHandle {
            pending,
            width,
            maximum,
            enabled,
        }),
        text(if enabled {
            "拖拽调整详情宽度；聚焦后用 ← / → 微调，Home 还原"
        } else {
            "窗口较窄，放大窗口后可调整详情宽度"
        })
        .size(12)
        .width(220)
        .wrapping(text::Wrapping::Glyph),
        tooltip::Position::Top,
    )
    .style(panel)
    .padding(8)
    .into()
}

struct ResizeHandle {
    pending: bool,
    width: f32,
    maximum: f32,
    enabled: bool,
}

#[derive(Default)]
struct State {
    drag: Option<Drag>,
    focused: bool,
    hovered: bool,
}

struct Drag {
    x: f32,
    width: f32,
}

impl Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }
    fn focus(&mut self) {
        self.focused = true;
    }
    fn unfocus(&mut self) {
        self.focused = false;
    }
}

impl ResizeHandle {
    fn handle_event(
        &self,
        state: &mut State,
        event: &Event,
        cursor: mouse::Cursor,
        bounds: Rectangle,
        shell: &mut Shell<'_, Message>,
    ) {
        let before = (state.hovered, state.focused, state.drag.is_some());
        state.hovered = self.enabled && cursor.is_over(bounds);
        if !self.enabled {
            *state = State::default();
        } else {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    state.focused = state.hovered;
                    state.drag = cursor
                        .position()
                        .filter(|_| state.hovered)
                        .map(|point| Drag {
                            x: point.x,
                            width: self.width,
                        });
                    if state.drag.is_some() {
                        shell.capture_event();
                    }
                }
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    if let Some(drag) = &state.drag {
                        self.publish(drag.width + drag.x - position.x, shell);
                        shell.capture_event();
                    }
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    if state.drag.take().is_some() {
                        shell.capture_event();
                    }
                }
                Event::Mouse(mouse::Event::CursorLeft) => {
                    state.drag = None;
                    state.hovered = false;
                }
                Event::Window(window::Event::Unfocused) => *state = State::default(),
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(key),
                    modifiers,
                    ..
                }) if state.focused && !modifiers.control() && !modifiers.alt() => {
                    let width = match key {
                        Named::ArrowLeft => Some(self.width + 10.0),
                        Named::ArrowRight => Some(self.width - 10.0),
                        Named::Home => Some(DETAILS_DEFAULT_WIDTH),
                        Named::End => Some(self.maximum),
                        Named::Escape => {
                            state.focused = false;
                            state.drag = None;
                            shell.capture_event();
                            None
                        }
                        _ => None,
                    };
                    if let Some(width) = width {
                        self.publish(width, shell);
                        shell.capture_event();
                    }
                }
                _ => {}
            }
        }
        if before != (state.hovered, state.focused, state.drag.is_some()) {
            shell.request_redraw();
        }
    }

    fn publish(&self, width: f32, shell: &mut Shell<'_, Message>) {
        let width = width.clamp(DETAILS_MIN_WIDTH, self.maximum);
        if width != self.width {
            shell.publish(Message::ResizeDetails(self.pending, width));
        }
    }
}

impl Widget<Message, Theme, Renderer> for ResizeHandle {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }
    fn diff(&self, tree: &mut Tree) {
        if !self.enabled {
            *tree.state.downcast_mut::<State>() = State::default();
        }
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(DETAILS_RESIZE_WIDTH), Length::Fill)
    }
    fn layout(&mut self, _: &mut Tree, _: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.resolve(self.size().width, Length::Fill, Size::ZERO))
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.enabled {
            operation.focusable(None, layout.bounds(), tree.state.downcast_mut::<State>());
        }
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _: &Renderer,
        _: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let cursor = if cursor.is_over(*viewport) {
            cursor
        } else {
            mouse::Cursor::Unavailable
        };
        self.handle_event(
            tree.state.downcast_mut::<State>(),
            event,
            cursor,
            layout.bounds(),
            shell,
        );
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if !bounds.intersects(viewport) {
            return;
        }
        let state = tree.state.downcast_ref::<State>();
        let active = state.hovered || state.focused || state.drag.is_some();
        let colors = palette(theme);
        let thickness = if active { 3.0 } else { 2.0 };
        let height = bounds.height.min(40.0);
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(
                    Point::new(
                        bounds.center_x() - thickness / 2.0,
                        bounds.center_y() - height / 2.0,
                    ),
                    Size::new(thickness, height),
                ),
                border: iced::border::rounded(thickness / 2.0),
                ..Default::default()
            },
            if active {
                colors.accent
            } else if self.enabled {
                colors.muted
            } else {
                colors.line
            },
        );
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        _: &Renderer,
    ) -> mouse::Interaction {
        if self.enabled
            && (tree.state.downcast_ref::<State>().drag.is_some()
                || (cursor.is_over(layout.bounds()) && cursor.is_over(*viewport)))
        {
            mouse::Interaction::ResizingHorizontally
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn divider() -> ResizeHandle {
        ResizeHandle {
            pending: true,
            width: 360.0,
            maximum: 600.0,
            enabled: true,
        }
    }

    fn send(handle: &ResizeHandle, state: &mut State, event: Event, x: f32) -> Vec<Message> {
        let mut messages = Vec::new();
        handle.handle_event(
            state,
            &event,
            mouse::Cursor::Available(Point::new(x, 100.0)),
            Rectangle::new(Point::new(700.0, 0.0), Size::new(16.0, 500.0)),
            &mut Shell::new(&mut messages),
        );
        messages
    }

    fn moved(x: f32) -> Event {
        Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(x, 100.0),
        })
    }

    fn key(key: Named, modifiers: keyboard::Modifiers) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(key),
            modified_key: keyboard::Key::Named(key),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        })
    }

    #[test]
    fn keyboard_adjustment_requires_focus_and_preserves_shortcuts() {
        let handle = divider();
        let mut state = State::default();
        assert!(
            send(
                &handle,
                &mut state,
                key(Named::ArrowLeft, keyboard::Modifiers::empty()),
                500.0
            )
            .is_empty()
        );
        state.focus();
        for (pressed, expected) in [
            (Named::ArrowLeft, 370.0),
            (Named::ArrowRight, 350.0),
            (Named::Home, 300.0),
            (Named::End, 600.0),
        ] {
            let messages = send(
                &handle,
                &mut state,
                key(pressed, keyboard::Modifiers::empty()),
                500.0,
            );
            assert!(
                matches!(messages.as_slice(), [Message::ResizeDetails(true, width)] if *width == expected)
            );
        }
        assert!(
            send(
                &handle,
                &mut state,
                key(Named::Home, keyboard::Modifiers::CTRL),
                500.0
            )
            .is_empty()
        );
        send(
            &handle,
            &mut state,
            key(Named::Escape, keyboard::Modifiers::empty()),
            500.0,
        );
        assert!(!state.is_focused());
    }

    #[test]
    fn window_limits_preserve_library_space_and_preferred_width() {
        for (window, navigation, expected) in [(1060.0, 216.0, 300.0), (960.0, 0.0, 416.0)] {
            let available = window - navigation - 48.0;
            let maximum = maximum_width(available);
            assert_eq!(maximum, expected);
            assert!(available - maximum - DETAILS_RESIZE_WIDTH >= LIBRARY_MIN_WIDTH);
        }
        let preferred: f32 = 580.0;
        assert_eq!(
            preferred.clamp(DETAILS_MIN_WIDTH, maximum_width(796.0)),
            300.0
        );
        assert_eq!(
            preferred.clamp(DETAILS_MIN_WIDTH, maximum_width(1116.0)),
            preferred
        );
        assert_eq!(maximum_width(2400.0), DETAILS_MAX_WIDTH);
    }

    #[test]
    fn drag_tracks_original_press_outside_handle_and_clamps_both_ends() {
        let mut handle = divider();
        let mut state = State::default();
        assert!(send(&handle, &mut state, moved(650.0), 650.0).is_empty());
        send(
            &handle,
            &mut state,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            708.0,
        );
        assert!(matches!(
            send(&handle, &mut state, moved(650.0), 650.0).as_slice(),
            [Message::ResizeDetails(true, 418.0)]
        ));
        handle.width = 418.0;
        assert!(matches!(
            send(&handle, &mut state, moved(620.0), 620.0).as_slice(),
            [Message::ResizeDetails(true, 448.0)]
        ));
        assert!(matches!(
            send(&handle, &mut state, moved(0.0), 0.0).as_slice(),
            [Message::ResizeDetails(true, 600.0)]
        ));
        assert!(matches!(
            send(&handle, &mut state, moved(1500.0), 1500.0).as_slice(),
            [Message::ResizeDetails(true, 300.0)]
        ));
    }

    #[test]
    fn release_focus_loss_and_window_exit_end_drag() {
        let handle = divider();
        for end in [
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            Event::Window(window::Event::Unfocused),
            Event::Mouse(mouse::Event::CursorLeft),
        ] {
            let mut state = State::default();
            send(
                &handle,
                &mut state,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                708.0,
            );
            assert!(state.drag.is_some());
            send(&handle, &mut state, end, 600.0);
            assert!(state.drag.is_none());
            assert!(send(&handle, &mut state, moved(500.0), 500.0).is_empty());
        }
    }

    #[test]
    fn hiding_or_disabling_divider_clears_drag_and_focus() {
        let mut handle = divider();
        let mut state = State::default();
        send(
            &handle,
            &mut state,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            708.0,
        );
        let mut tree = Tree {
            tag: handle.tag(),
            state: tree::State::new(state),
            children: vec![],
        };
        handle.enabled = false;
        handle.diff(&mut tree);
        let state = tree.state.downcast_mut::<State>();
        assert!(!state.focused && state.drag.is_none());
        assert!(send(&handle, state, moved(500.0), 500.0).is_empty());
    }
}
