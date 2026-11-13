//! Icone vettoriali originali dei dispositivi, disegnate con il painter di egui.

use eframe::egui::{epaint::PathShape, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};
use npt_core::DeviceKind;

pub const ICON_SIZE: Vec2 = Vec2::new(64.0, 48.0);

const OUTLINE: Color32 = Color32::from_rgb(40, 48, 60);
const SCREEN: Color32 = Color32::from_rgb(120, 190, 235);
const BODY: Color32 = Color32::from_rgb(214, 220, 228);

pub fn kind_color(kind: DeviceKind) -> Color32 {
    match kind {
        DeviceKind::Pc | DeviceKind::Laptop => Color32::from_rgb(120, 190, 235),
        DeviceKind::Server => Color32::from_rgb(150, 160, 175),
        DeviceKind::Hub => Color32::from_rgb(180, 170, 140),
        DeviceKind::Switch => Color32::from_rgb(40, 150, 140),
        DeviceKind::Router => Color32::from_rgb(55, 110, 190),
    }
}

/// Disegna l'icona centrata in `c`, scalata di `s` (1.0 = dimensione normale).
pub fn draw_device(p: &Painter, c: Pos2, kind: DeviceKind, s: f32) {
    let stroke = Stroke::new(1.5 * s, OUTLINE);
    let r = |w: f32, h: f32, dx: f32, dy: f32| Rect::from_center_size(c + Vec2::new(dx, dy) * s, Vec2::new(w, h) * s);
    match kind {
        DeviceKind::Pc => {
            let screen = r(40.0, 28.0, 0.0, -6.0);
            p.rect_filled(screen, 3.0 * s, BODY);
            p.rect_filled(screen.shrink(3.5 * s), 1.0 * s, SCREEN);
            p.rect_stroke(screen, 3.0 * s, stroke, StrokeKind::Middle);
            p.rect_filled(r(6.0, 6.0, 0.0, 11.0), 0.0, OUTLINE);
            p.rect_filled(r(22.0, 3.5, 0.0, 15.0), 1.5 * s, OUTLINE);
        }
        DeviceKind::Laptop => {
            let screen = r(36.0, 24.0, 0.0, -6.0);
            p.rect_filled(screen, 3.0 * s, BODY);
            p.rect_filled(screen.shrink(3.0 * s), 1.0 * s, SCREEN);
            p.rect_stroke(screen, 3.0 * s, stroke, StrokeKind::Middle);
            let y0 = c.y + 7.0 * s;
            let y1 = c.y + 14.0 * s;
            let base = vec![
                Pos2::new(c.x - 19.0 * s, y0),
                Pos2::new(c.x + 19.0 * s, y0),
                Pos2::new(c.x + 25.0 * s, y1),
                Pos2::new(c.x - 25.0 * s, y1),
            ];
            p.add(PathShape::convex_polygon(base, BODY, stroke));
        }
        DeviceKind::Server => {
            let body = r(28.0, 40.0, 0.0, 0.0);
            p.rect_filled(body, 3.0 * s, BODY);
            p.rect_stroke(body, 3.0 * s, stroke, StrokeKind::Middle);
            for i in 0..3 {
                let y = body.top() + (9.0 + i as f32 * 10.0) * s;
                p.line_segment([Pos2::new(body.left() + 4.0 * s, y), Pos2::new(body.right() - 9.0 * s, y)], stroke);
                p.circle_filled(Pos2::new(body.right() - 5.5 * s, y), 1.8 * s, Color32::from_rgb(80, 200, 120));
            }
        }
        DeviceKind::Hub => {
            let body = r(50.0, 18.0, 0.0, 0.0);
            p.rect_filled(body, 3.0 * s, kind_color(kind));
            p.rect_stroke(body, 3.0 * s, stroke, StrokeKind::Middle);
            for i in 0..6 {
                let x = body.left() + (7.0 + i as f32 * 7.2) * s;
                p.rect_filled(Rect::from_center_size(Pos2::new(x, c.y), Vec2::splat(4.0 * s)), 0.5, OUTLINE);
            }
        }
        DeviceKind::Switch => {
            let body = r(56.0, 22.0, 0.0, 0.0);
            p.rect_filled(body, CornerRadius::same((4.0 * s) as u8), kind_color(kind));
            p.rect_stroke(body, 4.0 * s, stroke, StrokeKind::Middle);
            let w = Stroke::new(2.0 * s, Color32::WHITE);
            // Due frecce opposte: commutazione.
            for (y, dir) in [(-4.0, 1.0), (4.0, -1.0)] {
                let a = c + Vec2::new(-16.0 * dir, y) * s;
                let b = c + Vec2::new(16.0 * dir, y) * s;
                p.line_segment([a, b], w);
                p.line_segment([b, b + Vec2::new(-5.0 * dir, -3.0) * s], w);
                p.line_segment([b, b + Vec2::new(-5.0 * dir, 3.0) * s], w);
            }
        }
        DeviceKind::Router => {
            let rad = 19.0 * s;
            p.circle_filled(c, rad, kind_color(kind));
            p.circle_stroke(c, rad, stroke);
            let w = Stroke::new(2.0 * s, Color32::WHITE);
            // Quattro frecce: due entranti, due uscenti.
            for (dx, dy, inward) in [(1.0, 0.0, false), (-1.0, 0.0, false), (0.0, 1.0, true), (0.0, -1.0, true)] {
                let d = Vec2::new(dx, dy);
                let outer = c + d * 13.0 * s;
                let inner = c + d * 4.0 * s;
                p.line_segment([inner, outer], w);
                let tip = if inward { inner } else { outer };
                let back = if inward { d } else { -d };
                let perp = Vec2::new(-d.y, d.x);
                p.line_segment([tip, tip + (back * 4.0 + perp * 3.0) * s], w);
                p.line_segment([tip, tip + (back * 4.0 - perp * 3.0) * s], w);
            }
        }
    }
}
