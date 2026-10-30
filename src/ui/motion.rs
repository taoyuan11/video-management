//! Local, redraw-driven motion. Widget trees retain animation state across views.
//! No animation messages, polling subscription, or changes to video opacity.
use super::{Message, fluent::*};
use iced::advanced::Renderer as _;
use iced::{
    Background, Color, Element, Event, Length, Padding, Point, Rectangle, Renderer, Size, Theme,
    Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    widget::button as iced_button,
};
use std::{
    cell::Cell,
    hash::{DefaultHasher, Hash, Hasher},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn refresh_preferences() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut enabled: i32 = 1;
    // SPI writes a BOOL into the supplied, correctly sized buffer.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        )
    };
    if ok != 0 {
        ENABLED.store(enabled != 0, Ordering::Relaxed);
    }
}

fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}
impl Tween {
    fn settled(value: f32, duration: Duration, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            started: now,
            duration,
        }
    }
    fn value(self, now: Instant) -> f32 {
        let t = (now.saturating_duration_since(self.started).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        self.from + (self.to - self.from) * (1.0 - (1.0 - t).powi(3))
    }
    fn active(self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.started) < self.duration
    }
    fn go(&mut self, target: f32, now: Instant, animate: bool) {
        if !animate {
            *self = Self::settled(target, self.duration, now);
        } else if target != self.to {
            self.from = self.value(now);
            self.to = target;
            self.started = now;
        }
    }
}

enum Kind {
    Enter(u64),
    Details { visible: bool, width: f32 },
    Hover { live: bool, amount: Rc<Cell<f32>> },
}
struct State {
    tween: Tween,
    key: u64,
    now: Instant,
}
struct Motion<'a> {
    content: Element<'a, Message>,
    kind: Kind,
    clip: bool,
}
struct DetailsTag;
struct HoverTag;

pub fn enter<'a>(key: impl Hash, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    let mut hash = DefaultHasher::new();
    key.hash(&mut hash);
    Element::new(Motion {
        content: content.into(),
        kind: Kind::Enter(hash.finish()),
        clip: true,
    })
}

pub fn popup<'a>(key: impl Hash, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    let mut hash = DefaultHasher::new();
    key.hash(&mut hash);
    Element::new(Motion {
        content: content.into(),
        kind: Kind::Enter(hash.finish()),
        clip: false,
    })
}

pub fn details<'a>(
    visible: bool,
    width: f32,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(Motion {
        content: content.into(),
        kind: Kind::Details { visible, width },
        clip: true,
    })
}

impl Motion<'_> {
    fn interactive(&self) -> bool {
        !matches!(self.kind, Kind::Details { visible: false, .. })
    }
    fn key(&self) -> u64 {
        if let Kind::Enter(key) = self.kind {
            key
        } else {
            0
        }
    }
}

impl Widget<Message, Theme, Renderer> for Motion<'_> {
    fn tag(&self) -> tree::Tag {
        match self.kind {
            Kind::Enter(_) => tree::Tag::of::<State>(),
            Kind::Details { .. } => tree::Tag::of::<DetailsTag>(),
            Kind::Hover { .. } => tree::Tag::of::<HoverTag>(),
        }
    }
    fn state(&self) -> tree::State {
        let now = Instant::now();
        let (initial, duration) = match self.kind {
            Kind::Enter(_) => (if enabled() { 0.0 } else { 1.0 }, MOTION_ENTER),
            Kind::Details { visible, .. } => (f32::from(visible), MOTION_PANEL),
            Kind::Hover { .. } => (0.0, MOTION_HOVER),
        };
        let mut tween = Tween::settled(initial, duration, now);
        if matches!(self.kind, Kind::Enter(_)) {
            tween.go(1.0, now, enabled());
        }
        tree::State::new(State {
            tween,
            key: self.key(),
            now,
        })
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        state.now = Instant::now();
        match self.kind {
            Kind::Enter(key) if key != state.key => {
                state.key = key;
                state.tween = Tween::settled(0.0, MOTION_ENTER, state.now);
                state.tween.go(1.0, state.now, enabled());
            }
            Kind::Details { visible, .. } => {
                state.tween.go(f32::from(visible), state.now, enabled())
            }
            _ => {}
        }
        if !enabled() {
            state.tween.go(state.tween.to, state.now, false);
        }
        tree.diff_children(std::slice::from_ref(&self.content));
    }
    fn size(&self) -> Size<Length> {
        if matches!(self.kind, Kind::Details { .. }) {
            Size::new(Length::Shrink, Length::Fill)
        } else {
            self.content.as_widget().size()
        }
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_ref::<State>();
        let value = state.tween.value(state.now);
        let child_limits = if let Kind::Details { width, .. } = self.kind {
            // Keep the inspector at its final width while revealing it; long paths
            // must not rewrap through dozens of narrow layouts on every frame.
            layout::Limits::new(
                Size::ZERO,
                Size::new(width + DETAILS_RESIZE_WIDTH, limits.max().height),
            )
        } else {
            *limits
        };
        let mut child =
            self.content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, &child_limits);
        let mut size = child.size();
        match self.kind {
            Kind::Details { width, .. } => {
                size.width = (width + DETAILS_RESIZE_WIDTH) * value;
                child.move_to_mut(Point::new(MOTION_DISTANCE * (1.0 - value), 0.0));
            }
            Kind::Enter(_) => child.move_to_mut(Point::new(0.0, MOTION_DISTANCE * (1.0 - value))),
            Kind::Hover { .. } => {}
        }
        layout::Node::with_children(size, vec![child])
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
        let now = match event {
            Event::Window(iced::window::Event::RedrawRequested(now)) => *now,
            _ => Instant::now(),
        };
        let was_active = state.tween.active(state.now);
        if let Kind::Hover { live, .. } = self.kind {
            state.tween.go(
                f32::from(live && cursor.is_over(layout.bounds()) && cursor.is_over(*viewport)),
                now,
                enabled(),
            );
        }
        state.now = now;
        if was_active || state.tween.active(now) {
            if !matches!(self.kind, Kind::Hover { .. }) {
                shell.invalidate_layout();
            }
            shell.request_redraw();
        }
        if self.interactive() {
            let cursor = if self.clip && !cursor.is_over(layout.bounds()) {
                mouse::Cursor::Unavailable
            } else {
                cursor
            };
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout.children().next().unwrap(),
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
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
        let state = tree.state.downcast_ref::<State>();
        if let Kind::Hover { amount, .. } = &self.kind {
            amount.set(state.tween.value(state.now));
        }
        let draw = |renderer: &mut Renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout.children().next().unwrap(),
                cursor,
                viewport,
            )
        };
        // Restore the full Fluent panel shadow when the reveal has finished.
        let clip = self.clip
            && !matches!(self.kind, Kind::Details { visible: true, .. } if state.tween.value(state.now) == 1.0);
        if clip {
            if let Some(bounds) = layout.bounds().intersection(viewport) {
                renderer.with_layer(bounds, draw);
            }
        } else {
            draw(renderer);
        }
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.interactive() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout.children().next().unwrap(),
                renderer,
                operation,
            );
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.interactive()
            && (cursor.is_over(layout.bounds()) || matches!(self.kind, Kind::Details { .. }))
        {
            let cursor = if self.clip && !cursor.is_over(layout.bounds()) {
                mouse::Cursor::Unavailable
            } else {
                cursor
            };
            self.content.as_widget().mouse_interaction(
                &tree.children[0],
                layout.children().next().unwrap(),
                cursor,
                viewport,
                renderer,
            )
        } else {
            mouse::Interaction::None
        }
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        if !self.interactive() {
            return None;
        }
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        )
    }
}

/// Keep the native iced button's input, disabled behavior and content; animate
/// only its Fluent surface. The shared cell is filled from retained tree state.
pub struct Button<'a> {
    inner: iced_button::Button<'a, Message>,
    style: ButtonStyle<'a>,
    live: bool,
}
type ButtonStyle<'a> = Box<dyn Fn(&Theme, iced_button::Status) -> iced_button::Style + 'a>;
pub fn button<'a>(content: impl Into<Element<'a, Message>>) -> Button<'a> {
    Button {
        inner: iced_button(content),
        style: Box::new(secondary),
        live: false,
    }
}
impl<'a> Button<'a> {
    pub fn style(
        mut self,
        style: impl Fn(&Theme, iced_button::Status) -> iced_button::Style + 'a,
    ) -> Self {
        self.style = Box::new(style);
        self
    }
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.inner = self.inner.padding(padding);
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.inner = self.inner.width(width);
        self
    }
    pub fn on_press(self, message: Message) -> Self {
        self.on_press_maybe(Some(message))
    }
    pub fn on_press_maybe(mut self, message: Option<Message>) -> Self {
        self.live = message.is_some();
        self.inner = self.inner.on_press_maybe(message);
        self
    }
}
impl<'a> From<Button<'a>> for Element<'a, Message> {
    fn from(button: Button<'a>) -> Self {
        let amount = Rc::new(Cell::new(0.0));
        let value = amount.clone();
        let style = button.style;
        let content = button.inner.style(move |theme, status| {
            let mut current = style(theme, status);
            if matches!(
                status,
                iced_button::Status::Active | iced_button::Status::Hovered
            ) {
                let active = style(theme, iced_button::Status::Active);
                let hovered = style(theme, iced_button::Status::Hovered);
                let color = |background| match background {
                    Some(Background::Color(color)) => color,
                    _ => Color::TRANSPARENT,
                };
                current.background = Some(
                    mix(
                        color(active.background),
                        color(hovered.background),
                        value.get(),
                    )
                    .into(),
                );
            }
            current
        });
        Element::new(Motion {
            content: content.into(),
            kind: Kind::Hover {
                live: button.live,
                amount,
            },
            clip: false,
        })
    }
}
fn mix(a: Color, b: Color, t: f32) -> Color {
    // Interpolate premultiplied RGB so a fully transparent endpoint contributes
    // no black to the visible surface. Return straight alpha for iced to render.
    let alpha = a.a * (1.0 - t) + b.a * t;
    if alpha == 0.0 {
        return Color::TRANSPARENT;
    }
    let channel =
        |from: f32, to: f32| ((from * a.a * (1.0 - t) + to * b.a * t) / alpha).clamp(0.0, 1.0);
    Color::from_rgba(
        channel(a.r, b.r),
        channel(a.g, b.g),
        channel(a.b, b.b),
        alpha,
    )
}

pub fn spinner<'a>(size: f32) -> Element<'a, Message> {
    Element::new(Spinner(size))
}
struct Spinner(f32);
struct SpinnerState {
    start: Instant,
    now: Instant,
}
impl Widget<Message, Theme, Renderer> for Spinner {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SpinnerState>()
    }
    fn state(&self) -> tree::State {
        let now = Instant::now();
        tree::State::new(SpinnerState { start: now, now })
    }
    fn size(&self) -> Size<Length> {
        Size::new(self.0.into(), self.0.into())
    }
    fn layout(&mut self, _: &mut Tree, _: &Renderer, _: &layout::Limits) -> layout::Node {
        layout::Node::new(Size::new(self.0, self.0))
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        _: mouse::Cursor,
        _: &Renderer,
        _: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            tree.state.downcast_mut::<SpinnerState>().now = *now;
            if enabled() && layout.bounds().intersects(viewport) {
                shell.request_redraw();
            }
        }
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
        let state = tree.state.downcast_ref::<SpinnerState>();
        let phase = if enabled() {
            state
                .now
                .saturating_duration_since(state.start)
                .as_secs_f32()
                / 1.2
        } else {
            0.0
        };
        let dot = (self.0 / 8.0).max(2.0);
        let radius = (self.0 - dot) / 2.0;
        for i in 0..8 {
            let angle = std::f32::consts::TAU * (phase + i as f32 / 8.0);
            let center = bounds.center();
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(
                        Point::new(
                            center.x + radius * angle.cos() - dot / 2.0,
                            center.y + radius * angle.sin() - dot / 2.0,
                        ),
                        Size::new(dot, dot),
                    ),
                    border: iced::border::rounded(dot / 2.0),
                    ..Default::default()
                },
                mix(
                    palette(theme).backdrop,
                    palette(theme).accent,
                    0.22 + 0.78 * i as f32 / 7.0,
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hover_never_flashes_darker_than_its_endpoints() {
        let Some(Background::Color(hovered)) =
            quiet(&Theme::Light, iced_button::Status::Hovered).background
        else {
            panic!("quiet hover must have a color background");
        };
        // Check the visible surface, including both the opaque fallback and
        // representative dark/light Acrylic backdrops, in both directions.
        for backdrop in [BACKDROP, Color::BLACK, Color::WHITE] {
            for (from, to) in [(Color::TRANSPARENT, hovered), (hovered, Color::TRANSPARENT)] {
                for step in 0..=24 {
                    let t = step as f32 / 24.0;
                    let actual = mix(from, to, t);
                    for (channel, start, end, base) in [
                        (actual.r, from.r, to.r, backdrop.r),
                        (actual.g, from.g, to.g, backdrop.g),
                        (actual.b, from.b, to.b, backdrop.b),
                    ] {
                        let visible = channel * actual.a + base * (1.0 - actual.a);
                        let start = start * from.a + base * (1.0 - from.a);
                        let end = end * to.a + base * (1.0 - to.a);
                        let expected = start + (end - start) * t;
                        assert!(
                            (visible - expected).abs() < 0.00001,
                            "hover surface flashes at t={t}: {visible} != {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn color_mix_preserves_opaque_surfaces_and_handles_invisible_colors() {
        for step in 0..=24 {
            let t = step as f32 / 24.0;
            // The spinner and opaque buttons retain their original transition.
            let color = mix(BACKDROP, ACCENT, t);
            assert!((color.r - (BACKDROP.r + (ACCENT.r - BACKDROP.r) * t)).abs() < 0.00001);
            assert!((color.g - (BACKDROP.g + (ACCENT.g - BACKDROP.g) * t)).abs() < 0.00001);
            assert!((color.b - (BACKDROP.b + (ACCENT.b - BACKDROP.b) * t)).abs() < 0.00001);
            assert_eq!(color.a, 1.0);
            assert_eq!(
                mix(Color::TRANSPARENT, Color::from_rgba(1.0, 0.0, 0.0, 0.0), t),
                Color::TRANSPARENT
            );
        }
    }

    #[test]
    fn interrupted_transition_continues_from_current_position() {
        let now = Instant::now();
        let mut tween = Tween::settled(0.0, MOTION_PANEL, now);
        tween.go(1.0, now, true);
        let halfway = now + MOTION_PANEL / 2;
        let value = tween.value(halfway);
        assert!(value > 0.0 && value < 1.0);
        tween.go(0.0, halfway, true);
        assert_eq!(tween.value(halfway), value);
        assert_eq!(tween.value(halfway + MOTION_PANEL), 0.0);
        assert!(!tween.active(halfway + MOTION_PANEL));
    }
    #[test]
    fn repeated_target_does_not_restart_and_reduced_motion_settles() {
        let now = Instant::now();
        let mut tween = Tween::settled(0.0, MOTION_ENTER, now);
        tween.go(1.0, now, true);
        tween.go(1.0, now + MOTION_ENTER / 2, true);
        assert_eq!(tween.value(now + MOTION_ENTER), 1.0);
        tween.go(0.0, now, false);
        assert_eq!(tween.value(now), 0.0);
        assert!(!tween.active(now));
    }
}
