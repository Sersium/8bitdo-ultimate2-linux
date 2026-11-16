//! Upstream controller artwork rendered with egui; shared by live input and gyro.
use crate::{
    artwork::{self, Part, Shape},
    motion::{Quaternion, View},
};
use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
const LINE: Color32 = Color32::from_rgb(226, 228, 236);
const ACCENT: Color32 = Color32::from_rgb(157, 117, 235);
const GREEN: Color32 = Color32::from_rgb(117, 226, 164);
const BG: Color32 = Color32::from_rgb(23, 28, 35);

fn smooth(points: &[[f32; 2]], closed: bool) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(points.len() * 6);
    for i in 0..points.len() {
        let p = points[i];
        let prev = points[if i == 0 {
            if closed { points.len() - 1 } else { 0 }
        } else {
            i - 1
        }];
        let next = points[if i + 1 == points.len() {
            if closed { 0 } else { i }
        } else {
            i + 1
        }];
        for k in 0..6 {
            let t = k as f32 / 6.;
            out.push(std::array::from_fn(|j| {
                (1. - t) * (1. - t) * (prev[j] + p[j]) * 0.5
                    + 2. * (1. - t) * t * p[j]
                    + t * t * (p[j] + next[j]) * 0.5
            }));
        }
    }
    out
}
fn projected(q: Quaternion, [x, y, z]: [f32; 3], motion: bool) -> Vec2 {
    if !motion {
        return Vec2::new(x, y);
    }
    let [x, y, z] = q.rotate([x, -y, z]);
    let depth = z * 0.8660254 - y * 0.5;
    Vec2::new(x, -y * 0.8660254 - z * 0.5) * (900. / (900. - depth))
}
fn ring(x: f32, y: f32, r: f32) -> Vec<[f32; 2]> {
    (0..40)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / 40.;
            [x + r * a.cos(), y + r * a.sin()]
        })
        .collect()
}
fn part_points(p: &Part) -> Vec<[f32; 2]> {
    let [x, y] = p.center;
    match p.shape {
        Shape::Circle(r) => ring(x, y, r),
        Shape::Polygon(points) => smooth(points, true),
        Shape::Rect(w, h) => vec![
            [x - w, y - h],
            [x + w, y - h],
            [x + w, y + h],
            [x - w, y + h],
        ],
    }
}

pub fn controller(
    ui: &mut egui::Ui,
    view: &View,
    follow: bool,
    lights: bool,
    height: f32,
    zones: [f32; 4],
) -> Option<(&'static str, usize)> {
    let width = ui.available_width();
    let callouts = !follow && width > 430. && height >= 280.;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let available = if callouts { width - 210. } else { width - 24. };
    let scale = (available / 440.).min((height - 20.) / 340.);
    let q = if follow && view.has_imu {
        view.orientation
    } else {
        Quaternion::default()
    };
    let project =
        |x: f32, y: f32, z: f32| center + projected(q, [x - 220., y - 165., z], follow) * scale;
    let polygon = |points: &[[f32; 2]], z: f32, fill: Color32, stroke: Stroke| {
        painter.add(egui::Shape::convex_polygon(
            points.iter().map(|p| project(p[0], p[1], z)).collect(),
            fill,
            stroke,
        ))
    };
    let mut action = None;
    let front = q.rotate([0., 0., 1.])[2] >= 0.;
    let paint_part = |p: &Part| {
        let on = view.sample.buttons & p.bit != 0;
        let fill = if on {
            Color32::from_rgb(52, 116, 84)
        } else {
            BG
        };
        polygon(
            &part_points(p),
            20.,
            fill,
            Stroke::new(if on { 2. } else { 1.2 }, if on { GREEN } else { LINE }),
        );
        let [x, y] = p.center;
        if p.name == "Home" {
            for (j, row) in artwork::HEART.iter().enumerate() {
                for (i, c) in row.bytes().enumerate() {
                    if c == b'X' {
                        let px = x + (i as f32 - 5.5) * 1.8;
                        let py = y + (j as f32 - 4.5) * 1.8;
                        polygon(
                            &[
                                [px, py],
                                [px + 1.8, py],
                                [px + 1.8, py + 1.8],
                                [px, py + 1.8],
                            ],
                            22.,
                            LINE,
                            Stroke::NONE,
                        );
                    }
                }
            }
        } else if p.name == "Star" {
            let pts = (0..10)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 10. - std::f32::consts::FRAC_PI_2;
                    let r = if i % 2 == 0 { 6. } else { 2.5 };
                    project(x + r * a.cos(), y + r * a.sin(), 22.)
                })
                .collect();
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.2, LINE)));
        } else if p.name == "Select" {
            painter.line_segment(
                [project(x - 4., y, 22.), project(x + 4., y, 22.)],
                Stroke::new(1.2, LINE),
            );
        } else if !p.label.is_empty() {
            painter.text(
                project(x, y, 22.),
                egui::Align2::CENTER_CENTER,
                p.label,
                egui::FontId::proportional(if ["A", "B", "X", "Y"].contains(&p.name) {
                    13.
                } else {
                    10.
                }),
                LINE,
            );
        }
    };
    for p in artwork::PARTS
        .iter()
        .filter(|p| ["LT", "RT"].contains(&p.name))
    {
        paint_part(p);
    }
    if follow {
        let bottom: Vec<Pos2> = smooth(artwork::BODY, true)
            .iter()
            .map(|p| project(p[0], p[1], -12.))
            .collect();
        painter.add(egui::Shape::closed_line(
            bottom,
            Stroke::new(2., Color32::from_rgb(91, 102, 120)),
        ));
    }
    let mut mesh = egui::Mesh::default();
    for p in artwork::BODY {
        mesh.colored_vertex(project(p[0], p[1], 12.), BG);
    }
    mesh.indices.extend_from_slice(artwork::TRIANGLES);
    painter.add(egui::Shape::mesh(mesh));
    painter.add(egui::Shape::closed_line(
        smooth(artwork::BODY, true)
            .iter()
            .map(|p| project(p[0], p[1], 12.))
            .collect(),
        Stroke::new(2., LINE),
    ));
    for points in artwork::DETAILS {
        painter.add(egui::Shape::line(
            smooth(points, false)
                .iter()
                .map(|p| project(p[0], p[1], 15.))
                .collect(),
            Stroke::new(0.8, Color32::from_gray(112)),
        ));
    }
    for p in artwork::PARTS
        .iter()
        .filter(|p| !["LT", "RT"].contains(&p.name))
    {
        if !front && !["PL", "PR"].contains(&p.name) {
            continue;
        }
        paint_part(p);
        let pts = part_points(p);
        let bounds = egui::Rect::from_points(
            &pts.iter()
                .map(|p| project(p[0], p[1], 20.))
                .collect::<Vec<_>>(),
        )
        .expand(3.);
        if ui
            .interact(
                bounds,
                ui.scope_id().with(("physical-part", p.name)),
                egui::Sense::click(),
            )
            .on_hover_text(p.name)
            .clicked()
        {
            action = Some((p.name, if ["L3", "R3"].contains(&p.name) { 2 } else { 1 }));
        }
    }
    if front {
        for (index, (x, y)) in [(93., 106.), (281., 177.)].iter().enumerate() {
            polygon(
                &ring(*x, *y, 37.),
                20.,
                Color32::TRANSPARENT,
                Stroke::new(
                    2.,
                    if lights {
                        GREEN
                    } else {
                        Color32::from_gray(98)
                    },
                ),
            );
            polygon(
                &ring(*x, *y, 27.),
                21.,
                Color32::TRANSPARENT,
                Stroke::new(1., LINE),
            );
            let dx = view.sample.sticks[index * 2];
            let dy = view.sample.sticks[index * 2 + 1];
            polygon(
                &ring(*x + dx * 8., *y + dy * 8., 17.),
                22.,
                BG,
                Stroke::new(1., LINE),
            );
            let inner = zones[index * 2].clamp(0., 1.);
            let outer = zones[index * 2 + 1].clamp(inner, 1.);
            if inner > 0. {
                polygon(
                    &ring(*x, *y, 32. * inner),
                    23.,
                    Color32::from_rgba_unmultiplied(239, 193, 94, 40),
                    Stroke::new(1.5, Color32::from_rgb(239, 193, 94)),
                );
            }
            if outer < 0.999 {
                polygon(
                    &ring(*x, *y, 32. * outer),
                    23.,
                    Color32::TRANSPARENT,
                    Stroke::new(1.5, Color32::LIGHT_BLUE),
                );
            }
            polygon(
                &ring(*x + dx * 32., *y + dy * 32., 3.),
                24.,
                Color32::WHITE,
                Stroke::NONE,
            );
        }
        for d in artwork::DECOR {
            polygon(
                &[[d[0], d[1]], [d[2], d[1]], [d[2], d[3]], [d[0], d[3]]],
                20.,
                BG,
                Stroke::new(1., LINE),
            );
        }
    }
    for (i, x) in [90., 350.].iter().enumerate() {
        if view.rumble[i] > 0. {
            polygon(
                &ring(*x, 265., 25.),
                23.,
                Color32::TRANSPARENT,
                Stroke::new(2., GREEN),
            );
        }
    }
    if callouts {
        for (label, left, fraction, target, key, section) in [
            ("LT", true, 0.10, [96., 12.], "LT", 1),
            ("L4", true, 0.19, [142., 21.], "L4", 1),
            ("LB", true, 0.28, [54., 42.], "LB", 1),
            ("Left stick", true, 0.45, [61., 106.], "L3", 2),
            ("D-pad", true, 0.68, [121., 175.], "Up", 1),
            ("RT", false, 0.10, [344., 12.], "RT", 1),
            ("R4", false, 0.19, [298., 21.], "R4", 1),
            ("RB", false, 0.28, [386., 42.], "RB", 1),
            ("ABXY", false, 0.45, [394., 104.], "A", 1),
            ("Right stick", false, 0.68, [313., 177.], "R3", 2),
            ("Back PL / PR", false, 0.92, [264., 308.], "PL", 1),
        ] {
            let y = rect.top() + height * fraction;
            let box_rect = egui::Rect::from_min_size(
                Pos2::new(
                    if left {
                        rect.left() + 3.
                    } else {
                        rect.right() - 99.
                    },
                    y - 11.,
                ),
                Vec2::new(96., 22.),
            );
            let hit = ui.interact(
                box_rect,
                ui.scope_id().with(("callout", label)),
                egui::Sense::click(),
            );
            painter.rect_stroke(
                box_rect,
                4.,
                Stroke::new(1., ACCENT),
                egui::StrokeKind::Inside,
            );
            painter.text(
                box_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.),
                LINE,
            );
            let edge = Pos2::new(
                if left {
                    box_rect.right()
                } else {
                    box_rect.left()
                },
                y,
            );
            let to = project(target[0], target[1], 22.);
            painter.line_segment([edge, to], Stroke::new(1., ACCENT));
            painter.circle_filled(to, 2.5, ACCENT);
            if hit.clicked() {
                action = Some((key, section));
            }
        }
    }
    action
}

pub fn section_icon(p: &egui::Painter, c: Pos2, index: usize) {
    let stroke = Stroke::new(1.5, LINE);
    let ellipse = |rx: f32, ry: f32| {
        p.add(egui::Shape::closed_line(
            (0..32)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 32.;
                    c + Vec2::new(a.cos() * rx, a.sin() * ry)
                })
                .collect(),
            stroke,
        ))
    };
    match index {
        1 => {
            for [x, y] in [[0., -8.], [0., 8.], [-8., 0.], [8., 0.]] {
                p.circle_stroke(c + Vec2::new(x, y), 3.5, stroke);
            }
        }
        2 => {
            for r in [12., 6., 2.] {
                ellipse(r, r);
            }
        }
        3 => {
            for x in [-9., -3., 3., 9.] {
                p.add(egui::Shape::line(
                    vec![
                        c + Vec2::new(x, -10.),
                        c + Vec2::new(x - 2., -4.),
                        c + Vec2::new(x + 2., 4.),
                        c + Vec2::new(x, 10.),
                    ],
                    stroke,
                ));
            }
        }
        4 => {
            ellipse(13., 5.);
            ellipse(5., 13.);
            p.circle_filled(c, 2., LINE);
        }
        5 => {
            ellipse(5., 5.);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.;
                let v = Vec2::new(a.cos(), a.sin());
                p.line_segment([c + v * 8., c + v * 12.], stroke);
            }
        }
        6 => {
            p.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::new(18., 22.)),
                3.,
                stroke,
                egui::StrokeKind::Inside,
            );
            for y in [-5., 0., 5.] {
                p.line_segment([c + Vec2::new(-5., y), c + Vec2::new(5., y)], stroke);
            }
        }
        _ => {
            p.circle_stroke(c + Vec2::new(0., -6.), 4., stroke);
            p.add(egui::Shape::line(
                vec![
                    c + Vec2::new(-9., 11.),
                    c + Vec2::new(-7., 4.),
                    c + Vec2::new(7., 4.),
                    c + Vec2::new(9., 11.),
                ],
                stroke,
            ));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_mesh_and_part_mapping_are_valid() {
        assert!(
            artwork::TRIANGLES
                .iter()
                .all(|&i| i < artwork::BODY.len() as u32)
        );
        assert_eq!(artwork::PARTS.len(), 22);
        assert!(
            artwork::PARTS
                .iter()
                .all(|p| crate::motion::BUTTON_BITS.contains(&p.bit))
        );
    }
    #[test]
    fn left_wheel_rotation_moves_right_grip_up() {
        let h = 45f32.to_radians();
        let q = Quaternion([h.cos(), 0., 0., h.sin()]);
        let p = projected(q, [100., 0., 0.], true);
        assert!(p.x.abs() < 0.01 && p.y < -80.);
    }
}
