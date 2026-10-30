//! Anchored tag menu: overlays the list without changing its layout.
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
    })
}

struct Dropdown<'a> {
    trigger: Element<'a, Message>,
    menu: Option<Element<'a, Message>>,
}

#[derive(Default)]
struct State(bool);

impl Focusable for State {
    fn is_focused(&self) -> bool {
        self.0
    }
    fn focus(&mut self) {
        self.0 = true;
    }
    fn unfocus(&mut self) {
        self.0 = false;
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
        operation.focusable(
            Some(&"tag-filter".into()),
            layout.bounds(),
            tree.state.downcast_mut::<State>(),
        );
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
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.0 = cursor.is_over(layout.bounds());
            }
            Event::Window(iced::window::Event::Unfocused) => state.0 = false,
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key),
                modifiers,
                ..
            }) if state.0 && !modifiers.control() && !modifiers.alt() => {
                if matches!(key, Named::Enter | Named::Space | Named::ArrowDown) {
                    shell.publish(Message::TagFilterOpen(true));
                    shell.capture_event();
                    return;
                }
                if *key == Named::Escape {
                    state.0 = false;
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
        if tree.state.downcast_ref::<State>().0 {
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
        Some(overlay::Element::new(Box::new(Menu {
            content,
            tree: &mut tree.children[1],
            anchor: layout.bounds() + translation,
        })))
    }
}

struct Menu<'a, 'b> {
    content: &'a mut Element<'b, Message>,
    tree: &'a mut Tree,
    anchor: Rectangle,
}

impl overlay::Overlay<Message, Theme, Renderer> for Menu<'_, '_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let below = (bounds.height - self.anchor.y - self.anchor.height - 12.0).max(0.0);
        let above = (self.anchor.y - 12.0).max(0.0);
        let max_height = below.max(above).min(360.0);
        let width = 380.0_f32.min((bounds.width - 16.0).max(0.0));
        let mut node = self.content.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width, max_height)),
        );
        let x = (self.anchor.x + self.anchor.width - node.size().width)
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
            Event::Mouse(mouse::Event::ButtonPressed(_))
                if !cursor.is_over(content_layout.bounds()) =>
            {
                shell.publish(Message::TagFilterOpen(false));
                shell.capture_event();
                return;
            }
            Event::Window(iced::window::Event::Unfocused) => {
                shell.publish(Message::TagFilterOpen(false));
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key),
                modifiers,
                ..
            }) if !modifiers.control() && !modifiers.alt() => {
                if *key == Named::Escape {
                    shell.publish(Message::TagFilterOpen(false));
                } else {
                    shell.publish(Message::TagFilterKey(*key, modifiers.shift()));
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
