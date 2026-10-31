//! Anchored menus: overlay the list without changing its layout.
use super::{Message, fluent};
use iced::advanced::Renderer as _;
use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, operation::Focusable, tree},
    },
    keyboard::{self, key::Named},
};

pub(super) fn tag_menu<'a>(
    trigger: impl Into<Element<'a, Message>>,
    menu: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(Dropdown {
        trigger: trigger.into(),
        menu,
        kind: Kind::Tags,
        enabled: true,
    })
}

pub(super) fn video_menu<'a>(
    trigger: impl Into<Element<'a, Message>>,
    menu: Option<Element<'a, Message>>,
    pending: bool,
    video_id: i64,
    enabled: bool,
) -> Element<'a, Message> {
    Element::new(Dropdown {
        trigger: trigger.into(),
        menu,
        kind: Kind::Video { pending, video_id },
        enabled,
    })
}

#[derive(Clone, Copy)]
enum Kind {
    Tags,
    Video { pending: bool, video_id: i64 },
}

impl Kind {
    fn id(self) -> String {
        match self {
            Self::Tags => "tag-filter".into(),
            Self::Video { pending, video_id } => super::selection::menu_id(pending, video_id),
        }
    }

    fn open(self, open: bool) -> Message {
        match self {
            Self::Tags => Message::TagFilterOpen(open),
            Self::Video { pending, video_id } => {
                Message::ContextMenu(pending, open.then_some(video_id))
            }
        }
    }

    fn key(self, key: Named, reverse: bool) -> Message {
        match self {
            Self::Tags => Message::TagFilterKey(key, reverse),
            Self::Video { pending, .. } => Message::ContextMenuKey(pending, key, reverse),
        }
    }

    fn width(self) -> f32 {
        match self {
            Self::Tags => 380.0,
            Self::Video { .. } => 260.0,
        }
    }

    fn capture_context_press(
        self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        viewport: Rectangle,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        if matches!(self, Self::Video { .. })
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
            )
            && cursor.is_over(bounds)
            && cursor.is_over(viewport)
        {
            state.focused = true;
            state.context_anchor = cursor.position().map(|point| point - bounds.position());
            shell.publish(self.open(true));
            // Do not forward right-clicks to row buttons or checkboxes.
            shell.capture_event();
            return true;
        }
        false
    }
}

struct Dropdown<'a> {
    trigger: Element<'a, Message>,
    menu: Option<Element<'a, Message>>,
    kind: Kind,
    enabled: bool,
}

#[derive(Default)]
struct State {
    focused: bool,
    context_anchor: Option<Vector>,
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

impl Widget<Message, Theme, Renderer> for Dropdown<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }
    fn children(&self) -> Vec<Tree> {
        std::iter::once(&self.trigger)
            .chain(self.menu.iter())
            .map(Tree::new)
            .collect()
    }
    fn diff(&self, tree: &mut Tree) {
        let children: Vec<_> = std::iter::once(&self.trigger)
            .chain(self.menu.iter())
            .collect();
        tree.diff_children(&children);
    }
    fn size(&self) -> Size<Length> {
        self.trigger.as_widget().size()
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.trigger
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.enabled {
            operation.focusable(
                Some(&self.kind.id().into()),
                layout.bounds(),
                tree.state.downcast_mut::<State>(),
            );
        }
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        if !self.enabled {
            state.focused = false;
            return;
        }
        if self
            .kind
            .capture_context_press(state, event, layout.bounds(), cursor, *viewport, shell)
        {
            return;
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.focused = cursor.is_over(layout.bounds()) && cursor.is_over(*viewport);
            }
            Event::Window(iced::window::Event::Unfocused) => state.focused = false,
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key),
                modifiers,
                ..
            }) if state.focused && !modifiers.control() && !modifiers.alt() => {
                if let Kind::Video { pending, video_id } = self.kind {
                    if *key == Named::ContextMenu || (*key == Named::F10 && modifiers.shift()) {
                        state.context_anchor = None;
                        shell.publish(self.kind.open(true));
                        shell.capture_event();
                        return;
                    }
                    if matches!(key, Named::Enter | Named::Space) {
                        shell.publish(Message::Select(pending, video_id));
                        shell.capture_event();
                        return;
                    }
                }
                if matches!(self.kind, Kind::Tags)
                    && matches!(key, Named::Enter | Named::Space | Named::ArrowDown)
                {
                    shell.publish(self.kind.open(true));
                    shell.capture_event();
                    return;
                }
                if *key == Named::Escape && matches!(self.kind, Kind::Tags) {
                    state.focused = false;
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
            _ => {}
        }
        self.trigger.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if self.enabled && tree.state.downcast_ref::<State>().focused {
            let focus = fluent::focus_surface(theme);
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds().shrink(1.0),
                    border: focus.border,
                    ..Default::default()
                },
                fluent::palette(theme).tint,
            );
        }
        self.trigger.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.trigger.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        _: &Renderer,
        _: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let content = self.menu.as_mut()?;
        let mut anchor = layout.bounds() + translation;
        if let Some(offset) = tree.state.downcast_ref::<State>().context_anchor {
            anchor = Rectangle::new(anchor.position() + offset, Size::ZERO);
        }
        Some(overlay::Element::new(Box::new(Menu {
            content,
            tree: &mut tree.children[1],
            anchor,
            kind: self.kind,
        })))
    }
}

struct Menu<'a, 'b> {
    content: &'a mut Element<'b, Message>,
    tree: &'a mut Tree,
    anchor: Rectangle,
    kind: Kind,
}

impl overlay::Overlay<Message, Theme, Renderer> for Menu<'_, '_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let below = (bounds.height - self.anchor.y - self.anchor.height - 12.0).max(0.0);
        let above = (self.anchor.y - 12.0).max(0.0);
        let max_height = below.max(above).min(360.0);
        let width = self.kind.width().min((bounds.width - 16.0).max(0.0));
        let mut node = self.content.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width, max_height)),
        );
        let x = (if matches!(self.kind, Kind::Video { .. }) {
            self.anchor.x
        } else {
            self.anchor.x + self.anchor.width - node.size().width
        })
        .clamp(8.0, (bounds.width - node.size().width - 8.0).max(8.0));
        let y = if node.size().height <= below {
            self.anchor.y + self.anchor.height + 4.0
        } else {
            (self.anchor.y - node.size().height - 4.0).max(8.0)
        };
        node.move_to_mut(Point::new(x, y));
        // Include the shadow in the overlay's clipping bounds.
        layout::Node::with_children(bounds, vec![node])
    }
    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let content_layout = layout.children().next().unwrap();
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { .. })
                if matches!(self.kind, Kind::Video { .. }) =>
            {
                shell.publish(self.kind.open(false));
                shell.capture_event();
                return;
            }
            Event::Mouse(mouse::Event::ButtonPressed(button))
                if !cursor.is_over(content_layout.bounds()) =>
            {
                shell.publish(self.kind.open(false));
                // A right-click on another visible row replaces the context menu.
                if *button != mouse::Button::Right || matches!(self.kind, Kind::Tags) {
                    shell.capture_event();
                }
                return;
            }
            Event::Window(iced::window::Event::Unfocused) => {
                shell.publish(self.kind.open(false));
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key),
                modifiers,
                ..
            }) if !modifiers.control() && !modifiers.alt() => {
                if *key == Named::Escape {
                    shell.publish(self.kind.open(false));
                } else {
                    shell.publish(self.kind.key(*key, modifiers.shift()));
                }
                shell.capture_event();
                return;
            }
            _ => {}
        }
        self.content.as_widget_mut().update(
            self.tree,
            event,
            content_layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );
        if cursor.is_over(content_layout.bounds()) && matches!(event, Event::Mouse(_)) {
            shell.capture_event();
        }
    }
    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.content.as_widget_mut().operate(
            self.tree,
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.content.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            layout.children().next().unwrap(),
            cursor,
            &layout.bounds(),
        );
    }
    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let content_layout = layout.children().next().unwrap();
        if cursor.is_over(content_layout.bounds()) {
            self.content
                .as_widget()
                .mouse_interaction(
                    self.tree,
                    content_layout,
                    cursor,
                    &layout.bounds(),
                    renderer,
                )
                .max(mouse::Interaction::Idle)
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_click_is_captured_for_only_the_visible_row_and_never_selects() {
        let bounds = Rectangle::new(Point::new(20.0, 100.0), Size::new(500.0, 80.0));
        let viewport = Rectangle::new(Point::ORIGIN, Size::new(960.0, 150.0));
        for (kind, button, position, captured) in [
            (
                Kind::Video {
                    pending: false,
                    video_id: 2,
                },
                mouse::Button::Right,
                Point::new(25.0, 110.0),
                true,
            ),
            (
                Kind::Video {
                    pending: true,
                    video_id: 2,
                },
                mouse::Button::Right,
                Point::new(400.0, 140.0),
                true,
            ),
            (
                Kind::Video {
                    pending: false,
                    video_id: 2,
                },
                mouse::Button::Left,
                Point::new(25.0, 110.0),
                false,
            ),
            (
                Kind::Video {
                    pending: false,
                    video_id: 2,
                },
                mouse::Button::Right,
                Point::new(25.0, 160.0),
                false,
            ),
            (
                Kind::Video {
                    pending: false,
                    video_id: 2,
                },
                mouse::Button::Right,
                Point::new(10.0, 110.0),
                false,
            ),
            (
                Kind::Tags,
                mouse::Button::Right,
                Point::new(25.0, 110.0),
                false,
            ),
        ] {
            let mut state = State::default();
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            assert_eq!(
                kind.capture_context_press(
                    &mut state,
                    &Event::Mouse(mouse::Event::ButtonPressed(button)),
                    bounds,
                    mouse::Cursor::Available(position),
                    viewport,
                    &mut shell,
                ),
                captured
            );
            assert_eq!(shell.is_event_captured(), captured);
            assert_eq!(state.focused, captured);
            if captured {
                assert!(matches!(&messages[..], [Message::ContextMenu(_, Some(2))]));
                assert_eq!(state.context_anchor, Some(position - bounds.position()));
            } else {
                assert!(messages.is_empty());
            }
        }
    }
}
