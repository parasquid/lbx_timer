//! Small vector icons drawn with iced's canvas, so they render identically on
//! every system instead of depending on which fonts provide media glyphs.

use iced::mouse;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path};
use iced::{Element, Length, Point, Rectangle, Renderer, Size, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
}

/// A square icon of `size` logical pixels, in the theme's secondary-button
/// text color.
pub fn icon<'a, Message: 'a>(icon: Icon, size: f32) -> Element<'a, Message> {
    Canvas::new(IconProgram { icon })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

struct IconProgram {
    icon: Icon,
}

impl<Message> canvas::Program<Message> for IconProgram {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let color = theme.extended_palette().secondary.base.text;
        let side = bounds.width.min(bounds.height);
        let origin = Point::new((bounds.width - side) / 2.0, (bounds.height - side) / 2.0);
        match self.icon {
            Icon::Play => {
                // An equilateral-ish triangle, nudged right so it looks
                // centered (its visual weight sits left of its box center).
                let height = side * 0.78;
                let width = height * 0.88;
                let left = origin.x + (side - width) / 2.0 + width * 0.08;
                let top = origin.y + (side - height) / 2.0;
                let triangle = Path::new(|path| {
                    path.move_to(Point::new(left, top));
                    path.line_to(Point::new(left + width, top + height / 2.0));
                    path.line_to(Point::new(left, top + height));
                    path.close();
                });
                frame.fill(&triangle, color);
            }
            Icon::Pause => {
                let height = side * 0.74;
                let bar = side * 0.24;
                let gap = side * 0.2;
                let left = origin.x + (side - (bar * 2.0 + gap)) / 2.0;
                let top = origin.y + (side - height) / 2.0;
                for x in [left, left + bar + gap] {
                    let rect = Path::rounded_rectangle(
                        Point::new(x, top),
                        Size::new(bar, height),
                        (bar * 0.25).into(),
                    );
                    frame.fill(&rect, color);
                }
            }
        }
        vec![frame.into_geometry()]
    }
}
