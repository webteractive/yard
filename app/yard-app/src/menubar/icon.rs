//! The menu bar icon: the D3 logo as an 18 pt template image, 36×36 px, black on clear (macOS
//! tints it), with a badge per health (visual design spec §6).

use yard_app_model::health::Health;

/// The icon's side in pixels (18 pt at 2x).
pub const SIZE: u32 = 36;

/// Each pixel's coverage, 0 to 255, row by row from the top: the shapes sampled 4×4 per pixel, so
/// edges are antialiased.
pub(crate) fn icon_alpha(health: &Health) -> Vec<u8> {
    let mut out = Vec::with_capacity((SIZE * SIZE) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut on = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let (px, py) = (x as f32 + (sx as f32 + 0.5) / 4., y as f32 + (sy as f32 + 0.5) / 4.);
                    on += covered(health, px, py) as u32;
                }
            }
            let mut a = on as f32 / 16. * 255.;
            if matches!(health, Health::Offline(_)) {
                a *= 0.38;
            }
            out.push(a.round() as u8);
        }
    }
    out
}

fn dist(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
}

/// The distance from (px, py) to the segment (ax, ay)–(bx, by).
fn to_segment(px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
    dist(px, py, ax + t * dx, ay + t * dy)
}

/// The two requests (upper nodes) joining and landing on the site (lower node), cut from the disc.
fn routes(px: f32, py: f32) -> bool {
    const JOIN: (f32, f32) = (18., 18.5);
    let stroke = [((12.5, 12.5), JOIN), ((23.5, 12.5), JOIN), (JOIN, (18., 24.))];
    stroke.iter().any(|(a, b)| to_segment(px, py, *a, *b) <= 2.3 / 2.)
        || dist(px, py, 12.5, 12.5) <= 2.6
        || dist(px, py, 23.5, 12.5) <= 2.6
        || dist(px, py, 18., 25.5) <= 3.2
}

/// Inside the triangle `M29 22.5 L35.5 34.5 H22.5Z`.
fn warning_triangle(px: f32, py: f32) -> bool {
    if !(22.5..=34.5).contains(&py) {
        return false;
    }
    let half = 6.5 * (py - 22.5) / 12.;
    (29. - half..=29. + half).contains(&px)
}

fn covered(health: &Health, px: f32, py: f32) -> bool {
    let disc = dist(px, py, 18., 18.) <= 15. && !routes(px, py);
    let badge = |inside: bool| (disc && dist(px, py, 29., 29.) > 9.) || inside;
    match health {
        Health::Ok | Health::Offline(_) => disc,
        Health::Busy => badge((dist(px, py, 29., 29.) - 4.6).abs() <= 1.1),
        Health::Warning(_) => badge(warning_triangle(px, py)),
        Health::Error(_) => badge(dist(px, py, 29., 29.) <= 5.8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(health: &Health, x: u32, y: u32) -> u8 {
        icon_alpha(health)[(y * SIZE + x) as usize]
    }

    /// Each health has a shape of its own, read at points the checklist's `ax.js icon` reads too:
    /// a body point on the disc, one on a route, the corner the badge cuts away, the badge's centre
    /// and the warning triangle's foot (warning and error looked the same as a notched circle).
    #[test]
    fn the_icon_tells_each_health_apart() {
        let ok = Health::Ok;
        let busy = Health::Busy;
        let warning = Health::Warning(vec![]);
        let error = Health::Error(vec![]);
        let offline = Health::Offline("x".into());
        let (body, route, corner, badge, foot) = ((7, 18), (18, 21), (24, 24), (28, 28), (24, 33));
        let a = |h: &Health, (x, y): (u32, u32)| at(h, x, y);
        assert_eq!((a(&ok, body), a(&ok, route), a(&ok, corner)), (255, 0, 255));
        assert_eq!((a(&busy, body), a(&busy, corner), a(&busy, badge)), (255, 0, 0));
        assert_eq!(
            (a(&warning, body), a(&warning, corner), a(&warning, badge), a(&warning, foot)),
            (255, 0, 255, 255)
        );
        assert_eq!((a(&error, body), a(&error, corner), a(&error, badge), a(&error, foot)), (255, 0, 255, 0));
        for p in [body, corner] {
            assert!((94..=100).contains(&a(&offline, p)), "offline is the icon at 38%: {}", a(&offline, p));
        }
    }
}
