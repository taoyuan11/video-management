//! Video and its controls share one viewport and retained interaction state.
//! Drawing the chrome locally keeps hidden controls out of mouse hit testing,
//! while explicit focus targets remain reachable by keyboard.
use super::{App, Message, fluent::*, time_label};
use iced::advanced::Renderer as _;
use iced::{
    Element, Event, Fill, Font, Length, Point, Rectangle, Renderer, Size, Theme,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, renderer,
        text::{self, Paragraph as _, Renderer as _},
        widget::{Operation, Tree, operation::Focusable, tree},
    },
    keyboard::{self, key::Named},
    widget::container,
    window,
};
use iced_video_player::VideoPlayer;
use std::time::{Duration, Instant};

const HIDE_AFTER: Duration = Duration::from_secs(3);
const PAD: f32 = 8.0;
const MINI_WIDTH: f32 = 400.0;
const MINI_HEIGHT: f32 = 240.0;

fn metrics(size: Size) -> MediaMetrics {
    if size.width < MINI_WIDTH || size.height < MINI_HEIGHT {
        MediaMetrics::MINI
    } else {
        MediaMetrics::REGULAR
    }
}

pub(super) fn view(app: &App, height: Length) -> Element<'_, Message> {
    let video = app.player.as_ref().expect("loaded player");
    Element::new(Player {
        content: container(
            VideoPlayer::new(video)
                .width(Fill)
                .height(height)
                .content_fit(iced::ContentFit::Contain)
                .on_new_frame(Message::Frame),
        )
        .width(Fill)
        .height(height)
        .into(),
        title: &app.player_title,
        pending: Some(app.player_window) == app.pending,
        key: Key {
            owner: app.player_window,
            file: app.playing_file,
            full: app.fullscreen,
        },
        playback: Playback {
            position: app.position,
            duration: video.duration().as_secs_f64(),
            volume: app.settings.volume,
            paused: video.paused(),
            muted: app.settings.muted,
        },
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    owner: window::Id,
    file: Option<i64>,
    full: bool,
}

#[derive(Clone, Copy)]
struct Playback {
    position: f64,
    duration: f64,
    volume: f64,
    paused: bool,
    muted: bool,
}

impl Playback {
    fn seekable(self) -> bool {
        self.duration.is_finite() && self.duration > 0.0
    }

    fn time(self) -> String {
        format!(
            "{} / {}",
            time_label(self.position),
            time_label(self.duration)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Seek,
    Play,
    Mute,
    Volume,
    Fullscreen,
}

const CONTROLS: [Control; 5] = [
    Control::Seek,
    Control::Play,
    Control::Mute,
    Control::Volume,
    Control::Fullscreen,
];

#[derive(Default)]
struct Focus(bool);

impl Focusable for Focus {
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

struct Interaction {
    now: Instant,
    activity: Option<Instant>,
    inside: bool,
    window_active: bool,
    hovered: Option<Control>,
    pressed: Option<Control>,
    drag: Option<Control>,
    volume_open: bool,
    volume_hover: bool,
    was_pinned: bool,
    focus: [Focus; 5],
}

impl Interaction {
    fn new(now: Instant) -> Self {
        Self {
            now,
            activity: None,
            inside: false,
            window_active: true,
            hovered: None,
            pressed: None,
            drag: None,
            volume_open: false,
            volume_hover: false,
            was_pinned: false,
            focus: Default::default(),
        }
    }

    fn focused(&self) -> Option<Control> {
        CONTROLS
            .into_iter()
            .zip(&self.focus)
            .find_map(|(c, f)| f.0.then_some(c))
    }

    fn pinned(&self) -> bool {
        self.focused().is_some()
            || self.pressed.is_some()
            || self.drag.is_some()
            || self.volume_hover
    }

    fn visible(&self) -> bool {
        self.window_active
            && (self.pinned()
                || (self.inside
                    && self
                        .activity
                        .is_some_and(|t| self.now.saturating_duration_since(t) < HIDE_AFTER)))
    }

    fn popup(&self) -> bool {
        self.volume_open || matches!(self.focused(), Some(Control::Mute | Control::Volume))
    }

    fn finish_drag(&mut self, shell: &mut Shell<'_, Message>) {
        if self.drag.take() == Some(Control::Seek) {
            shell.publish(Message::SeekRelease);
        }
        self.pressed = None;
    }

    fn move_slider(
        &self,
        control: Control,
        point: Point,
        geometry: &Geometry,
        playback: Playback,
        shell: &mut Shell<'_, Message>,
    ) {
        let rail = geometry.rail(control);
        let fraction = if control == Control::Volume {
            (rail.y + rail.height - point.y) / rail.height.max(1.0)
        } else {
            (point.x - rail.x) / rail.width.max(1.0)
        }
        .clamp(0.0, 1.0) as f64;
        match control {
            Control::Seek if playback.seekable() => {
                shell.publish(Message::Seek(fraction * playback.duration))
            }
            Control::Volume => shell.publish(Message::Volume((fraction * 100.0).round() / 100.0)),
            _ => {}
        }
    }

    fn handle(
        &mut self,
        event: &Event,
        pointer: Option<Point>,
        geometry: &Geometry,
        playback: Playback,
        key: Key,
        shell: &mut Shell<'_, Message>,
    ) {
        let was_visible = self.visible();
        let was_popup = self.popup();
        let was_hovered = self.hovered;
        self.now = match event {
            Event::Window(window::Event::RedrawRequested(now)) => *now,
            _ => self.now,
        };
        match event {
            Event::Window(window::Event::Unfocused) => {
                self.finish_drag(shell);
                *self = Self::new(self.now);
                self.window_active = false;
                shell.request_redraw();
                return;
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                self.finish_drag(shell);
                self.inside = false;
                self.hovered = None;
                self.activity = None;
                self.volume_hover = false;
                self.volume_open = false;
                // Leaving the window with the mouse does not transfer keyboard
                // focus. A focused control must stay visible and usable.
                self.was_pinned = self.pinned();
                shell.request_redraw();
                return;
            }
            Event::Window(window::Event::Focused) => self.window_active = true,
            _ => {}
        }
        let inside = self.window_active && pointer.is_some_and(|p| geometry.bounds.contains(p));
        if inside
            && (!self.inside || matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. })))
        {
            self.activity = Some(self.now);
        }
        self.inside = inside;
        let point = pointer.filter(|_| inside);
        let over_mute = point.is_some_and(|p| geometry.rect(Control::Mute).contains(p));
        let over_popup = point.is_some_and(|p| geometry.popup.contains(p));
        self.volume_open = matches!(self.focused(), Some(Control::Mute | Control::Volume))
            || self.drag == Some(Control::Volume)
            || (self.visible() && over_mute)
            || (self.volume_open && over_popup);
        self.volume_hover = self.volume_open && (over_mute || over_popup);
        self.hovered = if self.visible() {
            point.and_then(|p| geometry.hit(p, self.popup(), playback.seekable()))
        } else {
            None
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Mouse input must not leave a keyboard focus pin behind.
                for focus in &mut self.focus {
                    focus.unfocus();
                }
                self.pressed = self.hovered;
                if let Some(control) = self.pressed {
                    self.activity = Some(self.now);
                    if matches!(control, Control::Seek | Control::Volume) {
                        self.drag = Some(control);
                        self.move_slider(control, point.unwrap(), geometry, playback, shell);
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(control) = self.drag {
                    // Continue dragging outside the video/scroll viewport until release.
                    self.move_slider(control, *position, geometry, playback, shell);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let pressed = self.pressed;
                if self.drag.is_some() {
                    self.finish_drag(shell);
                    shell.capture_event();
                } else if let Some(control) = pressed {
                    if Some(control) == self.hovered {
                        activate(control, key, shell);
                    }
                    self.pressed = None;
                    shell.capture_event();
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(pressed),
                modifiers,
                repeat,
                ..
            }) if !modifiers.control() && !modifiers.alt() => {
                if let Some(control) = self.focused() {
                    let direction = match pressed {
                        Named::ArrowLeft | Named::ArrowDown => -1.0,
                        Named::ArrowRight | Named::ArrowUp => 1.0,
                        _ => 0.0,
                    };
                    if direction != 0.0 && matches!(control, Control::Seek | Control::Volume) {
                        if control == Control::Seek && playback.seekable() {
                            shell.publish(Message::Seek(
                                (playback.position + direction * 5.0).clamp(0.0, playback.duration),
                            ));
                            shell.publish(Message::SeekRelease);
                        } else if control == Control::Volume {
                            shell.publish(Message::Volume(
                                (playback.volume + direction * 0.05).clamp(0.0, 1.0),
                            ));
                        }
                        shell.capture_event();
                    } else if matches!(pressed, Named::Enter | Named::Space) && !repeat {
                        activate(control, key, shell);
                        shell.capture_event();
                    }
                }
                // Leave Tab and Escape to the application's existing navigation.
                if *pressed == Named::Escape {
                    self.finish_drag(shell);
                }
            }
            _ => {}
        }

        if self.was_pinned && !self.pinned() {
            self.activity = Some(self.now);
        }
        self.was_pinned = self.pinned();
        if !self.visible() {
            self.volume_open = false;
            self.hovered = None;
        }
        if was_visible != self.visible()
            || was_popup != self.popup()
            || was_hovered != self.hovered
            || !matches!(event, Event::Window(window::Event::RedrawRequested(_)))
        {
            shell.request_redraw();
        }
        if self.visible()
            && !self.pinned()
            && let Some(activity) = self.activity
        {
            shell.request_redraw_at(activity + HIDE_AFTER);
        }
    }
}

fn activate(control: Control, key: Key, shell: &mut Shell<'_, Message>) {
    let message = match control {
        Control::Play => Message::Pause,
        Control::Mute => Message::Mute,
        Control::Fullscreen => Message::Fullscreen(key.owner),
        _ => return,
    };
    shell.publish(message);
}

struct Geometry {
    metrics: MediaMetrics,
    bounds: Rectangle,
    header: Rectangle,
    title: Rectangle,
    footer: Rectangle,
    time: Rectangle,
    popup: Rectangle,
    percent: Rectangle,
    controls: [Rectangle; 5],
}

impl Geometry {
    fn new(bounds: Rectangle, time_width: f32) -> Self {
        let metrics = metrics(bounds.size());
        let pad = metrics.padding;
        let x = bounds.x + pad;
        let width = (bounds.width - 2.0 * pad).max(0.0);
        let y = bounds.y + bounds.height - pad - metrics.button;
        let button =
            |x| Rectangle::new(Point::new(x, y), Size::new(metrics.button, metrics.button));
        let play = button(x);
        let full = button(x + width - metrics.button);
        let mute = button(full.x - metrics.button - metrics.gap);
        let time_x = play.x + play.width + pad;
        let compact = time_width > mute.x - pad - time_x;
        let time = if compact {
            Rectangle::new(
                Point::new(x, y - metrics.line_height),
                Size::new(width, metrics.line_height),
            )
        } else {
            Rectangle::new(
                Point::new(time_x, y + (metrics.button - metrics.line_height) / 2.0),
                Size::new(mute.x - pad - time_x, metrics.line_height),
            )
        };
        let seek = Rectangle::new(
            Point::new(
                x,
                y - metrics.seek_height - if compact { metrics.line_height } else { 0.0 },
            ),
            Size::new(width, metrics.seek_height),
        );
        let popup_width = metrics.button + metrics.popup_padding * 2.0;
        let popup_height = (y - bounds.y - pad).min(metrics.popup_height);
        let popup = Rectangle::new(
            Point::new(mute.center_x() - popup_width / 2.0, y - popup_height),
            Size::new(popup_width, popup_height),
        );
        let volume = Rectangle::new(
            Point::new(
                popup.x + metrics.popup_padding,
                popup.y + metrics.popup_padding + metrics.line_height + metrics.gap,
            ),
            Size::new(
                metrics.button,
                popup.height - metrics.popup_padding * 2.0 - metrics.line_height - metrics.gap,
            ),
        );
        Self {
            metrics,
            bounds,
            header: Rectangle::new(
                bounds.position(),
                Size::new(bounds.width, metrics.line_height + pad * 2.0),
            ),
            title: Rectangle::new(
                Point::new(x, bounds.y + pad),
                Size::new(width, metrics.line_height),
            ),
            footer: Rectangle::new(
                Point::new(bounds.x, seek.y - pad),
                Size::new(bounds.width, bounds.y + bounds.height - seek.y + pad),
            ),
            time,
            popup,
            percent: Rectangle::new(
                Point::new(popup.x + pad, popup.y + metrics.popup_padding),
                Size::new(popup.width - pad * 2.0, metrics.line_height),
            ),
            controls: [seek, play, mute, volume, full],
        }
    }

    fn rect(&self, control: Control) -> Rectangle {
        self.controls[control as usize]
    }

    fn rail(&self, control: Control) -> Rectangle {
        let bounds = self.rect(control);
        let inset = self.metrics.rail_inset;
        let thickness = self.metrics.rail_thickness;
        if control == Control::Volume {
            Rectangle::new(
                Point::new(bounds.center_x() - thickness / 2.0, bounds.y + inset),
                Size::new(thickness, (bounds.height - inset * 2.0).max(1.0)),
            )
        } else {
            Rectangle::new(
                Point::new(bounds.x + inset, bounds.center_y() - thickness / 2.0),
                Size::new((bounds.width - inset * 2.0).max(1.0), thickness),
            )
        }
    }

    fn hit(&self, point: Point, popup: bool, seekable: bool) -> Option<Control> {
        if popup && self.popup.contains(point) {
            return self
                .rect(Control::Volume)
                .contains(point)
                .then_some(Control::Volume);
        }
        CONTROLS.into_iter().find(|c| {
            *c != Control::Volume
                && (*c != Control::Seek || seekable)
                && self.rect(*c).contains(point)
        })
    }
}

struct Player<'a> {
    content: Element<'a, Message>,
    title: &'a str,
    pending: bool,
    key: Key,
    playback: Playback,
}

struct State {
    key: Key,
    input: Interaction,
    title_source: String,
    title_width: f32,
    title_size: f32,
    title: String,
    time_width: f32,
}

impl State {
    fn new(key: Key) -> Self {
        Self {
            key,
            input: Interaction::new(Instant::now()),
            title_source: String::new(),
            title_width: -1.0,
            title_size: 0.0,
            title: String::new(),
            time_width: 0.0,
        }
    }
}

impl Widget<Message, Theme, Renderer> for Player<'_> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let content = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        let state = tree.state.downcast_mut::<State>();
        let metrics = metrics(content.size());
        let width = (content.size().width - metrics.padding * 2.0).max(0.0);
        if state.title_source != self.title
            || state.title_width != width
            || state.title_size != metrics.title_size
        {
            self.title.clone_into(&mut state.title_source);
            state.title_width = width;
            state.title_size = metrics.title_size;
            state.title = ellipsize(self.title, width, |s| measure(s, metrics.title_size));
        }
        state.time_width = measure(&self.playback.time(), metrics.text_size);
        layout::Node::with_children(content.size(), vec![content])
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
        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let state = tree.state.downcast_ref::<State>();
        renderer.with_layer(clip, |renderer| {
            quad(renderer, bounds, MEDIA_BACKGROUND, 8.0);
            // Custom video primitives and the chrome need separate renderer
            // layers; otherwise the shader can paint over the text and quads.
            renderer.with_layer(clip, |renderer| {
                self.content.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    layout.children().next().unwrap(),
                    cursor,
                    &clip,
                );
            });
            if state.input.visible() {
                let geometry = Geometry::new(bounds, state.time_width);
                renderer.with_layer(clip, |renderer| {
                    self.draw_controls(state, &geometry, renderer, clip)
                });
            }
        });
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::new(self.key))
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.key != self.key {
            *state = State::new(self.key);
        }
        tree.diff_children(std::slice::from_ref(&self.content));
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let geometry = Geometry::new(layout.bounds(), state.time_width);
        for (control, focus) in CONTROLS.into_iter().zip(&mut state.input.focus) {
            if control != Control::Seek || self.playback.seekable() {
                operation.focusable(None, geometry.rect(control), focus);
                if focus.0 {
                    let bounds = if matches!(control, Control::Mute | Control::Volume) {
                        geometry.popup.union(&geometry.rect(Control::Mute))
                    } else {
                        geometry.rect(control)
                    };
                    operation.container(Some(&focus_id(self.pending).into()), bounds);
                }
            }
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
        state.input.now = match event {
            Event::Window(window::Event::RedrawRequested(now)) => *now,
            _ => Instant::now(),
        };
        let pointer = cursor.position().filter(|p| viewport.contains(*p));
        let geometry = Geometry::new(layout.bounds(), state.time_width);
        // Scrollable translates the cursor into content coordinates, but leaves
        // the raw event in window coordinates. Vertical drags must use the
        // translated position, including when dragging outside the viewport.
        let motion;
        let interaction_event = if let Event::Mouse(mouse::Event::CursorMoved { position }) = event
        {
            motion = Event::Mouse(mouse::Event::CursorMoved {
                position: cursor.land().position().unwrap_or(*position),
            });
            &motion
        } else {
            event
        };
        state.input.handle(
            interaction_event,
            pointer,
            &geometry,
            self.playback,
            self.key,
            shell,
        );
        // Keep forwarding redraw events to the decoder regardless of chrome state.
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
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        _: &Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        if state.input.drag.is_some() {
            return mouse::Interaction::Pointer;
        }
        if state.input.visible()
            && cursor.position().is_some_and(|p| {
                viewport.contains(p)
                    && Geometry::new(layout.bounds(), state.time_width)
                        .hit(p, state.input.popup(), self.playback.seekable())
                        .is_some()
            })
        {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }
}

impl Player<'_> {
    fn draw_controls(&self, state: &State, g: &Geometry, renderer: &mut Renderer, clip: Rectangle) {
        let metrics = g.metrics;
        quad(renderer, g.header, MEDIA_SCRIM, 0.0);
        quad(renderer, g.footer, MEDIA_SCRIM, 0.0);
        draw_text(
            renderer,
            &state.title,
            g.title,
            metrics.title_size,
            TEXT_FONT,
            MEDIA_TEXT,
            clip,
        );
        draw_text(
            renderer,
            &self.playback.time(),
            g.time,
            metrics.text_size,
            TEXT_FONT,
            MEDIA_TEXT,
            clip,
        );
        self.draw_slider(
            state,
            g,
            Control::Seek,
            if self.playback.seekable() {
                self.playback.position / self.playback.duration
            } else {
                0.0
            },
            renderer,
        );
        for (control, glyph) in [
            (
                Control::Play,
                if self.playback.paused {
                    PLAY
                } else {
                    '\u{E769}'
                },
            ),
            (
                Control::Mute,
                if self.playback.muted {
                    '\u{E74F}'
                } else {
                    '\u{E767}'
                },
            ),
            (
                Control::Fullscreen,
                if self.key.full {
                    '\u{E73F}'
                } else {
                    '\u{E740}'
                },
            ),
        ] {
            let rect = g.rect(control);
            control_focus(state, control, g, renderer);
            let icon = Rectangle::new(
                Point::new(
                    rect.center_x() - metrics.icon / 2.0,
                    rect.center_y() - metrics.line_height / 2.0,
                ),
                Size::new(metrics.icon, metrics.line_height),
            );
            draw_text(
                renderer,
                &glyph.to_string(),
                icon,
                metrics.icon,
                icon_font(),
                MEDIA_TEXT,
                clip,
            );
        }
        if state.input.popup() {
            renderer.with_layer(clip, |renderer| {
                quad(renderer, g.popup, MEDIA_POPUP, metrics.radius);
                self.draw_slider(state, g, Control::Volume, self.playback.volume, renderer);
                draw_text(
                    renderer,
                    &format!("{}%", (self.playback.volume * 100.0).round()),
                    g.percent,
                    metrics.text_size,
                    TEXT_FONT,
                    MEDIA_TEXT,
                    clip,
                );
            });
        }
    }

    fn draw_slider(
        &self,
        state: &State,
        g: &Geometry,
        control: Control,
        fraction: f64,
        renderer: &mut Renderer,
    ) {
        control_focus(state, control, g, renderer);
        let rail = g.rail(control);
        let fraction = fraction.clamp(0.0, 1.0) as f32;
        let (filled, thumb) = if control == Control::Volume {
            let height = fraction * rail.height;
            let y = rail.y + rail.height - height;
            (
                Rectangle { y, height, ..rail },
                Point::new(rail.center_x(), y),
            )
        } else {
            let width = fraction * rail.width;
            (
                Rectangle { width, ..rail },
                Point::new(rail.x + width, rail.center_y()),
            )
        };
        let enabled = control != Control::Seek || self.playback.seekable();
        quad(renderer, rail, MEDIA_TRACK, g.metrics.rail_thickness / 2.0);
        quad(
            renderer,
            filled,
            if enabled { MEDIA_ACCENT } else { MEDIA_MUTED },
            g.metrics.rail_thickness / 2.0,
        );
        let radius = g.metrics.thumb_radius;
        quad(
            renderer,
            Rectangle::new(
                Point::new(thumb.x - radius, thumb.y - radius),
                Size::new(radius * 2.0, radius * 2.0),
            ),
            if enabled { MEDIA_TEXT } else { MEDIA_MUTED },
            radius,
        );
    }
}

fn control_focus(state: &State, control: Control, geometry: &Geometry, renderer: &mut Renderer) {
    if state.input.focused() == Some(control) {
        quad(
            renderer,
            geometry.rect(control),
            MEDIA_FOCUS,
            geometry.metrics.radius,
        );
    }
}

fn quad(renderer: &mut Renderer, bounds: Rectangle, color: iced::Color, radius: f32) {
    renderer.fill_quad(
        renderer::Quad {
            bounds,
            border: iced::border::rounded(radius),
            ..Default::default()
        },
        color,
    );
}

fn text_spec(content: &str, size: f32, font: Font, bounds: Size) -> text::Text<&str> {
    text::Text {
        content,
        bounds,
        size: size.into(),
        line_height: iced::Pixels(bounds.height).into(),
        font,
        align_x: text::Alignment::Left,
        align_y: iced::alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::None,
    }
}

fn draw_text(
    renderer: &mut Renderer,
    value: &str,
    bounds: Rectangle,
    size: f32,
    font: Font,
    color: iced::Color,
    clip: Rectangle,
) {
    renderer.fill_text(
        text_spec(value, size, font, bounds.size()).with_content(value.to_owned()),
        bounds.position(),
        color,
        clip,
    );
}

fn measure(value: &str, size: f32) -> f32 {
    <Renderer as text::Renderer>::Paragraph::with_text(text_spec(
        value,
        size,
        TEXT_FONT,
        Size::new(f32::INFINITY, 20.0),
    ))
    .min_width()
}

fn ellipsize(value: &str, width: f32, measure: impl Fn(&str) -> f32) -> String {
    let value = value.replace(['\r', '\n'], " ");
    if measure(&value) <= width {
        return value;
    }
    if measure("…") > width {
        return String::new();
    }
    let offsets: Vec<_> = value
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(value.len()))
        .collect();
    let mut low = 0;
    let mut high = offsets.len() - 1;
    while low < high {
        let mid = (low + high).div_ceil(2);
        if measure(&format!("{}…", &value[..offsets[mid]])) <= width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    format!("{}…", &value[..offsets[low]])
}

pub(super) fn scroll_id(pending: bool) -> &'static str {
    if pending {
        "pending-details"
    } else {
        "library-details"
    }
}

fn focus_id(pending: bool) -> &'static str {
    if pending {
        "pending-player-focus"
    } else {
        "library-player-focus"
    }
}

// Run after Tab navigation so a portrait video's focused controls are scrolled
// into view. Only move the relevant inspector, and only as far as necessary.
pub(super) fn reveal_focus(pending: bool) -> iced::Task<Message> {
    iced::advanced::widget::operate(RevealFocus {
        pending,
        viewport: None,
        target: None,
    })
}

struct RevealFocus {
    pending: bool,
    viewport: Option<(Rectangle, Rectangle, iced::Vector)>,
    target: Option<Rectangle>,
}

impl Operation<Message> for RevealFocus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Message>)) {
        operate(self);
    }
    fn container(&mut self, id: Option<&iced::advanced::widget::Id>, bounds: Rectangle) {
        if id == Some(&focus_id(self.pending).into()) {
            self.target = Some(bounds);
        }
    }
    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: iced::Vector,
        _: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&scroll_id(self.pending).into()) {
            self.viewport = Some((bounds, content, translation));
        }
    }
    fn finish(&self) -> iced::advanced::widget::operation::Outcome<Message> {
        use iced::advanced::widget::operation::{Outcome, scrollable};
        if let (Some((viewport, content, translation)), Some(target)) = (self.viewport, self.target)
            && let Some(y) = reveal_offset(viewport, content, translation.y, target)
        {
            return Outcome::Chain(Box::new(scrollable::scroll_to(
                scroll_id(self.pending).into(),
                scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(y),
                },
            )));
        }
        Outcome::None
    }
}

fn reveal_offset(
    viewport: Rectangle,
    content: Rectangle,
    offset: f32,
    target: Rectangle,
) -> Option<f32> {
    let top = target.y - offset;
    let bottom = top + target.height;
    if top < viewport.y {
        Some((target.y - content.y - PAD).max(0.0))
    } else if bottom > viewport.y + viewport.height {
        Some((target.y + target.height - content.y - viewport.height + PAD).max(0.0))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback() -> Playback {
        Playback {
            position: 20.0,
            duration: 120.0,
            volume: 0.7,
            paused: true,
            muted: false,
        }
    }

    fn key() -> Key {
        Key {
            owner: window::Id::unique(),
            file: Some(1),
            full: false,
        }
    }

    fn geometry() -> Geometry {
        Geometry::new(
            Rectangle::with_size(Size::new(256.0, MEDIA_MIN_HEIGHT)),
            110.0,
        )
    }

    fn send(
        input: &mut Interaction,
        event: Event,
        point: Option<Point>,
        now: Instant,
    ) -> (Vec<Message>, bool) {
        input.now = now;
        let mut messages = Vec::new();
        let captured;
        {
            let mut shell = Shell::new(&mut messages);
            input.handle(&event, point, &geometry(), playback(), key(), &mut shell);
            captured = shell.is_event_captured();
        }
        (messages, captured)
    }

    fn moved(point: Point) -> Event {
        Event::Mouse(mouse::Event::CursorMoved { position: point })
    }
    fn redraw(now: Instant) -> Event {
        Event::Window(window::Event::RedrawRequested(now))
    }
    fn press() -> Event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
    }
    fn release() -> Event {
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
    }
    fn keyboard(key: Named, modifiers: keyboard::Modifiers) -> Event {
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
    fn hover_timeout_and_leave_work_for_playing_and_paused_video() {
        let start = Instant::now();
        let point = Point::new(30.0, 30.0);
        for paused in [false, true] {
            let mut input = Interaction::new(start);
            let mut playback = playback();
            playback.paused = paused;
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            input.handle(
                &moved(point),
                Some(point),
                &geometry(),
                playback,
                key(),
                &mut shell,
            );
            assert!(input.visible());
            input.handle(
                &redraw(start + Duration::from_millis(2999)),
                Some(point),
                &geometry(),
                playback,
                key(),
                &mut shell,
            );
            assert!(input.visible());
            input.handle(
                &redraw(start + HIDE_AFTER),
                Some(point),
                &geometry(),
                playback,
                key(),
                &mut shell,
            );
            assert!(!input.visible());
            input.handle(
                &moved(point),
                Some(point),
                &geometry(),
                playback,
                key(),
                &mut shell,
            );
            assert!(input.visible());
            input.handle(
                &moved(Point::new(300.0, 10.0)),
                None,
                &geometry(),
                playback,
                key(),
                &mut shell,
            );
            assert!(!input.visible());
            assert!(messages.is_empty());
        }
    }

    #[test]
    fn hidden_controls_do_not_click_through_and_blank_video_does_not_toggle() {
        let start = Instant::now();
        let mut input = Interaction::new(start);
        let play = geometry().rect(Control::Play).center();
        send(&mut input, moved(play), Some(play), start);
        let late = start + HIDE_AFTER;
        send(&mut input, redraw(late), Some(play), late);
        assert!(!input.visible());
        assert!(send(&mut input, press(), Some(play), late).0.is_empty());
        assert!(send(&mut input, release(), Some(play), late).0.is_empty());
        send(&mut input, moved(play), Some(play), late);
        assert!(send(&mut input, press(), Some(play), late).1);
        assert!(matches!(
            send(&mut input, release(), Some(play), late).0.as_slice(),
            [Message::Pause]
        ));
        let blank = Point::new(30.0, 30.0);
        send(&mut input, moved(blank), Some(blank), late);
        assert!(send(&mut input, press(), Some(blank), late).0.is_empty());
        assert!(send(&mut input, release(), Some(blank), late).0.is_empty());
    }

    #[test]
    fn seek_drag_survives_timeout_and_widget_exit_and_releases_once() {
        let start = Instant::now();
        let mut input = Interaction::new(start);
        let seek = geometry().rect(Control::Seek).center();
        send(&mut input, moved(seek), Some(seek), start);
        assert!(matches!(
            send(&mut input, press(), Some(seek), start).0.as_slice(),
            [Message::Seek(_)]
        ));
        let late = start + Duration::from_secs(10);
        let outside = Point::new(500.0, 500.0);
        assert!(matches!(
            send(&mut input, moved(outside), Some(outside), late)
                .0
                .as_slice(),
            [Message::Seek(120.0)]
        ));
        assert!(input.visible());
        assert!(matches!(
            send(&mut input, release(), Some(outside), late)
                .0
                .as_slice(),
            [Message::SeekRelease]
        ));
        assert!(!input.visible());
        assert!(
            send(&mut input, release(), Some(outside), late)
                .0
                .is_empty()
        );
    }

    #[test]
    fn focus_loss_and_leaving_the_window_finish_drag_without_reappearing() {
        let now = Instant::now();
        let point = geometry().rect(Control::Seek).center();
        for event in [
            Event::Window(window::Event::Unfocused),
            Event::Mouse(mouse::Event::CursorLeft),
        ] {
            let mut input = Interaction::new(now);
            send(&mut input, moved(point), Some(point), now);
            send(&mut input, press(), Some(point), now);
            assert!(matches!(
                send(&mut input, event.clone(), Some(point), now)
                    .0
                    .as_slice(),
                [Message::SeekRelease]
            ));
            assert!(!input.visible());
            send(
                &mut input,
                redraw(now),
                if matches!(event, Event::Window(_)) {
                    Some(point)
                } else {
                    None
                },
                now,
            );
            assert!(!input.visible());
            assert!(input.drag.is_none());
        }
    }

    #[test]
    fn volume_popup_has_a_continuous_hot_area_and_retains_drag() {
        let now = Instant::now();
        let g = geometry();
        let mute = g.rect(Control::Mute).center();
        let mut input = Interaction::new(now);
        send(&mut input, moved(mute), Some(mute), now);
        assert!(input.popup());
        send(&mut input, press(), Some(mute), now);
        assert!(matches!(
            send(&mut input, release(), Some(mute), now).0.as_slice(),
            [Message::Mute]
        ));
        for y in (g.popup.y as i32..mute.y as i32).rev() {
            let point = Point::new(mute.x, y as f32);
            send(&mut input, moved(point), Some(point), now);
            assert!(input.popup());
        }
        let volume = g.rect(Control::Volume).center();
        send(&mut input, moved(volume), Some(volume), now);
        assert!(matches!(
            send(&mut input, press(), Some(volume), now).0.as_slice(),
            [Message::Volume(0.5)]
        ));
        // Horizontal movement does not affect a vertical slider's value.
        let sideways = Point::new(-20.0, volume.y);
        assert!(matches!(
            send(&mut input, moved(sideways), Some(sideways), now)
                .0
                .as_slice(),
            [Message::Volume(0.5)]
        ));
        let outside = Point::new(-20.0, -20.0);
        assert!(matches!(
            send(&mut input, moved(outside), Some(outside), now + HIDE_AFTER)
                .0
                .as_slice(),
            [Message::Volume(1.0)]
        ));
        assert!(input.visible());
        assert!(input.popup());
        let below = Point::new(volume.x, g.bounds.y + g.bounds.height + 20.0);
        assert!(matches!(
            send(&mut input, moved(below), Some(below), now + HIDE_AFTER)
                .0
                .as_slice(),
            [Message::Volume(0.0)]
        ));
        assert!(
            send(&mut input, release(), Some(outside), now + HIDE_AFTER)
                .0
                .is_empty()
        );
        assert!(!input.visible());
        assert!(!input.popup());
    }

    #[test]
    fn keyboard_focus_reveals_controls_and_preserves_tab_escape_and_shortcuts() {
        let now = Instant::now();
        let mut input = Interaction::new(now);
        input.focus[Control::Seek as usize].focus();
        send(&mut input, redraw(now + HIDE_AFTER), None, now + HIDE_AFTER);
        assert!(input.visible());
        send(
            &mut input,
            Event::Mouse(mouse::Event::CursorLeft),
            None,
            now + HIDE_AFTER,
        );
        assert!(input.visible());
        assert_eq!(input.focused(), Some(Control::Seek));
        let (messages, captured) = send(
            &mut input,
            keyboard(Named::ArrowRight, keyboard::Modifiers::empty()),
            None,
            now,
        );
        assert!(captured);
        assert!(matches!(
            messages.as_slice(),
            [Message::Seek(25.0), Message::SeekRelease]
        ));
        for pressed in [Named::Tab, Named::Escape] {
            assert!(
                !send(
                    &mut input,
                    keyboard(pressed, keyboard::Modifiers::empty()),
                    None,
                    now
                )
                .1
            );
        }
        assert!(
            !send(
                &mut input,
                keyboard(Named::ArrowRight, keyboard::Modifiers::CTRL),
                None,
                now
            )
            .1
        );
        input.focus[Control::Seek as usize].unfocus();
        input.focus[Control::Volume as usize].focus();
        send(&mut input, redraw(now), None, now);
        assert!(input.popup());
        assert!(
            matches!(send(&mut input, keyboard(Named::ArrowUp, keyboard::Modifiers::empty()), None, now).0.as_slice(), [Message::Volume(v)] if (*v - 0.75).abs() < 0.001)
        );
        input.focus[Control::Volume as usize].unfocus();
        send(&mut input, redraw(now), None, now);
        assert!(!input.visible());
    }

    #[test]
    fn playback_file_window_and_fullscreen_changes_clear_transient_state() {
        let first = key();
        for next in [
            Key {
                file: Some(2),
                ..first
            },
            Key {
                owner: window::Id::unique(),
                ..first
            },
            Key {
                full: true,
                ..first
            },
        ] {
            let player = Player {
                content: iced::widget::space().into(),
                title: "video",
                pending: false,
                key: first,
                playback: playback(),
            };
            let mut tree = Tree::new(&player as &dyn Widget<Message, Theme, Renderer>);
            let state = tree.state.downcast_mut::<State>();
            state.input.volume_open = true;
            state.input.drag = Some(Control::Volume);
            state.input.focus[1].focus();
            let player = Player {
                key: next,
                ..player
            };
            player.diff(&mut tree);
            let state = tree.state.downcast_ref::<State>();
            assert!(state.input.drag.is_none());
            assert!(state.input.focused().is_none());
            assert!(!state.input.visible());
            assert!(!state.input.popup());
        }
    }

    #[test]
    fn narrow_long_duration_and_portrait_layouts_keep_controls_in_bounds() {
        for size in [
            Size::new(256.0, 128.0),
            Size::new(256.0, 455.0),
            Size::new(399.0, 240.0),
            Size::new(400.0, 239.0),
            Size::new(400.0, 240.0),
            Size::new(596.0, 128.0),
            Size::new(596.0, 335.0),
            Size::new(1920.0, 1080.0),
        ] {
            for time_width in [70.0, 140.0, 210.0] {
                let g = Geometry::new(Rectangle::new(Point::new(30.0, 20.0), size), time_width);
                for rect in g
                    .controls
                    .into_iter()
                    .chain([g.header, g.title, g.time, g.popup, g.percent, g.footer])
                {
                    assert!(rect.x >= g.bounds.x && rect.y >= g.bounds.y);
                    assert!(rect.x + rect.width <= g.bounds.x + size.width);
                    assert!(rect.y + rect.height <= g.bounds.y + size.height);
                }
                assert!(g.title.y + g.title.height <= g.footer.y);
                assert!(g.header.intersection(&g.footer).is_none());
                let volume = g.rect(Control::Volume);
                let rail = g.rail(Control::Volume);
                assert!(rail.height > rail.width);
                assert_eq!(rail.center_x(), g.rect(Control::Mute).center_x());
                assert_eq!(g.popup.y + g.popup.height, g.rect(Control::Mute).y);
                assert!(g.percent.intersection(&volume).is_none());
                let radius = g.metrics.thumb_radius;
                assert!(
                    g.popup
                        .contains(Point::new(rail.center_x(), rail.y - radius))
                );
                assert!(
                    g.popup
                        .contains(Point::new(rail.center_x(), rail.y + rail.height + radius))
                );
                for (index, left) in g.controls.iter().enumerate() {
                    if index == Control::Volume as usize {
                        continue;
                    }
                    for (other, right) in g.controls.iter().enumerate().skip(index + 1) {
                        if other != Control::Volume as usize {
                            assert!(left.intersection(right).is_none());
                        }
                    }
                }
                assert!(g.time.width >= time_width);
            }
        }
    }

    #[test]
    fn mini_switches_on_actual_player_size_and_releases_more_picture_area() {
        for (size, expected) in [
            (Size::new(399.0, 600.0), MediaMetrics::MINI),
            (Size::new(1200.0, 239.0), MediaMetrics::MINI),
            (Size::new(400.0, 240.0), MediaMetrics::REGULAR),
            (Size::new(1920.0, 1080.0), MediaMetrics::REGULAR),
        ] {
            // Time-label length may move the timestamp onto a second line,
            // but must never change the size mode or cause a feedback loop.
            for time_width in [70.0, 210.0] {
                assert_eq!(
                    Geometry::new(Rectangle::with_size(size), time_width).metrics,
                    expected
                );
            }
        }
        let mini = Geometry::new(Rectangle::with_size(Size::new(256.0, 455.0)), 110.0);
        let regular = Geometry::new(Rectangle::with_size(Size::new(596.0, 455.0)), 110.0);
        assert_eq!(mini.header.height + mini.footer.height, 72.0);
        assert_eq!(regular.header.height + regular.footer.height, 108.0);
        assert!(mini.popup.width * mini.popup.height < regular.popup.width * regular.popup.height);
        assert!(mini.metrics.title_size < regular.metrics.title_size);
        assert!(mini.metrics.text_size < regular.metrics.text_size);
        assert!(mini.metrics.icon < regular.metrics.icon);
        for g in [&mini, &regular] {
            for control in CONTROLS {
                assert_eq!(
                    g.hit(g.rect(control).center(), control == Control::Volume, true),
                    Some(control)
                );
            }
        }
    }

    #[test]
    fn resizing_between_modes_preserves_focus_drag_and_control_actions() {
        let player_key = key();
        let player = |size: Size| Player {
            content: iced::widget::space()
                .width(size.width)
                .height(size.height)
                .into(),
            title: "很长的视频名称.mp4",
            pending: false,
            key: player_key,
            playback: playback(),
        };
        let regular = Size::new(596.0, 335.0);
        let mini = Size::new(256.0, 144.0);
        let initial = player(regular);
        let mut tree = Tree::new(&initial as &dyn Widget<Message, Theme, Renderer>);
        let now = Instant::now();
        {
            let input = &mut tree.state.downcast_mut::<State>().input;
            input.now = now;
            input.focus[Control::Volume as usize].focus();
            input.drag = Some(Control::Volume);
            input.pressed = Some(Control::Volume);
        }
        for size in [mini, regular, mini, regular] {
            player(size).diff(&mut tree);
            let input = &mut tree.state.downcast_mut::<State>().input;
            assert_eq!(input.focused(), Some(Control::Volume));
            assert_eq!(input.drag, Some(Control::Volume));
            let g = Geometry::new(Rectangle::with_size(size), 110.0);
            let position = g.rail(Control::Volume).center();
            let mut messages = Vec::new();
            input.handle(
                &moved(position),
                Some(position),
                &g,
                playback(),
                player_key,
                &mut Shell::new(&mut messages),
            );
            assert!(matches!(messages.as_slice(), [Message::Volume(0.5)]));
            assert!(input.visible());
            assert!(input.popup());
        }
        let input = &mut tree.state.downcast_mut::<State>().input;
        let g = Geometry::new(Rectangle::with_size(regular), 110.0);
        let mut messages = Vec::new();
        input.handle(
            &release(),
            None,
            &g,
            playback(),
            player_key,
            &mut Shell::new(&mut messages),
        );
        input.focus[Control::Volume as usize].unfocus();
        input.focus[Control::Fullscreen as usize].focus();
        input.handle(
            &keyboard(Named::Enter, keyboard::Modifiers::empty()),
            None,
            &g,
            playback(),
            player_key,
            &mut Shell::new(&mut messages),
        );
        assert!(
            matches!(messages.as_slice(), [Message::Fullscreen(owner)] if *owner == player_key.owner)
        );
    }

    #[test]
    fn ellipsis_handles_chinese_unicode_and_line_breaks_without_overflow() {
        let measure = |s: &str| s.chars().count() as f32 * 12.0;
        assert_eq!(
            ellipsize("测试🎬很长的视频名称.mp4", 60.0, measure),
            "测试🎬很…"
        );
        assert_eq!(ellipsize("小视频.mp4", 200.0, measure), "小视频.mp4");
        assert_eq!(ellipsize("一\n二", 100.0, measure), "一 二");
        assert_eq!(ellipsize("长文件名", 1.0, measure), "");
    }

    #[test]
    fn portrait_keyboard_focus_scrolls_only_when_outside_the_viewport() {
        let viewport = Rectangle::new(Point::new(20.0, 100.0), Size::new(256.0, 400.0));
        let content = Rectangle {
            height: 1000.0,
            ..viewport
        };
        let target = Rectangle::new(Point::new(28.0, 850.0), Size::new(32.0, 32.0));
        assert_eq!(reveal_offset(viewport, content, 0.0, target), Some(390.0));
        assert_eq!(reveal_offset(viewport, content, 390.0, target), None);
        let target = Rectangle { y: 110.0, ..target };
        assert_eq!(reveal_offset(viewport, content, 390.0, target), Some(2.0));
    }

    #[test]
    fn interaction_end_restarts_the_timeout_and_clipped_content_is_not_hoverable() {
        let now = Instant::now();
        let mut input = Interaction::new(now);
        let play = geometry().rect(Control::Play).center();
        send(&mut input, moved(play), Some(play), now);
        send(&mut input, press(), Some(play), now);
        let late = now + Duration::from_secs(12);
        send(&mut input, redraw(late), Some(play), late);
        assert!(input.visible());
        send(&mut input, release(), Some(play), late);
        send(
            &mut input,
            redraw(late + Duration::from_secs(2)),
            Some(play),
            late + Duration::from_secs(2),
        );
        assert!(input.visible());
        send(
            &mut input,
            redraw(late + HIDE_AFTER),
            Some(play),
            late + HIDE_AFTER,
        );
        assert!(!input.visible());
        // The wrapper filters out points outside the scroll viewport even when
        // the video itself extends under them.
        send(&mut input, moved(play), None, late + HIDE_AFTER);
        assert!(!input.visible());
        assert!(!send(&mut input, press(), None, late + HIDE_AFTER).1);
    }

    #[test]
    fn unknown_duration_disables_seeking_without_disabling_playback_buttons() {
        let mut playback = playback();
        playback.duration = 0.0;
        let now = Instant::now();
        let mut input = Interaction::new(now);
        let g = geometry();
        let point = g.rect(Control::Seek).center();
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        input.handle(&moved(point), Some(point), &g, playback, key(), &mut shell);
        input.handle(&press(), Some(point), &g, playback, key(), &mut shell);
        assert!(input.drag.is_none());
        assert!(!shell.is_event_captured());
        assert_eq!(
            g.hit(g.rect(Control::Play).center(), false, false),
            Some(Control::Play)
        );
        assert!(messages.is_empty());
    }
}
